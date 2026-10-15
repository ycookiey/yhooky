//! wago-verb-reminder (PreToolUse).
//!
//! In Japanese written for other people, notes the verbs that are not
//! Sino-Japanese compounds (和語の動詞: 要る・確かめる・足す), so that saying
//! them with a compound (必要・確認・追加) can be weighed. Replacing them is
//! not asked for. It looks at:
//! - text typed into a page or sent as a message (form_input and computer
//!   of claude-in-chrome, beeper's send_message), and
//! - text written with Write or Edit into a README or an HTML file of a
//!   public repository (src/public_repo.rs). Documents for agents (docs,
//!   CLAUDE.md, skill definitions) are left out.
//!
//! The text is split into words with UniDic (lindera); a word counts when
//! its part of speech is 動詞 and its word type (語種) is 和, unless its
//! lexeme is a helper that grammar asks for (為る in 確認する, 有る, 居る,
//! 置く, 因る in によって). The dictionary is read only when a text gets
//! that far: from `$YHOOKY_UNIDIC_DIR`, else the `unidic` folder beside the
//! executable (built by examples/build_unidic.rs). Without it nothing is
//! said.
//!
//! A note reaches Claude with the tool's result, after a message has gone
//! out, so the tools that may send (a message, a click or a key in a page)
//! are stopped once per session and text instead; the same call sent again
//! goes through. Which were stopped is kept as empty files in
//! `$WAGO_GUARD_STATE_DIR`, else `<temp>/claude-wago-verb`. Typing into a
//! field (form_input) and writing a file can still be revised, so there it
//! only notes. A script run in a page (javascript_tool) is code, mostly
//! reading the page, and its Japanese strings are search words rather than
//! prose (くらうど was split into 食らう and stopped, 2026-10-16), so it is
//! left out.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::text::windows_path;

const ID: &str = "wago-verb-reminder";

/// Tools that put text into a page or a message.
static TYPING_TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(mcp__claude-in-chrome__(form_input|computer)|mcp__beeper__send_message)$")
        .unwrap()
});
/// Of those, the ones that may send what they type.
static SENDING_TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(mcp__claude-in-chrome__computer|mcp__beeper__send_message)$").unwrap()
});
const TYPING_FIELDS: [&str; 2] = ["tool_input.value", "tool_input.text"];

/// Lexemes left out: helpers that grammar asks for, not word choices.
/// 成る・出来る・言う are counted.
const HELPERS: [&str; 5] = ["為る", "有る", "居る", "置く", "因る"];

/// At most this many words are listed.
const LIMIT: usize = 15;

static JAPANESE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{Hiragana}\p{Han}]").unwrap());
static MD_FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```.*?(```|$)").unwrap());
static MD_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`\n]*`").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").unwrap());
static HTML_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<!--.*?-->|<script\b.*?</script>|<style\b.*?</style>").unwrap()
});
static HTML_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());

/// One word as UniDic describes it.
pub struct Word<'a> {
    pub pos: &'a str,
    pub lexeme: &'a str,
    pub base: &'a str,
    pub word_type: &'a str,
}

/// The base forms of the words to note, each once, in order.
pub fn pick<'a>(words: impl IntoIterator<Item = Word<'a>>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for word in words {
        if word.pos != "動詞" || word.word_type != "和" || HELPERS.contains(&word.lexeme) {
            continue;
        }
        let shown = if word.base.is_empty() || word.base == "*" {
            word.lexeme
        } else {
            word.base
        };
        if !found.iter().any(|f| f == shown) {
            found.push(shown.to_string());
        }
    }
    found
}

/// The prose of a file's text: code, URLs and markup left out.
fn prose(path: &str, text: &str) -> String {
    let lower = path.to_lowercase();
    if lower.ends_with(".html") || lower.ends_with(".htm") {
        let text = HTML_BLOCK.replace_all(text, " ");
        HTML_TAG.replace_all(&text, " ").into_owned()
    } else {
        let text = MD_FENCE.replace_all(text, " ");
        let text = MD_CODE.replace_all(&text, " ");
        URL.replace_all(&text, " ").into_owned()
    }
}

/// Whether a file is a document for people: a README, or an HTML page.
fn for_people(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    name.starts_with("readme") || name.ends_with(".html") || name.ends_with(".htm")
}

