//! The event stream natively: a blocking tungstenite socket over a TCP
//! connection, read on the stream's own thread (`background.rs`). A short
//! read timeout lets the thread take the app's commands between messages.

use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use tungstenite::{Message as Frame, WebSocket};

use super::{Command, Ended, Item, Subscription, parse, take_commands};

/// How long a read waits for a message before the thread looks for a
/// command from the app.
const POLL: Duration = Duration::from_millis(100);
/// A hub that is running answers a connection at once on loopback.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// The longest the WebSocket handshake may take.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub type Socket = WebSocket<TcpStream>;

/// The event stream of the hub at `host:port`.
pub fn url(hub: &str) -> String {
    format!("ws://{hub}/ws")
}

/// Opens the socket: a TCP connection with a timeout, then the WebSocket
/// handshake, then a short read timeout so that the thread can look for
/// commands between messages.
pub async fn connect(hub: &str, url: &str) -> Result<Socket, String> {
    let addresses: Vec<_> = hub
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {hub}: {e}"))?
        .collect();
    let mut failure = format!("{hub} resolves to no address");
    let tcp = addresses
        .iter()
        .find_map(
            |address| match TcpStream::connect_timeout(address, CONNECT_TIMEOUT) {
                Ok(tcp) => Some(tcp),
                Err(e) => {
                    failure = e.to_string();
                    None
                }
            },
        )
        .ok_or(failure)?;
    let configure = |tcp: &TcpStream, timeout: Duration| {
        tcp.set_read_timeout(Some(timeout))
            .and_then(|()| tcp.set_write_timeout(Some(HANDSHAKE_TIMEOUT)))
            .map_err(|e| format!("cannot set the socket's timeouts: {e}"))
    };
    let _ = tcp.set_nodelay(true);
    configure(&tcp, HANDSHAKE_TIMEOUT)?;
    let (socket, _) = tungstenite::client(url, tcp)
        .map_err(|e| format!("the WebSocket handshake failed: {e}"))?;
    configure(socket.get_ref(), POLL)?;
    Ok(socket)
}

pub fn send(socket: &mut Socket, text: String) -> Result<(), String> {
    socket
        .send(Frame::text(text))
        .map_err(|e| format!("cannot send the subscription: {e}"))
}

/// Closes the socket, once the app has gone.
pub fn close(socket: &mut Socket) {
    let _ = socket.close(None);
    let _ = socket.flush();
}

/// Reads the socket until it drops, passing on every text message and
/// carrying out the app's commands between reads.
pub async fn follow(
    socket: &mut Socket,
    subscription: &mut Subscription,
    items: &Sender<Item>,
    commands: &Receiver<Command>,
) -> Ended {
    loop {
        if let Err(ended) = take_commands(socket, subscription, commands) {
            return ended;
        }
        match socket.read() {
            Ok(Frame::Text(text)) => {
                if items.send(Item::Message(parse(text.as_str()))).is_err() {
                    return Ended::AppGone;
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ended::Lost("the hub closed the stream".into());
            }
            Err(error) => return Ended::Lost(error.to_string()),
        }
    }
}
