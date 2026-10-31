//! The page the browser build runs in (`index.html`): the options in its
//! address, the line of text over the canvas that says what the start is
//! doing, and the words for an error the browser gives.
//!
//! A page has no command line, so its query string gives the same options:
//! `?graph=memory&preset=3d&time=claim` reads as `--graph=memory
//! --preset=3d --time=claim`, with the same defaults, and an option that
//! takes no value is given bare, `?hide`, or as `1` or `true`. Two kinds of
//! option are refused with the reason: `hub`, because the page reaches the
//! hub at its own address and the hub sends no CORS headers for any other
//! (VIZ_CONTRACT.md §2), and the native build's development options
//! (`main.rs`).

/// The selector of the canvas the app draws into.
#[cfg(target_arch = "wasm32")]
pub const CANVAS: &str = "#epimemer-bevy";

/// The id of the line of text over the canvas.
#[cfg(target_arch = "wasm32")]
const STATUS: &str = "status";

/// The options that take no value.
const FLAGS: [&str; 9] = [
    "all-edges",
    "fit-topic",
    "fps",
    "help",
    "hide",
    "log-events",
    "no-persist",
    "orbit",
    "recalculate",
];

/// A query string, with or without its `?`, as the arguments the command
/// line would give: `name=value` as `--name=value`, and a flag given bare,
/// empty or as `1`, `true`, `yes` or `on` as `--name`; a flag as `0`,
/// `false`, `no` or `off` is left out. Anything else is passed on for
/// `parse_args` to judge.
pub fn query_args(query: &str) -> Result<Vec<String>, String> {
    let query = query.strip_prefix('?').unwrap_or(query);
    let mut args = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let (name, value) = (decode(name)?, decode(value)?);
        if name == "hub" {
            return Err(
                "hub: the browser build reads the hub at the page's own address, since the hub sends no CORS headers; open the page from `trunk serve`, which passes /api and /ws on to the hub, or from the hub itself".into(),
            );
        }
        if FLAGS.contains(&name.as_str()) {
            match value.to_ascii_lowercase().as_str() {
                "" | "1" | "true" | "yes" | "on" => args.push(format!("--{name}")),
                "0" | "false" | "no" | "off" => {}
                _ => args.push(format!("--{name}={value}")),
            }
        } else {
            args.push(format!("--{name}={value}"));
        }
    }
    Ok(args)
}

/// A name or value of a query string, decoded: `+` as a space, and `%`
/// with two hex digits as that byte of UTF-8.
fn decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => decoded.push(b' '),
            b'%' => {
                let byte = bytes
                    .get(i + 1..i + 3)
                    .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
                    .and_then(|hex| std::str::from_utf8(hex).ok())
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                    .ok_or_else(|| format!("the address has a broken escape in {text:?}"))?;
                decoded.push(byte);
                i += 2;
            }
            byte => decoded.push(byte),
        }
        i += 1;
    }
    String::from_utf8(decoded)
        .map_err(|_| format!("the address has an escape in {text:?} that is not UTF-8"))
}

/// The page's query string, with its `?`.
#[cfg(target_arch = "wasm32")]
pub fn query() -> String {
    web_sys::window()
        .and_then(|window| window.location().search().ok())
        .unwrap_or_default()
}

/// The page's host and port, where the hub is reached.
#[cfg(target_arch = "wasm32")]
pub fn page_host() -> String {
    web_sys::window()
        .and_then(|window| window.location().host().ok())
        .unwrap_or_default()
}

/// Puts `text` in the line over the canvas.
#[cfg(target_arch = "wasm32")]
pub fn show_status(text: &str) {
    if let Some(element) = status() {
        element.set_text_content(Some(text));
    }
}

/// Puts `text` in the line over the canvas, marked as the reason the start
/// stopped.
#[cfg(target_arch = "wasm32")]
pub fn show_failure(text: &str) {
    if let Some(element) = status() {
        element.set_class_name("failed");
        element.set_text_content(Some(&format!("{text}\n\nReload the page to try again.")));
    }
}

/// Takes the line over the canvas away, once the app draws.
#[cfg(target_arch = "wasm32")]
pub fn hide_status() {
    if let Some(element) = status() {
        element.remove();
    }
}

#[cfg(target_arch = "wasm32")]
fn status() -> Option<web_sys::Element> {
    web_sys::window()?.document()?.get_element_by_id(STATUS)
}

/// An error the browser gives, in words: a JavaScript error's message, or
/// the value as text.
#[cfg(target_arch = "wasm32")]
pub fn describe(value: &wasm_bindgen::JsValue) -> String {
    use wasm_bindgen::JsCast;
    match value.dyn_ref::<js_sys::Error>() {
        Some(error) => String::from(error.message()),
        None => value.as_string().unwrap_or_else(|| format!("{value:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_reads_as_the_command_line_would() {
        assert_eq!(
            query_args("?graph=memory&preset=3d&time=claim").unwrap(),
            ["--graph=memory", "--preset=3d", "--time=claim"]
        );
        assert_eq!(query_args("").unwrap(), Vec::<String>::new());
        assert_eq!(query_args("?").unwrap(), Vec::<String>::new());
        assert_eq!(
            query_args("hide&all-edges=1&orbit=true&fit-topic=0&fps=off").unwrap(),
            ["--hide", "--all-edges", "--orbit"],
            "flags bare or on, and off left out"
        );
        assert_eq!(
            query_args("hide=maybe").unwrap(),
            ["--hide=maybe"],
            "left for parse_args to refuse"
        );
        assert_eq!(
            query_args("graph=").unwrap(),
            ["--graph="],
            "left for parse_args to say it needs a value"
        );
    }

    #[test]
    fn names_and_values_are_decoded() {
        assert_eq!(
            query_args("search=swifts+sleep&window=2026-10-14..2026-10-20&ui-state=topics%3Avis")
                .unwrap(),
            [
                "--search=swifts sleep",
                "--window=2026-10-14..2026-10-20",
                "--ui-state=topics:vis"
            ]
        );
        assert_eq!(
            query_args("focus=Caf%C3%A9%20society").unwrap(),
            ["--focus=Café society"]
        );
        assert!(
            query_args("focus=%E2%28")
                .unwrap_err()
                .contains("not UTF-8")
        );
        for broken in ["focus=%", "focus=%4", "focus=%zz", "focus=%+1"] {
            assert!(
                query_args(broken).unwrap_err().contains("broken escape"),
                "{broken}"
            );
        }
    }

    #[test]
    fn the_hub_is_refused_with_the_reason() {
        let error = query_args("graph=memory&hub=127.0.0.1:8765").unwrap_err();
        assert!(error.contains("no CORS headers"), "{error}");
    }
}
