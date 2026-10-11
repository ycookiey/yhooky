//! Matching the way the shell hooks this replaces matched: `grep` looks at
//! one line at a time, so `^` and `$` hold at every line and a negated class
//! never crosses a line. Each line is matched on its own here too.

use regex::Regex;

/// The lines of `text`, as `grep` reads them: split on `\n`, without the
/// `\r` that Windows tools leave at a line's end.
pub fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

/// Whether any line of `text` matches, as `grep -q` would.
pub fn any_line(re: &Regex, text: &str) -> bool {
    lines(text).any(|line| re.is_match(line))
}

/// Every match on every line, in order, as `grep -o` would print them.
pub fn matches<'a>(re: &Regex, text: &'a str) -> Vec<&'a str> {
    lines(text)
        .flat_map(|line| re.find_iter(line).map(|m| m.as_str()))
        .collect()
}

/// The number of lines `grep -c ''` counts: none for empty text, and a last
/// line without its `\n` still counts.
pub fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let newlines = text.matches('\n').count();
    if text.ends_with('\n') {
        newlines
    } else {
        newlines + 1
    }
}

/// A Git Bash path (`/c/x`) as a Windows one (`C:/x`); other paths as given.
pub fn windows_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let is_drive = bytes.len() >= 2
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && (bytes.len() == 2 || bytes[2] == b'/');
    if is_drive {
        format!(
            "{}:{}",
            (bytes[1] as char).to_ascii_uppercase(),
            if bytes.len() == 2 { "/" } else { &path[2..] }
        )
    } else {
        path.to_string()
    }
}

/// A path in one form for comparing: `/` between parts, `C:` for the drive,
/// lower case, no `/` at the end.
pub fn comparable_path(path: &str) -> String {
    let path = windows_path(&path.replace('\\', "/"));
    path.trim_end_matches('/').to_lowercase()
}

/// `text` without one pair of `quote` around it, as `${x%\"}` and `${x#\"}`
/// take off each end on its own.
pub fn strip_quote(text: &str, quote: char) -> &str {
    let text = text.strip_suffix(quote).unwrap_or(text);
    text.strip_prefix(quote).unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_like_grep() {
        assert_eq!(line_count(""), 0);
        assert_eq!(line_count("a"), 1);
        assert_eq!(line_count("a\n"), 1);
        assert_eq!(line_count("a\nb"), 2);
    }

    #[test]
    fn anchors_hold_on_each_line() {
        let re = Regex::new("^b$").unwrap();
        assert!(any_line(&re, "a\nb\r\nc"));
        assert!(!re.is_match("a\nb\nc"));
    }

    #[test]
    fn converts_git_bash_paths() {
        assert_eq!(windows_path("/c/Main/x"), "C:/Main/x");
        assert_eq!(windows_path("/c"), "C:/");
        assert_eq!(windows_path("/tmp/x"), "/tmp/x");
        assert_eq!(comparable_path("C:\\Main\\X\\"), "c:/main/x");
        assert_eq!(comparable_path("/c/Main/X"), "c:/main/x");
    }
}
