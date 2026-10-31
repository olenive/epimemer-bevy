//! Saving the topic map on the client (DESIGN.md §7): the server never
//! learns about positions.
//!
//! A [`Store`] is where maps are kept, chosen at the start and passed as a
//! value: natively a directory with a small file per graph,
//! `<data directory>/<hub>/<graph>.json` (`persist/native.rs`), where the
//! data directory is the platform's, `~/Library/Application
//! Support/epimemer-bevy` on macOS and the XDG data directory on Linux; in
//! a browser the page's local storage under `epimemer-bevy/<hub>/<graph>`
//! (`persist/web.rs`); or nowhere, for `--no-persist`. A [`MapKey`] names
//! one map, by the hub it was read from and its graph, and the file holds
//! the [`TopicMap`] as JSON.
//!
//! A map that cannot be read, or was written in a shape this client does
//! not know, reads as an error, and the caller fits a new map in its place.

use crate::topic_map::{FORMAT, TopicMap};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
pub use native::{data_dir, file_of};

/// Where the maps are kept.
#[derive(Clone, Debug)]
pub enum Store {
    /// Nowhere: nothing is read and nothing is written.
    Off,
    /// A directory, natively.
    #[cfg(not(target_arch = "wasm32"))]
    Dir(std::path::PathBuf),
    /// The page's local storage.
    #[cfg(target_arch = "wasm32")]
    LocalStorage,
    /// A map of keys to texts in memory, standing in for local storage in
    /// the tests.
    #[cfg(test)]
    Memory(std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, String>>>),
}

/// One saved map: the hub, as `host:port`, and the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapKey {
    pub hub: String,
    pub graph: String,
}

/// The key a map is kept under in local storage.
#[cfg(any(test, target_arch = "wasm32"))]
pub fn storage_key(key: &MapKey) -> String {
    format!("epimemer-bevy/{}/{}", key.hub, key.graph)
}

/// Where a store keeps a map, in words for the log.
pub fn describe(store: &Store, key: &MapKey) -> String {
    match store {
        Store::Off => "nowhere, since --no-persist was given".to_owned(),
        #[cfg(not(target_arch = "wasm32"))]
        Store::Dir(dir) => file_of(dir, key).display().to_string(),
        #[cfg(target_arch = "wasm32")]
        Store::LocalStorage => format!("the page's local storage, as {:?}", storage_key(key)),
        #[cfg(test)]
        Store::Memory(_) => format!("memory, as {:?}", storage_key(key)),
    }
}

/// The text kept under `key`, or `None` where there is none.
pub fn read_text(store: &Store, key: &MapKey) -> Result<Option<String>, String> {
    match store {
        Store::Off => Ok(None),
        #[cfg(not(target_arch = "wasm32"))]
        Store::Dir(dir) => native::read(&file_of(dir, key)),
        #[cfg(target_arch = "wasm32")]
        Store::LocalStorage => web::read(&storage_key(key)),
        #[cfg(test)]
        Store::Memory(memory) => Ok(memory
            .lock()
            .map_err(|_| "the store is poisoned".to_owned())?
            .get(&storage_key(key))
            .cloned()),
    }
}

/// Keeps `text` under `key`, replacing what was there.
pub fn write_text(store: &Store, key: &MapKey, text: &str) -> Result<(), String> {
    match store {
        Store::Off => Ok(()),
        #[cfg(not(target_arch = "wasm32"))]
        Store::Dir(dir) => native::write(&file_of(dir, key), text),
        #[cfg(target_arch = "wasm32")]
        Store::LocalStorage => web::write(&storage_key(key), text),
        #[cfg(test)]
        Store::Memory(memory) => {
            memory
                .lock()
                .map_err(|_| "the store is poisoned".to_owned())?
                .insert(storage_key(key), text.to_owned());
            Ok(())
        }
    }
}

