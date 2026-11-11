//! avoid-words-reminder (PreToolUse).
//!
//! In text written for other people, notes the words a table says not to
//! use, with what to say instead and the table's note. It looks at the same calls and the same
//! parts of them as wago-verb-reminder: text typed into a page or sent as a
//! message, a README or an HTML file of a public repository written with
//! Write or Edit, and the title and body of a PR or a release that gh
//! creates or edits in a public repository. As there, a call that may send
//! or publish is stopped once per session and text (the same call sent
//! again goes through), and the others only get the note. Which were
//! stopped is kept as empty files in `$AVOID_WORDS_STATE_DIR`, else
//! `<temp>/claude-avoid-words`.
//!
//! The table is `$AVOID_WORDS_FILE`, else `avoid-words.tsv` in the docs
//! folder (`YHOOKY_DOCS_DIR`). Without it, or when it cannot be read,
//! nothing is noted. A line starting with `#` is a comment; each other
//! line is, separated by tabs:
//!
//! ```text
//! word<TAB>what to say instead<TAB>note<TAB>exceptions
//! ```
//!
//! Only the first column is required. The note is free text, shown in
//! parentheses after the word (a reason or a source, say). Exceptions are
//! words, separated by `|`, that hold the word and may stay: 時間帯 (time
//! slot), which holds 帯 (band). A word of ASCII
//! letters and digits matches ignoring ASCII case, and not inside a longer
//! ASCII word.

use std::path::{Path, PathBuf};

use super::wago::{Outgoing, outgoing_text};
use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::text::windows_path;

const ID: &str = "avoid-words-reminder";
const TABLE: &str = "avoid-words.tsv";
/// At most this many words are listed.
const LIMIT: usize = 10;

/// One line of the table.
#[derive(Debug, PartialEq)]
pub struct Entry {
    pub word: String,
    pub instead: String,
    /// Free text shown after the word.
    pub note: String,
    pub exceptions: Vec<String>,
}

/// The entries of a table's text. Comments, blank lines and lines with an
/// empty word are left out.
pub fn parse(text: &str) -> Vec<Entry> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim_start().starts_with('#') {
            continue;
        }
        let mut cols = line.split('\t').map(str::trim);
        let word = cols.next().unwrap_or("");
        if word.is_empty() {
            continue;
        }
        let instead = cols.next().unwrap_or("").to_string();
        let note = cols.next().unwrap_or("").to_string();
        let exceptions = cols
            .next()
            .unwrap_or("")
            .split('|')
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(String::from)
            .collect();
        entries.push(Entry {
            word: word.to_string(),
            instead,
            note,
            exceptions,
        });
    }
    entries
}

/// Where `word` occurs in `text` (lowered the same way), as byte ranges.
/// An ASCII letter or digit at an end of the word must not run on into
/// another one.
fn places(text: &str, word: &str) -> Vec<(usize, usize)> {
    let alnum = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    let (first, last) = (word.chars().next(), word.chars().last());
    text.match_indices(word)
        .map(|(at, _)| (at, at + word.len()))
        .filter(|&(at, end)| {
            let before = text[..at].chars().next_back();
            let after = text[end..].chars().next();
            let runs_on_before = alnum(first) && alnum(before);
            let runs_on_after = alnum(last) && alnum(after);
            !runs_on_before && !runs_on_after
        })
        .collect()
}

/// The entries whose word occurs in `text` other than inside one of its
/// exceptions, each once, in table order.
pub fn find<'a>(text: &str, entries: &'a [Entry]) -> Vec<&'a Entry> {
    // ASCII lowering keeps every byte where it was, so ranges still match.
    let text = text.to_ascii_lowercase();
    entries
        .iter()
        .filter(|entry| {
            let word = entry.word.to_ascii_lowercase();
            let covered: Vec<(usize, usize)> = entry
                .exceptions
                .iter()
                .flat_map(|e| places(&text, &e.to_ascii_lowercase()))
                .collect();
            places(&text, &word)
                .iter()
                .any(|&(at, end)| !covered.iter().any(|&(a, b)| a <= at && end <= b))
        })
        .collect()
}

/// `~` and `~/...` as the home folder; other paths as Windows writes them.
fn expand(path: &str) -> PathBuf {
    let path = path.trim();
    if let Some(rest) = path.strip_prefix('~')
        && (rest.is_empty() || rest.starts_with(['/', '\\']))
        && let Some(home) = crate::home()
    {
        return home.join(rest.trim_start_matches(['/', '\\']));
    }
    PathBuf::from(windows_path(path))
}

/// The table's path: `$AVOID_WORDS_FILE`, else the one in the docs folder.
fn table_file() -> Option<PathBuf> {
    match std::env::var("AVOID_WORDS_FILE") {
        Ok(file) if !file.trim().is_empty() => Some(expand(&file)),
        _ => {
            let dir = std::env::var("YHOOKY_DOCS_DIR").ok()?;
            (!dir.trim().is_empty()).then(|| expand(&dir).join(TABLE))
        }
    }
}

