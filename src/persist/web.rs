//! The topic maps in a browser: the page's local storage, one item per
//! graph. Local storage holds a few megabytes for a page and a map is tens
//! of kilobytes.

use crate::browser::describe;

fn storage() -> Result<web_sys::Storage, String> {
    web_sys::window()
        .ok_or("the page has no window")?
        .local_storage()
        .map_err(|e| describe(&e))?
        .ok_or_else(|| "the page has no local storage".to_owned())
}

pub fn read(key: &str) -> Result<Option<String>, String> {
    storage()?.get_item(key).map_err(|e| describe(&e))
}

pub fn write(key: &str, text: &str) -> Result<(), String> {
    storage()?.set_item(key, text).map_err(|e| {
        format!(
            "cannot keep the topic map in local storage: {}",
            describe(&e)
        )
    })
}
