//! `<state>/shown.toml`: the notices shown once, so they are not shown again. A notice is kept as
//! an opaque id ([`id`]: a hash of what it was about), never as a name or a path:
//!
//! ```toml
//! ids = ["9c1f0e2a4b7d3e61"]
//! ```
//!
//! A file that is not there has none. A file that is there but cannot be read or parsed is never
//! written over ([`add`] refuses): unknown is not "nothing shown". Entries this version does not
//! read are kept as they are.

use std::collections::BTreeSet;
use std::path::Path;

/// The file's name, in the state directory.
pub const FILE: &str = "shown.toml";

/// The file exists but could not be read or parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable;

/// The id of a notice about `parts` (in order): a 64-bit FNV-1a hash of them, in hex. The same
/// parts give the same id in every version and on every platform.
pub fn id(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        // A separator no part holds, so ("ab", "c") is not ("a", "bc").
        for b in p.bytes().chain(std::iter::once(0)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// The ids in the file: none when there is none.
pub fn load(path: &Path) -> Result<BTreeSet<String>, Unreadable> {
    Ok(read(path)?.map(|d| ids(&d)).unwrap_or_default())
}

/// Add `id` to the file atomically, with what the file has now. A file that cannot be read or
/// parsed is not written over (an `InvalidData` error), nor is an `ids` that is not a list.
pub fn add(path: &Path, id: &str) -> std::io::Result<()> {
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "shown file unreadable");
    let mut doc = read(path).map_err(|_| invalid())?.unwrap_or_default();
    if doc.get("ids").is_some_and(|i| i.as_array().is_none()) {
        return Err(invalid());
    }
    let mut all = ids(&doc);
    if !all.insert(id.to_string()) {
        return Ok(());
    }
    doc["ids"] = toml_edit::value(all.iter().map(String::as_str).collect::<toml_edit::Array>());
    crate::fsutil::atomic_write(path, doc.to_string().as_bytes())
}

fn ids(doc: &toml_edit::DocumentMut) -> BTreeSet<String> {
    let a = doc.get("ids").and_then(toml_edit::Item::as_array);
    a.into_iter().flat_map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string))).collect()
}

/// The file as a document; `None` when it is not there.
fn read(path: &Path) -> Result<Option<toml_edit::DocumentMut>, Unreadable> {
    match std::fs::read_to_string(path) {
        Ok(text) => text.parse::<toml_edit::DocumentMut>().map(Some).map_err(|_| Unreadable),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Unreadable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("datarig-shown-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_id_is_stable_and_keeps_its_parts_apart() {
        assert_eq!(id(&[]), "cbf29ce484222325");
        assert_eq!(id(&["a"]), id(&["a"]));
        assert_ne!(id(&["ab", "c"]), id(&["a", "bc"]));
        assert_eq!(id(&["theme", "mine"]).len(), 16);
    }

    #[test]
    fn ids_are_added_once_and_the_rest_of_the_file_is_kept() {
        let d = dir("add");
        let p = d.join(FILE);
        assert_eq!(load(&p), Ok(BTreeSet::new()), "no file: none");
        add(&p, "b").unwrap();
        add(&p, "a").unwrap();
        add(&p, "a").unwrap();
        assert_eq!(load(&p).unwrap().into_iter().collect::<Vec<_>>(), ["a", "b"]);
        std::fs::write(&p, "later = 1\nids = [\"a\"]\n").unwrap();
        add(&p, "c").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("later = 1"), "{text}");
        assert_eq!(load(&p).unwrap().into_iter().collect::<Vec<_>>(), ["a", "c"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_never_written_over() {
        let d = dir("broken");
        let p = d.join(FILE);
        std::fs::write(&p, "ids = [").unwrap();
        assert_eq!(load(&p), Err(Unreadable));
        assert_eq!(add(&p, "a").unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "ids = [");
        std::fs::write(&p, "ids = \"a\"\n").unwrap();
        assert_eq!(add(&p, "b").unwrap_err().kind(), std::io::ErrorKind::InvalidData, "not a list");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "ids = \"a\"\n");
        let _ = std::fs::remove_dir_all(&d);
    }
}
