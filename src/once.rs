//! Stopping a call only the first time: which keys were seen is kept as
//! empty files named by the sha1 of the key, in a folder given by an
//! environment variable (for tests), else under the temporary folder.

use std::fs;
use std::path::PathBuf;

use crate::text::windows_path;

/// `$<var>` when set, else `<temp>/<name>`.
pub fn state_dir(var: &str, name: &str) -> PathBuf {
    match std::env::var(var) {
        Ok(dir) if !dir.is_empty() => PathBuf::from(windows_path(&dir)),
        _ => std::env::temp_dir().join(name),
    }
}

/// Whether `key` is seen for the first time in `dir`; remembers it. Without
/// a place to remember it, a stop would come back every time, so that
/// counts as seen.
pub fn first_time(dir: &PathBuf, key: &str) -> bool {
    let hash = sha1_smol::Sha1::from(key.as_bytes()).digest().to_string();
    let mark = dir.join(&hash[..16]);
    !mark.exists() && fs::create_dir_all(dir).is_ok() && fs::write(&mark, "").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().join("state");
        assert!(first_time(&dir, "s\na"));
        assert!(!first_time(&dir, "s\na"));
        assert!(first_time(&dir, "s\nb"));
    }
}