fn dictionary_dir() -> Option<PathBuf> {
    match std::env::var("YHOOKY_UNIDIC_DIR") {
        Ok(dir) if !dir.is_empty() => Some(PathBuf::from(windows_path(&dir))),
        _ => Some(std::env::current_exe().ok()?.parent()?.join("unidic")),
    }
    .filter(|dir| dir.join("metadata.json").is_file())
}

/// The words of `text` to note, or None without a dictionary.
fn analyze(text: &str) -> Option<Vec<String>> {
    use lindera::dictionary::load_dictionary_with_options;
    use lindera::mode::Mode;
    use lindera::segmenter::Segmenter;

    let Some(dir) = dictionary_dir() else {
        crate::log("wago-verb-reminder: no dictionary (YHOOKY_UNIDIC_DIR or <exe dir>/unidic)");
        return None;
    };
    let dictionary = load_dictionary_with_options(&dir.to_string_lossy(), true)
        .map_err(|e| crate::log(&format!("wago-verb-reminder: {e}")))
        .ok()?;
    let segmenter = Segmenter::new(Mode::Normal, dictionary, None);
    let mut tokens = segmenter
        .segment(Cow::Borrowed(text))
        .map_err(|e| crate::log(&format!("wago-verb-reminder: {e}")))
        .ok()?;
    // details: part of speech, its subcategory, lexeme, base spelling, word type.
    let details: Vec<Vec<String>> = tokens
        .iter_mut()
        .map(|t| t.details().iter().map(|d| d.to_string()).collect())
        .collect();
    fn field(d: &[String], i: usize) -> &str {
        d.get(i).map(String::as_str).unwrap_or("")
    }
    Some(pick(details.iter().map(|d| Word {
        pos: field(d, 0),
        lexeme: field(d, 2),
        base: field(d, 3),
        word_type: field(d, 4),
    })))
}

/// The text to look at, if this call writes Japanese for other people.
fn outgoing_text(input: &Input) -> Option<String> {
    let tool = input.tool();
    if TYPING_TOOL.is_match(&tool) {
        let fields: Vec<String> = TYPING_FIELDS.iter().map(|f| f.to_string()).collect();
        return Some(input.first_text(&fields));
    }
    if tool != "Write" && tool != "Edit" {
        return None;
    }
    let path = input.text("tool_input.file_path");
    if !for_people(&path) {
        return None;
    }
    let text = match tool.as_str() {
        "Write" => input.text("tool_input.content"),
        _ => input.text("tool_input.new_string"),
    };
    if !JAPANESE.is_match(&text) {
        return None;
    }
    // Looked up only now: it may run `gh`.
    if !crate::public_repo::is_public(Path::new(&windows_path(&path.replace('\\', "/")))) {
        return None;
    }
    Some(prose(&path, &text))
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" {
        return Vec::new();
    }
    let Some(text) = outgoing_text(input) else {
        return Vec::new();
    };
    if !JAPANESE.is_match(&text) {
        return Vec::new();
    }
    let Some(mut words) = analyze(&text) else {
        return Vec::new();
    };
    if words.is_empty() {
        return Vec::new();
    }
    let more = words.len().saturating_sub(LIMIT);
    words.truncate(LIMIT);
    let rest = if more > 0 {
        format!(" ほか {more} 語")
    } else {
        String::new()
    };
    let note = format!(
        "外に出る日本語の文に、熟語でない動詞(和語)がある: {}{rest}。熟語で言うほうが通るかを考え直す。置き換えは任意。\
         規則1(具体的な語を選ぶ)とは別の軸(~/.claude/docs/external-japanese.md の規則19)。",
        words.join("・")
    );
    let stop = SENDING_TOOL.is_match(&input.tool()) && first_time(&input.text("session_id"), &text);
    let finding = if stop {
        Finding::new(
            ID,
            Kind::Deny,
            format!(
                "{note}送ると取り消せないので、送る前に一度止めた。このまま送るなら、同じ操作を出し直せば通る。"
            ),
        )
    } else {
        Finding::new(ID, Kind::Context, note)
    };
    vec![finding]
}

