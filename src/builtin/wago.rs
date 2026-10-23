//! wago-verb-reminder (PreToolUse).
//!
//! In Japanese written for other people, notes the verbs that are not
//! Sino-Japanese compounds (和語の動詞: 要る・確かめる・足す), so that saying
//! them with a compound (必要・確認・追加) can be weighed, and phrases that
//! make the text read as machine-written or translated from English
//! (phrases.rs). Replacing them is not asked for. It looks at:
//! - text typed into a page or sent as a message (form_input and computer
//!   of claude-in-chrome, beeper's send_message),
//! - text written with Write or Edit into a README or an HTML file of a
//!   public repository (src/public_repo.rs). Documents for agents (docs,
//!   CLAUDE.md, skill definitions) are left out, and
//! - the title and body of a PR or a release that gh creates or edits in a
//!   public repository: the title, body and notes given in the command
//!   (inline or in a heredoc), and the files it names with `--body-file`,
//!   `--notes-file` or `-F`.
//!
//! The text is split into words with UniDic (lindera); a word counts when
//! its part of speech is 動詞 and its word type (語種) is 和, unless its
//! lexeme is a helper that grammar asks for (為る in 確認する, 有る, 居る,
//! 置く, 因る in によって). The dictionary is read only when a text gets
//! that far: from `$YHOOKY_UNIDIC_DIR`, else the `unidic` folder beside the
//! executable (built by examples/build_unidic.rs). Without it no verbs are
//! noted; the phrases still are.
//!
//! A note reaches Claude with the tool's result, after a message has gone
//! out or a PR has been made, so the calls that may send or publish (a
//! message, a click or a key in a page, gh) are stopped once per session
//! and text instead; the same call sent again goes through. Which were
//! stopped is kept as empty files in
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

use super::backslash::split_heredocs;
use super::phrases;
use super::post_push::runs;
use super::public_writing::{gh_dir, public_repo_of_command};
use crate::docs;
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

/// gh commands that publish text: a PR or a release, made or edited.
static PUBLISH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(^|[^[:alnum:]_-])gh[[:space:]]+(pr|release)[[:space:]]+(create|edit)([[:space:]]|$)",
    )
    .unwrap()
});
/// A title, body or notes given to such a command, inline or as a file.
static TEXT_FLAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(^|\s)(--title|-t|--body|-b|--notes|-n|--body-file|--notes-file|-F)(=|\s+)("(?:[^"\\]|\\.)*"|'[^']*'|[^\s;|&)]+)"#,
    )
    .unwrap()
});
const FILE_FLAGS: [&str; 3] = ["--body-file", "--notes-file", "-F"];

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

/// What a gh command publishes, each piece on lines of its own: the
/// heredoc bodies in the command (a body is often given as `--body
/// "$(cat <<'EOF' ...)"`), the title, body and notes given after `gh`, and
/// the files named there (relative ones from `dir`; `-` is standard input
/// and not read).
fn published_text(command: &str, dir: &Path) -> String {
    let (rest, mut pieces) = split_heredocs(command);
    let from = PUBLISH.find(&rest).map_or(0, |m| m.start());
    for caps in TEXT_FLAG.captures_iter(&rest[from..]) {
        let value = &caps[4];
        let value = match value.chars().next() {
            Some('"') => value[1..value.len() - 1].replace("\\\"", "\""),
            Some('\'') => value[1..value.len() - 1].to_string(),
            _ => value.to_string(),
        };
        if !FILE_FLAGS.contains(&&caps[2]) {
            pieces.push(value);
            continue;
        }
        if value == "-" {
            continue;
        }
        let path = PathBuf::from(windows_path(&value));
        let path = if path.is_absolute() {
            path
        } else {
            dir.join(path)
        };
        if let Ok(body) = std::fs::read_to_string(&path) {
            pieces.push(body);
        }
    }
    pieces.join("\n")
}

/// Text this call puts out for other people.
struct Outgoing {
    text: String,
    /// Whether the call may send or publish it, past revising.
    sends: bool,
}

