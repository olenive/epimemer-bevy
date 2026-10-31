//! The hub's HTTP in a browser: the page's fetch, awaited on the page's
//! event loop (`background.rs`).
//!
//! The page is served from the hub's own origin, or through `trunk serve`,
//! whose proxy passes `/api` on to the hub (`Trunk.toml`), so every address
//! is relative to the page and no read is cross-origin; the hub sends no
//! CORS headers (VIZ_CONTRACT.md §2). A fetch reads the whole body before it
//! gives it, so the size limit is checked against the length the answer
//! declares before the body is read, and against the text once it is.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortController, RequestInit, Response, Window};

use crate::browser::describe;

/// The hub gives a session ten seconds to answer and then returns `504`,
/// so a read that takes much longer than that has lost the hub.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Nothing to keep between reads: the browser keeps its own connections.
pub struct Client;

pub fn client() -> Client {
    Client
}

/// The hub, as an address: the page's own origin.
pub fn origin(_hub: &str) -> String {
    web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_else(|| "the page's origin".to_owned())
}

/// The address of `path` on the hub: on the page's own origin, so the path
/// alone.
pub fn url(_hub: &str, path: &str) -> String {
    path.to_owned()
}

/// `GET url?query`: the status and the body, refused past `limit` bytes,
/// or why there was no whole answer.
pub async fn get(
    _client: &Client,
    url: &str,
    query: &[(&str, &str)],
    limit: u64,
) -> Result<(u16, String), String> {
    let window = web_sys::window().ok_or("the page has no window")?;
    let controller = AbortController::new().map_err(|e| describe(&e))?;
    let deadline = Deadline::start(&window, &controller, READ_TIMEOUT)?;
    let init = RequestInit::new();
    init.set_method("GET");
    init.set_signal(Some(&controller.signal()));
    let failed = |error: JsValue| {
        if deadline.fired.get() {
            format!("no answer within {} s", READ_TIMEOUT.as_secs())
        } else {
            describe(&error)
        }
    };
    let response: Response =
        JsFuture::from(window.fetch_with_str_and_init(&with_query(url, query), &init))
            .await
            .map_err(failed)?
            .dyn_into()
            .map_err(|_| "the fetch gave something other than a response".to_owned())?;
    let declared = response
        .headers()
        .get("content-length")
        .ok()
        .flatten()
        .and_then(|length| length.trim().parse::<u64>().ok());
    if let Some(length) = declared
        && length > limit
    {
        return Err(too_large(length, limit));
    }
    let text = JsFuture::from(response.text().map_err(|e| describe(&e))?)
        .await
        .map_err(failed)?
        .as_string()
        .unwrap_or_default();
    if text.len() as u64 > limit {
        return Err(too_large(text.len() as u64, limit));
    }
    Ok((response.status(), text))
}

fn too_large(length: u64, limit: u64) -> String {
    format!("the answer is {length} bytes, more than the {limit} this read allows")
}

/// `url` with its query, each name and value encoded.
fn with_query(url: &str, query: &[(&str, &str)]) -> String {
    let encode = |text: &str| String::from(js_sys::encode_uri_component(text));
    let pairs: Vec<String> = query
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect();
    if pairs.is_empty() {
        url.to_owned()
    } else {
        format!("{url}?{}", pairs.join("&"))
    }
}

/// Aborts a fetch that has taken longer than its time. Dropping it clears
/// the timer, so its callback is never called once it is gone.
struct Deadline {
    window: Window,
    handle: i32,
    fired: Rc<Cell<bool>>,
    _abort: Closure<dyn FnMut()>,
}

impl Deadline {
    fn start(
        window: &Window,
        controller: &AbortController,
        after: Duration,
    ) -> Result<Self, String> {
        let fired = Rc::new(Cell::new(false));
        let abort = {
            let (controller, fired) = (controller.clone(), fired.clone());
            Closure::<dyn FnMut()>::new(move || {
                fired.set(true);
                controller.abort();
            })
        };
        let millis = i32::try_from(after.as_millis()).unwrap_or(i32::MAX);
        let handle = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                abort.as_ref().unchecked_ref(),
                millis,
            )
            .map_err(|e| describe(&e))?;
        Ok(Deadline {
            window: window.clone(),
            handle,
            fired,
            _abort: abort,
        })
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        self.window.clear_timeout_with_handle(self.handle);
    }
}
