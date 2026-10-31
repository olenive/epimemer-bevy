//! The event stream in a browser: the page's WebSocket, followed by a task
//! on the page's event loop (`background.rs`).
//!
//! The socket's callbacks pass each message to the app as it comes, and the
//! task looks every [`POLL`] for the app's commands and for the socket
//! having closed, as the native thread looks between reads. A message that
//! arrives before the app has been told the socket opened is held until
//! then, so the app sees the order the native client gives. The address is
//! the page's own `/ws`, which the hub serves, or which `trunk serve` passes
//! on to it (`Trunk.toml`), so the socket is never cross-origin.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

use super::{Command, Ended, Item, Subscription, parse, take_commands};
use crate::background::sleep;
use crate::browser::describe;

/// How often the task looks for a command from the app and for the socket
/// having closed.
const POLL: Duration = Duration::from_millis(50);
/// The longest a socket may take to open, a connection and a handshake, as
/// natively.
const OPEN_TIMEOUT: Duration = Duration::from_secs(8);

/// What the socket's callbacks share with the task.
#[derive(Default)]
struct Shared {
    /// Where a message goes once the app has been told the socket opened.
    items: RefCell<Option<Sender<Item>>>,
    /// The messages that came before that.
    early: RefCell<Vec<String>>,
    /// Why the socket closed, once it has.
    closed: RefCell<Option<String>>,
    /// The app's end of the channel has gone.
    app_gone: Cell<bool>,
}

/// An open socket and its callbacks. Dropping it takes the callbacks off
/// the socket before they are freed, and closes it.
pub struct Socket {
    socket: WebSocket,
    shared: Rc<Shared>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut(CloseEvent)>,
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.socket.set_onmessage(None);
        self.socket.set_onclose(None);
        let _ = self.socket.close();
    }
}

/// The event stream at the page's own address: `wss` on a page served over
/// HTTPS, `ws` otherwise.
pub fn url(_hub: &str) -> String {
    let location = web_sys::window().map(|window| window.location());
    let secure = location
        .as_ref()
        .and_then(|l| l.protocol().ok())
        .is_some_and(|protocol| protocol == "https:");
    let host = location.and_then(|l| l.host().ok()).unwrap_or_default();
    format!("{}://{host}/ws", if secure { "wss" } else { "ws" })
}

/// Opens the socket and waits for it to open, to close, or for
/// [`OPEN_TIMEOUT`].
pub async fn connect(_hub: &str, url: &str) -> Result<Socket, String> {
    let socket = WebSocket::new(url).map_err(|e| describe(&e))?;
    let shared = Rc::new(Shared::default());
    let on_message = {
        let shared = shared.clone();
        Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            // Every message of the contract is text.
            if let Some(text) = event.data().as_string() {
                deliver(&shared, text);
            }
        })
    };
    let on_close = {
        let shared = shared.clone();
        Closure::<dyn FnMut(CloseEvent)>::new(move |event: CloseEvent| {
            shared.closed.replace(Some(close_reason(&event)));
        })
    };
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    let socket = Socket {
        socket,
        shared,
        _on_message: on_message,
        _on_close: on_close,
    };
    let until = web_time::Instant::now() + OPEN_TIMEOUT;
    loop {
        match socket.socket.ready_state() {
            WebSocket::OPEN => return Ok(socket),
            WebSocket::CLOSING | WebSocket::CLOSED => {
                let reason = socket.shared.closed.take();
                return Err(reason.unwrap_or_else(|| "the socket closed as it opened".into()));
            }
            _ => {}
        }
        if web_time::Instant::now() >= until {
            return Err(format!(
                "the socket did not open within {} s",
                OPEN_TIMEOUT.as_secs()
            ));
        }
        sleep(Duration::from_millis(10)).await;
    }
}

/// Passes a message to the app, or holds it until the app has been told
/// the socket opened.
fn deliver(shared: &Shared, text: String) {
    match shared.items.borrow().as_ref() {
        Some(items) => {
            if items.send(Item::Message(parse(&text))).is_err() {
                shared.app_gone.set(true);
            }
        }
        None => shared.early.borrow_mut().push(text),
    }
}

/// Why a socket closed, in words. A browser gives no reason for a
/// connection that failed, only the code 1006, and says more in its
/// console.
fn close_reason(event: &CloseEvent) -> String {
    match (event.code(), event.reason()) {
        (1000 | 1001, _) => "the hub closed the stream".into(),
        (1006, _) => "the connection failed or was lost (code 1006)".into(),
        (code, reason) if reason.is_empty() => format!("the socket closed with code {code}"),
        (code, reason) => format!("the socket closed with code {code}: {reason}"),
    }
}

pub fn send(socket: &mut Socket, text: String) -> Result<(), String> {
    socket
        .socket
        .send_with_str(&text)
        .map_err(|e| format!("cannot send the subscription: {}", describe(&e)))
}

/// Closes the socket, once the app has gone.
pub fn close(socket: &mut Socket) {
    let _ = socket.socket.close();
}

/// Passes on every message until the socket closes, carrying out the app's
/// commands meanwhile.
pub async fn follow(
    socket: &mut Socket,
    subscription: &mut Subscription,
    items: &Sender<Item>,
    commands: &Receiver<Command>,
) -> Ended {
    // The app has just been told the socket opened: what came before goes
    // first, and every message after goes straight to it.
    for text in socket.shared.early.take() {
        if items.send(Item::Message(parse(&text))).is_err() {
            return Ended::AppGone;
        }
    }
    socket.shared.items.replace(Some(items.clone()));
    loop {
        if let Err(ended) = take_commands(socket, subscription, commands) {
            return ended;
        }
        if socket.shared.app_gone.get() {
            return Ended::AppGone;
        }
        if let Some(reason) = socket.shared.closed.take() {
            return Ended::Lost(reason);
        }
        sleep(POLL).await;
    }
}