/// The text to look at, if this call writes Japanese for other people.
fn outgoing_text(input: &Input) -> Option<Outgoing> {
    let tool = input.tool();
    if TYPING_TOOL.is_match(&tool) {
        let fields: Vec<String> = TYPING_FIELDS.iter().map(|f| f.to_string()).collect();
        return Some(Outgoing {
            text: input.first_text(&fields),
            sends: SENDING_TOOL.is_match(&tool),
        });
    }
    if tool == "Bash" || tool == "PowerShell" {
        let command = input.command();
        if !runs(&PUBLISH, &command) {
            return None;
        }
        let cwd = input.cwd();
        let dir = windows_path(&gh_dir(&command, &cwd));
        let text = published_text(&command, Path::new(&dir));
        if !JAPANESE.is_match(&text) {
            return None;
        }
        // Looked up only now: it may run `gh`.
        public_repo_of_command(&command, &cwd)?;
        return Some(Outgoing {
            text: prose("body.md", &text),
            sends: true,
        });
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
    Some(Outgoing {
        text: prose(&path, &text),
        sends: false,
    })
}

/// The note on native verbs, if any are found.
fn verb_note(text: &str) -> Option<String> {
    let mut words = analyze(text)?;
    if words.is_empty() {
        return None;
    }
    let more = words.len().saturating_sub(LIMIT);
    words.truncate(LIMIT);
    let rest = if more > 0 {
        format!(" ほか {more} 語")
    } else {
        String::new()
    };
    // The rule numbers belong to the document, so they go with its pointer.
    let axis = match docs::path("external-japanese.md") {
        Some(path) => format!("規則1(具体的な語を選ぶ)とは別の軸({path} の規則19)。"),
        None => "具体的な語を選ぶこととは別の軸。".to_string(),
    };
    Some(format!(
        "外に出る日本語の文に、熟語でない動詞(和語)がある: {}{rest}。熟語で言うほうが通るかを考え直す。置き換えは任意。{axis}",
        words.join("・")
    ))
}

/// The note on phrases, if any are found.
fn phrase_note(text: &str) -> Option<String> {
    let found = phrases::notes(text);
    if found.is_empty() {
        return None;
    }
    let lines: String = found.iter().map(|line| format!("\n- {line}")).collect();
    Some(format!(
        "外に出る日本語の文に、見直す言い回しがある。直すかは文脈で決める。{lines}"
    ))
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" {
        return Vec::new();
    }
    let Some(Outgoing { text, sends }) = outgoing_text(input) else {
        return Vec::new();
    };
    if !JAPANESE.is_match(&text) {
        return Vec::new();
    }
    let notes: Vec<String> = [verb_note(&text), phrase_note(&text)]
        .into_iter()
        .flatten()
        .collect();
    if notes.is_empty() {
        return Vec::new();
    }
    let note = notes.join("\n");
    let stop = sends && first_time(&input.text("session_id"), &text);
    let finding = if stop {
        Finding::new(
            ID,
            Kind::Deny,
            format!(
                "{note}\n送ったり公開したりした文は、後で直しても相手に届いたものが残るので、その前に一度止めた。このまま進めるなら、同じ操作を出し直せば通る。"
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

    #[test]
    fn reads_what_gh_publishes() {
        for command in [
            "gh pr create --title t --body b",
            "cd x && gh release edit v1 --notes-file n.md",
            "gh pr edit 3 -F body.md",
        ] {
            assert!(runs(&PUBLISH, command), "{command}");
        }
        for command in ["gh pr view 3", "gh release list", "echo 'gh pr create'"] {
            assert!(!runs(&PUBLISH, command), "{command}");
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("body.md"), "本文の文。").unwrap();
        std::fs::write(dir.path().join("n.md"), "ノートの文。").unwrap();
        let text = published_text(
            "grep -F x y && gh pr create --title 題 --body-file body.md && gh release create v1 --notes-file=\"n.md\" -F -",
            dir.path(),
        );
        assert_eq!(text, "題\n本文の文。\nノートの文。");
        let text = published_text(
            "gh pr create -t \"重要なのは速さ\" -b '結論から言うと、動く。'",
            dir.path(),
        );
        assert_eq!(text, "重要なのは速さ\n結論から言うと、動く。");
        let text = published_text(
            "gh pr create --title \"題 \\\"a\\\"\" --body \"$(cat <<'EOF'\n本文に \"引用\" がある。\nEOF\n)\"",
            dir.path(),
        );
        assert!(text.contains("本文に \"引用\" がある。"), "{text}");
        assert!(text.contains("題 \"a\""), "{text}");
        let notes = phrases::notes(&published_text(
            "gh pr create --title \"重要なのは速さ\" --body \"結論から言うと、動く。いかがでしたか\"",
            dir.path(),
        ));
        assert!(
            notes
                .join("")
                .contains("重要なのは・結論から言うと・いかがでしたでしょうか"),
            "{notes:?}"
        );
    }

    /// The phrase is found without a dictionary, the verb only with one.
    #[test]
    fn stops_a_send_once() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: only this test reads the variable.
        unsafe { std::env::set_var("WAGO_GUARD_STATE_DIR", dir.path()) };
        let call = |tool: &str| {
            let text = "結果を確かめる。いかがでしたでしょうか。";
            let raw = serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": tool,
                "session_id": "s",
                "tool_input": { "text": text, "value": text },
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