/// Whether this session sends `text` for the first time; remembers it.
fn first_time(session: &str, text: &str) -> bool {
    let dir = crate::once::state_dir("WAGO_GUARD_STATE_DIR", "claude-wago-verb");
    crate::once::first_time(&dir, &format!("{session}\n{text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word<'a>(pos: &'a str, lexeme: &'a str, base: &'a str, word_type: &'a str) -> Word<'a> {
        Word {
            pos,
            lexeme,
            base,
            word_type,
        }
    }

    #[test]
    fn picks_native_verbs_once() {
        let words = vec![
            word("動詞", "要る", "要る", "和"),
            word("名詞", "確認", "確認", "漢"),
            word("動詞", "為る", "為る", "和"),
            word("動詞", "確かめる", "確かめる", "和"),
            word("動詞", "要る", "要る", "和"),
            word("動詞", "因る", "因る", "和"),
            word("動詞", "見付かる", "見つかる", "和"),
            word("動詞", "ドライブる", "*", "混"),
        ];
        assert_eq!(pick(words), vec!["要る", "確かめる", "見つかる"]);
    }

    #[test]
    fn keeps_prose_only() {
        let md =
            "説明を足す。\n```sh\nls を要る\n```\n`確かめる` と https://example.com/足す を見る";
        let text = prose("README.md", md);
        assert!(text.contains("説明を足す"));
        assert!(!text.contains("要る") && !text.contains("確かめる") && !text.contains("example"));
        let html = "<p>ボタンを押す</p><script>const a = '書く';</script><!-- 消す -->";
        let text = prose("index.html", html);
        assert!(text.contains("ボタンを押す"));
        assert!(!text.contains("書く") && !text.contains("消す"));
    }

    #[test]
    fn chooses_documents_for_people() {
        for path in [
            "README.md",
            "docs/readme.ja.md",
            "C:/x/site/index.html",
            "a.HTM",
        ] {
            assert!(for_people(path), "{path}");
        }
        for path in [
            "CLAUDE.md",
            "claude/docs/git.md",
            "skills/x/SKILL.md",
            "src/main.rs",
        ] {
            assert!(!for_people(path), "{path}");
        }
    }

    #[test]
    fn tells_sending_tools_apart() {
        assert!(!TYPING_TOOL.is_match("mcp__claude-in-chrome__javascript_tool"));
        for tool in [
            "mcp__beeper__send_message",
            "mcp__claude-in-chrome__computer",
        ] {
            assert!(
                TYPING_TOOL.is_match(tool) && SENDING_TOOL.is_match(tool),
                "{tool}"
            );
        }
        assert!(TYPING_TOOL.is_match("mcp__claude-in-chrome__form_input"));
        assert!(!SENDING_TOOL.is_match("mcp__claude-in-chrome__form_input"));
        assert!(!TYPING_TOOL.is_match("mcp__claude-in-chrome__navigate"));
    }

    /// Runs only with a dictionary, as below.
    #[test]
    fn stops_a_send_once() {
        if std::env::var("YHOOKY_UNIDIC_DIR").is_err() || dictionary_dir().is_none() {
            eprintln!("skipped: set YHOOKY_UNIDIC_DIR to run");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: only this test reads the variable.
        unsafe { std::env::set_var("WAGO_GUARD_STATE_DIR", dir.path()) };
        let call = |tool: &str| {
            let raw = serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": tool,
                "session_id": "s",
                "tool_input": { "text": "結果を確かめる。", "value": "結果を確かめる。" },
            });
            run(&Input::parse(&raw.to_string()).unwrap())
        };
        let kinds = |found: Vec<Finding>| {
            found
                .iter()
                .map(|f| matches!(f.kind, Kind::Deny))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(call("mcp__beeper__send_message")),
            vec![true],
            "first send"
        );
        assert_eq!(
            kinds(call("mcp__beeper__send_message")),
            vec![false],
            "sent again"
        );
        assert_eq!(
            kinds(call("mcp__claude-in-chrome__form_input")),
            vec![false],
            "typing only"
        );
    }

    /// Runs only with a dictionary: YHOOKY_UNIDIC_DIR=<dir built by
    /// examples/build_unidic.rs> cargo test.
    #[test]
    fn analyzes_with_the_dictionary() {
        if dictionary_dir().is_none() || std::env::var("YHOOKY_UNIDIC_DIR").is_err() {
            eprintln!("skipped: set YHOOKY_UNIDIC_DIR to run");
            return;
        }
        let words =
            analyze("設定を確認する。足りない項目を足して、結果を確かめる。条件によって違う。")
                .unwrap();
        assert!(words.contains(&"足す".to_string()), "{words:?}");
        assert!(words.contains(&"確かめる".to_string()), "{words:?}");
        assert!(
            !words
                .iter()
                .any(|w| w == "為る" || w == "する" || w == "因る" || w == "よる"),
            "{words:?}"
        );
    }
}