/// The map saved under `key`, or `None` where none is. A map of another
/// graph or another shape is an error, so it is fitted again rather than
/// drawn wrong.
pub fn read_map(store: &Store, key: &MapKey) -> Result<Option<TopicMap>, String> {
    let Some(text) = read_text(store, key)? else {
        return Ok(None);
    };
    let map: TopicMap = serde_json::from_str(&text)
        .map_err(|e| format!("the saved topic map does not read: {e}"))?;
    if map.format != FORMAT {
        return Err(format!(
            "the saved topic map is in format {}, and this client reads format {FORMAT}",
            map.format
        ));
    }
    if map.graph != key.graph {
        return Err(format!(
            "the saved topic map is of graph {:?}, not {:?}",
            map.graph, key.graph
        ));
    }
    Ok(Some(map))
}

/// Saves `map` under `key`.
pub fn write_map(store: &Store, key: &MapKey, map: &TopicMap) -> Result<(), String> {
    let text = serde_json::to_string(map).map_err(|e| format!("the map does not write: {e}"))?;
    write_text(store, key, &text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topic_map::{TopicInput, fit_map, tests::grouped};

    fn map(graph: &str) -> TopicMap {
        let topics = grouped(12, 6);
        let inputs: Vec<TopicInput> = topics
            .iter()
            .map(|(id, v)| TopicInput {
                id,
                vector: v,
                claims: 2,
                parent: None,
            })
            .collect();
        fit_map(graph, "all-MiniLM-L6-v2", &inputs, 40.0)
            .unwrap()
            .map
    }

    fn key(graph: &str) -> MapKey {
        MapKey {
            hub: "127.0.0.1:8765".into(),
            graph: graph.into(),
        }
    }

    fn round_trip(store: &Store) {
        let saved = map("memory");
        assert_eq!(read_map(store, &key("memory")), Ok(None), "nothing yet");
        write_map(store, &key("memory"), &saved).unwrap();
        assert_eq!(read_map(store, &key("memory")), Ok(Some(saved.clone())));
        assert_eq!(read_map(store, &key("games")), Ok(None), "keyed by graph");
        let other_hub = MapKey {
            hub: "127.0.0.1:9000".into(),
            ..key("memory")
        };
        assert_eq!(read_map(store, &other_hub), Ok(None), "and by hub");
        let mut replaced = saved.clone();
        replaced.topics.insert("t-new".into(), [1.0, 2.0]);
        write_map(store, &key("memory"), &replaced).unwrap();
        assert_eq!(read_map(store, &key("memory")), Ok(Some(replaced)));
        write_text(store, &key("broken"), "{not json").unwrap();
        assert!(
            read_map(store, &key("broken"))
                .unwrap_err()
                .contains("does not read")
        );
        write_map(store, &key("elsewhere"), &saved).unwrap();
        assert!(
            read_map(store, &key("elsewhere"))
                .unwrap_err()
                .contains("of graph")
        );
    }

    #[test]
    fn a_map_round_trips_through_local_storage_as_the_memory_store_stands_in_for_it() {
        let store = Store::Memory(Default::default());
        round_trip(&store);
        let Store::Memory(memory) = &store else {
            unreachable!()
        };
        assert!(
            memory
                .lock()
                .unwrap()
                .contains_key("epimemer-bevy/127.0.0.1:8765/memory")
        );
    }

    #[test]
    fn a_map_round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!(
            "epimemer-bevy-test-{}-{}",
            std::process::id(),
            crate::layout::fnv1a(b"a_map_round_trips_through_a_file")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::Dir(dir.clone());
        round_trip(&store);
        let file = file_of(&dir, &key("memory"));
        assert!(file.is_file(), "{}", file.display());
        assert!(file.starts_with(dir.join("127.0.0.1%3A8765")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_store_keeps_nothing() {
        write_map(&Store::Off, &key("memory"), &map("memory")).unwrap();
        assert_eq!(read_map(&Store::Off, &key("memory")), Ok(None));
    }
}
