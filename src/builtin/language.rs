//! language-guard (Stop): when the turn's final reply is mostly English,
//! keeps Claude from stopping and asks for it in Japanese.
//!
//! What is judged: the text after the turn's last tool call (from the
//! transcript) and `last_assistant_message`. Code blocks, inline code,
//! quoted lines, URLs, e-mail addresses and Markdown links are left out
//! before counting. A text is English when it has at least
//! LANGUAGE_GUARD_MIN_LATIN Latin letters and more than
//! LANGUAGE_GUARD_RATIO times as many Latin letters as Japanese characters.
//! A rewrite is never blocked again (`stop_hook_active`), so the guard
//! cannot loop. The short lines between tool calls are not seen.
//!
//! The approach follows minorun365/claude-code-japanese-guard (Apache-2.0).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::LazyLock;
use std::thread::sleep;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

use crate::input::Input;
use crate::output::{Finding, Kind};

const ID: &str = "language-guard";
/// The final text of a turn comes after its last tool call, near the end.
const TAIL_LINES: usize = 400;
/// Where to look for whether the final reply has been written.
const ENDED_LINES: usize = 50;
const POLL: Duration = Duration::from_millis(100);

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[[:space:]]*```").unwrap());
static QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[[:space:]]*>").unwrap());
static LEFT_OUT: LazyLock<[Regex; 4]> = LazyLock::new(|| {
    [
        Regex::new(r"`[^`]*`").unwrap(),
        Regex::new(r"\[[^\]]*\]\([^)]*\)").unwrap(),
        Regex::new(r"https?://[^[:space:]]+").unwrap(),
        Regex::new(r"[[:alnum:]._%+-]+@[[:alnum:].-]+\.[[:alpha:]][[:alpha:]]+").unwrap(),
    ]
});

fn is_japanese(c: char) -> bool {
    matches!(c, 'ぁ'..='ん' | 'ァ'..='ヶ' | 'ー' | '一'..='龥' | '々')
}

/// The first line of `text` when it is mostly English.
pub fn english_head(text: &str, min_latin: u64, ratio: u64) -> Option<String> {
    let mut prose = String::new();
    let mut head = String::new();
    let mut in_fence = false;
    for line in text.split('\n') {
        if head.is_empty() && line.chars().any(|c| !c.is_whitespace()) {
            head = line.chars().take(80).collect();
        }
        if FENCE.is_match(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || QUOTE.is_match(line) {
            continue;
        }
        let mut line = line.to_string();
        for re in LEFT_OUT.iter() {
            line = re.replace_all(&line, "").into_owned();
        }
        prose.push_str(&line);
        prose.push('\n');
    }
    let latin = prose.chars().filter(char::is_ascii_alphabetic).count() as u64;
    let ja = prose.chars().filter(|c| is_japanese(*c)).count() as u64;
    (latin >= min_latin && latin > ja * ratio).then_some(head)
}

/// A user entry starts a turn when it is typed text: not only tool
/// results, not a meta message.
fn is_user_turn(entry: &Value) -> bool {
    if entry["type"] != "user" || entry["isMeta"].as_bool().unwrap_or(false) {
        return false;
    }
    match &entry["message"]["content"] {
        Value::String(_) => true,
        Value::Array(blocks) => blocks.iter().any(|b| b["type"] != "tool_result"),
        _ => false,
    }
}

/// Each turn's final texts, the turn before the first prompt first. A tool
/// call drops the texts before it.
pub fn turns(entries: impl Iterator<Item = Value>) -> Vec<Vec<String>> {
    let mut done = Vec::new();
    let mut last: Vec<String> = Vec::new();
    for entry in entries {
        if is_user_turn(&entry) {
            done.push(std::mem::take(&mut last));
        } else if entry["type"] == "assistant" {
            for block in entry["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use" {
                    last.clear();
                } else if block["type"] == "text" {
                    last.push(block["text"].as_str().unwrap_or_default().to_string());
                }
            }
        }
    }
    done.push(last);
    done
}

/// The entries in `lines`. A line that is not JSON (one still being
/// written) is skipped.
fn entries(lines: &[String]) -> impl Iterator<Item = Value> + '_ {
    lines.iter().filter_map(|l| serde_json::from_str(l).ok())
}

