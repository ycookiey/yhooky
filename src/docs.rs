//! Where a check's message points for details. The documents live in a
//! folder each user keeps, so the folder comes from `YHOOKY_DOCS_DIR`;
//! without it a message leaves the pointer out rather than name a path the
//! reader may not have.

/// `<dir>/<file>` in the docs folder, or `None` when no folder is set.
pub fn path(file: &str) -> Option<String> {
    let dir = std::env::var("YHOOKY_DOCS_DIR").ok()?;
    let dir = dir.trim().trim_end_matches(['/', '\\']);
    if dir.is_empty() {
        return None;
    }
    Some(format!("{dir}/{file}"))
}

/// `<before><dir>/<file><after>`, or nothing when no docs folder is set.
pub fn pointer(before: &str, file: &str, after: &str) -> String {
    path(file)
        .map(|path| format!("{before}{path}{after}"))
        .unwrap_or_default()
}
