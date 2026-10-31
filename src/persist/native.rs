//! The topic maps natively: a small file per graph under the platform's
//! data directory, written whole to a file beside it and then renamed over
//! the old, so that a map is never half written.

use std::path::{Path, PathBuf};

use super::MapKey;

/// The directory under the data directory that holds this client's maps.
const APP: &str = "epimemer-bevy";

/// The platform's data directory for this client, from the environment
/// lookup `var`: on macOS `$HOME/Library/Application Support/epimemer-bevy`,
/// on Windows `%APPDATA%\epimemer-bevy`, and elsewhere
/// `$XDG_DATA_HOME/epimemer-bevy`, else `$HOME/.local/share/epimemer-bevy`.
/// `None` where the environment names no home.
pub fn data_dir(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |name: &str| {
        var(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(target_os = "macos") {
        set("HOME").map(|home| home.join("Library").join("Application Support"))
    } else if cfg!(windows) {
        set("APPDATA")
    } else {
        set("XDG_DATA_HOME").or_else(|| set("HOME").map(|home| home.join(".local").join("share")))
    };
    base.map(|base| base.join(APP))
}

/// The file of one map in `dir`: `<hub>/<graph>.json`, each name with every
/// byte other than a letter, a digit, `.`, `-` and `_` written as `%` and
/// two hex digits, so that no hub or graph name reaches outside its
/// directory or is refused by a file system.
pub fn file_of(dir: &Path, key: &MapKey) -> PathBuf {
    dir.join(encode(&key.hub))
        .join(format!("{}.json", encode(&key.graph)))
}

fn encode(name: &str) -> String {
    let encoded: String = name
        .bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => (byte as char).to_string(),
            b'.' if name.chars().any(|c| c != '.') => ".".to_owned(),
            _ => format!("%{byte:02X}"),
        })
        .collect();
    if encoded.is_empty() {
        "%".to_owned()
    } else {
        encoded
    }
}

pub fn read(file: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", file.display())),
    }
}

pub fn write(file: &Path, text: &str) -> Result<(), String> {
    let dir = file
        .parent()
        .ok_or_else(|| format!("{} has no directory", file.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let partial = file.with_extension("json.partial");
    std::fs::write(&partial, text)
        .map_err(|e| format!("cannot write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, file).map_err(|e| format!("cannot write {}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_directory_is_the_platforms() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let home = data_dir(env(&[("HOME", "/Users/someone")]));
        if cfg!(target_os = "macos") {
            assert_eq!(
                home,
                Some(PathBuf::from(
                    "/Users/someone/Library/Application Support/epimemer-bevy"
                ))
            );
        } else if !cfg!(windows) {
            assert_eq!(
                home,
                Some(PathBuf::from("/Users/someone/.local/share/epimemer-bevy"))
            );
            assert_eq!(
                data_dir(env(&[("HOME", "/h"), ("XDG_DATA_HOME", "/x")])),
                Some(PathBuf::from("/x/epimemer-bevy"))
            );
        }
        assert_eq!(data_dir(env(&[("HOME", "")])), None);
        assert_eq!(data_dir(env(&[])), None);
    }

    #[test]
    fn a_name_cannot_reach_outside_its_directory() {
        let dir = Path::new("/maps");
        let file = |hub: &str, graph: &str| {
            file_of(
                dir,
                &MapKey {
                    hub: hub.into(),
                    graph: graph.into(),
                },
            )
        };
        assert_eq!(
            file("127.0.0.1:8765", "memory"),
            Path::new("/maps/127.0.0.1%3A8765/memory.json")
        );
        assert_eq!(
            file("h", "../../etc/passwd"),
            Path::new("/maps/h/..%2F..%2Fetc%2Fpasswd.json")
        );
        assert_eq!(file("..", ".."), Path::new("/maps/%2E%2E/%2E%2E.json"));
        assert_eq!(file("h", ""), Path::new("/maps/h/%.json"));
        assert_eq!(
            file("h", "dev history"),
            Path::new("/maps/h/dev%20history.json")
        );
    }
}