/// The last `count` lines of the file, read from its end.
fn tail(path: &Path, count: usize) -> Vec<String> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(size) = file.seek(SeekFrom::End(0)) else {
        return Vec::new();
    };
    let mut start = size;
    let mut chunk = 64 * 1024u64;
    let mut buf = Vec::new();
    loop {
        start = start.saturating_sub(chunk);
        buf.clear();
        if file.seek(SeekFrom::Start(start)).is_err()
            || file
                .by_ref()
                .take(size - start)
                .read_to_end(&mut buf)
                .is_err()
        {
            return Vec::new();
        }
        let newlines = buf.iter().filter(|b| **b == b'\n').count();
        if start == 0 || newlines > count {
            break;
        }
        chunk *= 2;
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    if start > 0 && !lines.is_empty() {
        // The first line may have been cut.
        lines.remove(0);
    }
    let skip = lines.len().saturating_sub(count);
    lines.split_off(skip)
}

/// The final reply has been written: the last assistant entry ends with text.
fn ended_in_text(path: &Path) -> bool {
    let lines = tail(path, ENDED_LINES);
    let last = entries(&lines).filter(|e| e["type"] == "assistant").last();
    last.and_then(|e| {
        e["message"]["content"]
            .as_array()
            .and_then(|c| c.last())
            .cloned()
    })
    .is_some_and(|b| b["type"] == "text")
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "Stop" || input.text("stop_hook_active") == "true" {
        return Vec::new();
    }
    let min = env_number("LANGUAGE_GUARD_MIN_LATIN", 25);
    let ratio = env_number("LANGUAGE_GUARD_RATIO", 20);
    let wait = env_number("LANGUAGE_GUARD_WAIT", 3);
    let mut heads: Vec<String> = english_head(&input.text("last_assistant_message"), min, ratio)
        .into_iter()
        .collect();
    let transcript = input.text("transcript_path");
    let path = Path::new(&transcript);
    if !transcript.is_empty() && path.is_file() {
        // Without an English message to go on, wait for the final reply to
        // reach the transcript before reading it.
        if heads.is_empty() {
            let deadline = Instant::now() + Duration::from_secs(wait);
            while !ended_in_text(path) && Instant::now() < deadline {
                sleep(POLL);
            }
        }
        let lines = tail(path, TAIL_LINES);
        let last = turns(entries(&lines)).pop().unwrap_or_default();
        heads.extend(last.iter().filter_map(|t| english_head(t, min, ratio)));
    }
    let mut unique: Vec<String> = Vec::new();
    for head in heads.into_iter().filter(|h| !h.is_empty()) {
        if !unique.contains(&head) {
            unique.push(head);
        }
    }
    if unique.is_empty() {
        return Vec::new();
    }
    let quoted: Vec<String> = unique.iter().take(5).map(|h| format!("- {h}")).collect();
    super::keep_input(input, "language-guard-last-block.json");
    vec![Finding::new(
        ID,
        Kind::Block,
        format!(
            "直前の返答が英語主体だった。同じ内容を日本語で書き直し、書き直した返答だけを出す(コード・識別子・引用はそのままでよい)。\
             英語と判定した本文の書き出し:\n{}",
            quoted.join("\n")
        ),
    )]
}

/// Each turn of a past session whose final text the guard would block, as
/// `turn <n>: <first line>` (`yhooky language-check <transcript.jsonl>`).
pub fn check(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<String> = text.lines().map(String::from).collect();
    let min = env_number("LANGUAGE_GUARD_MIN_LATIN", 25);
    let ratio = env_number("LANGUAGE_GUARD_RATIO", 20);
    let mut found = Vec::new();
    for (i, texts) in turns(entries(&lines)).iter().enumerate() {
        for t in texts {
            if let Some(head) = english_head(t, min, ratio) {
                found.push(format!("turn {}: {head}", i + 1));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judges_english() {
        let en = "This is an English reply with quite a lot of Latin letters in it.";
        assert_eq!(english_head(en, 25, 20).as_deref(), Some(en));
        assert!(
            english_head(
                "日本語の返答。`some_identifier_here_long` を直した。",
                25,
                20
            )
            .is_none()
        );
        assert!(
            english_head(
                "```\nlots of english code here and more and more\n```\n短い",
                25,
                20
            )
            .is_none()
        );
    }
}