/// The table's entries; none when it is missing or cannot be read.
fn load(path: &Path) -> Vec<Entry> {
    match std::fs::read(path) {
        Ok(bytes) => parse(&String::from_utf8_lossy(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            crate::log(&format!("{ID}: {}: {e}", path.display()));
            Vec::new()
        }
    }
}

/// The message for the entries found.
fn message(found: &[&Entry], table: &Path) -> String {
    let lines: String = found
        .iter()
        .take(LIMIT)
        .map(|e| {
            let instead = if e.instead.is_empty() {
                String::new()
            } else {
                format!(" → {}", e.instead)
            };
            let note = if e.note.is_empty() {
                String::new()
            } else {
                format!("({})", e.note)
            };
            format!("\n- {}{instead}{note}", e.word)
        })
        .collect();
    let more = found.len().saturating_sub(LIMIT);
    let rest = if more > 0 {
        format!("\n- ほか {more} 語")
    } else {
        String::new()
    };
    format!(
        "外に出る文に、使わないと決めた語がある({})。代わりの言い方で書き直す。{lines}{rest}",
        table.display()
    )
}

/// The check with the table at `table`.
fn check(input: &Input, table: &Path) -> Vec<Finding> {
    if input.event() != "PreToolUse" {
        return Vec::new();
    }
    // The table first: finding the text may run `gh`.
    let entries = load(table);
    if entries.is_empty() {
        return Vec::new();
    }
    let Some(Outgoing { text, sends }) = outgoing_text(input) else {
        return Vec::new();
    };
    let found = find(&text, &entries);
    if found.is_empty() {
        return Vec::new();
    }
    let note = message(&found, table);
    let dir = crate::once::state_dir("AVOID_WORDS_STATE_DIR", "claude-avoid-words");
    let key = format!("{}\n{text}", input.text("session_id"));
    if sends && crate::once::first_time(&dir, &key) {
        return vec![Finding::new(
            ID,
            Kind::Deny,
            format!(
                "{note}\n送ったり公開したりした文は、後で直しても相手に届いたものが残るので、その前に一度止めた。このまま進めるなら、同じ操作を出し直せば通る。"
            ),
        )];
    }
    vec![Finding::new(ID, Kind::Context, note)]
}

pub fn run(input: &Input) -> Vec<Finding> {
    match table_file() {
        Some(table) => check(input, &table),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE_TEXT: &str = "\u{feff}# 使わない語\r\n# 列: 語<TAB>代わり<TAB>注記<TAB>除外\r\n帯\t画面の表記を使う\t指摘\t時間帯|携帯\r\n\r\n  # 字下げしたコメント\t\t\t\nPR\tプルリクエスト\n\t空の語\n断片\n";

    fn entries() -> Vec<Entry> {
        parse(TABLE_TEXT)
    }

    fn words(text: &str) -> Vec<String> {
        let entries = entries();
        find(text, &entries)
            .iter()
            .map(|e| e.word.clone())
            .collect()
    }

    #[test]
    fn parses_the_table() {
        let entries = entries();
        assert_eq!(entries.len(), 3, "{entries:?}");
        assert_eq!(
            entries[0],
            Entry {
                word: "帯".into(),
                instead: "画面の表記を使う".into(),
                note: "指摘".into(),
                exceptions: vec!["時間帯".into(), "携帯".into()],
            }
        );
        assert_eq!(entries[2].word, "断片");
        assert!(entries[2].instead.is_empty() && entries[2].exceptions.is_empty());
    }

    #[test]
    fn finds_words_outside_their_exceptions() {
        assert_eq!(words("上の帯に名前を出す。"), ["帯"]);
        assert!(words("時間帯と携帯の話。").is_empty(), "exceptions only");
        assert_eq!(words("時間帯の帯を消す。"), ["帯"], "one bare use");
        assert_eq!(words("断片と帯。"), ["帯", "断片"], "table order");
        assert!(words("コメントの語はない。字下げしたコメント。").is_empty());
    }

    #[test]
    fn matches_ascii_words_whole_and_ignoring_case() {
        assert_eq!(words("この pr を見る。"), ["PR"]);
        assert_eq!(words("PRを出す。"), ["PR"], "next to Japanese");
        assert!(words("PRINT と APR。").is_empty(), "inside longer words");
    }

    #[test]
    fn expands_the_home_folder() {
        let home = crate::home().unwrap();
        assert_eq!(expand("~/.claude/docs"), home.join(".claude/docs"));
        assert_eq!(expand("/c/x/t.tsv"), PathBuf::from("C:/x/t.tsv"));
        assert_eq!(expand("~x/t"), PathBuf::from("~x/t"));
    }

    #[test]
    fn notes_typing_and_stops_a_send_once() {
        let dir = tempfile::tempdir().unwrap();
        let table = dir.path().join(TABLE);
        std::fs::write(&table, TABLE_TEXT).unwrap();
        // SAFETY: only this test reads the variable.
        unsafe { std::env::set_var("AVOID_WORDS_STATE_DIR", dir.path().join("state")) };
        let call = |tool: &str, text: &str, table: &Path| {
            let raw = serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": tool,
                "session_id": "s",
                "tool_input": { "text": text, "value": text },
            });
            check(&Input::parse(&raw.to_string()).unwrap(), table)
                .iter()
                .map(|f| (matches!(f.kind, Kind::Deny), f.message.clone()))
                .collect::<Vec<_>>()
        };
        let typed = call(
            "mcp__claude-in-chrome__form_input",
            "上の帯を見る。",
            &table,
        );
        assert_eq!(typed.len(), 1);
        assert!(!typed[0].0, "typing only notes");
        assert!(
            typed[0].1.contains("帯 → 画面の表記を使う(指摘)"),
            "{}",
            typed[0].1
        );
        let send = "mcp__beeper__send_message";
        assert!(call(send, "上の帯を見る。", &table)[0].0, "first send");
        assert!(!call(send, "上の帯を見る。", &table)[0].0, "sent again");
        assert!(call(send, "時間帯を見る。", &table).is_empty(), "exception");
        let missing = dir.path().join("none.tsv");
        assert!(
            call(send, "上の帯を見る。", &missing).is_empty(),
            "no table"
        );
        assert!(
            call("mcp__claude-in-chrome__javascript_tool", "帯", &table).is_empty(),
            "not text for people"
        );
    }
}
