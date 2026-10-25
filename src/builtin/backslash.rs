//! backslash-shrink-guard (PreToolUse, Bash).
//!
//! Claude Code's Bash tool turns `\\` into a single `\` before bash reads
//! the command. Where bash would have kept the text as written, that
//! changes what runs:
//! - a heredoc body (a Python script's "\\n" became a real newline), and
//! - single quotes and $'...'.
//!
//! - double quotes, where the `\\` comes right before `$`, `` ` `` or `"`:
//!   the shrunk `\` escapes that character (`"HKLM\\$k"` stopped expanding
//!   `$k`, 2026-10-12).
//!
//! Those are refused, pointing to single quotes or the Write tool. Elsewhere
//! in double quotes, or outside quotes, bash often gives the same result
//! either way, so there it only reminds, unless the command hands a script
//! over inline (`node -e`, `python -c`, `sed`, `awk`, ...): the script then
//! reads one backslash where two were written (a replacement's `\\r\\n` and
//! a JS `'\\n'` came out broken, 2026-10-15). That is stopped once per
//! session and command; the same command sent again only reminds. Which
//! commands were stopped is kept as empty files in
//! `$BACKSLASH_GUARD_STATE_DIR`, else `<temp>/claude-backslash-guard`.

use std::sync::LazyLock;

use regex::Regex;

use crate::docs;
use crate::input::Input;
use crate::once;
use crate::output::{Finding, Kind};
pub(crate) use crate::shell::split_heredocs;

const ID: &str = "backslash-shrink-guard";

/// A command that takes its script as an argument.
static INLINE_SCRIPT: LazyLock<Regex> = LazyLock::new(|| {
    let head = r"(^|[;&|(\s])";
    let args = r"\s([^;&|\n]*\s)?";
    Regex::new(&format!(
        r"{head}(node|bun)(\.exe)?{args}(-e|-p|--eval|--print)(\s|=)|{head}deno(\.exe)?\s+eval\s|{head}(python3?|py)(\.exe)?{args}-c\s|{head}(perl|ruby)(\.exe)?{args}-[a-zA-Z]*e\s|{head}(sed|g?awk)(\.exe)?\s"
    ))
    .unwrap()
});

/// Whether this session sends `command` for the first time; remembers it.
fn first_time(session: &str, command: &str) -> bool {
    let dir = once::state_dir("BACKSLASH_GUARD_STATE_DIR", "claude-backslash-guard");
    once::first_time(&dir, &format!("{session}\n{command}"))
}

#[derive(PartialEq)]
enum Place {
    Single,
    /// In double quotes, right before `$`, `` ` `` or `"`.
    DoubleSpecial,
    Other,
}

/// Where each `\\` sits: in single quotes or $'...', in double quotes before
/// a character a backslash escapes there, or elsewhere.
fn places(text: &str) -> Vec<Place> {
    #[derive(PartialEq)]
    enum State {
        Out,
        Single,
        Ansi,
        Double,
    }
    let chars: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    let mut state = State::Out;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let pair = chars.get(i + 1).is_some_and(|n| c == '\\' && *n == '\\');
        match state {
            State::Single => {
                if pair {
                    found.push(Place::Single);
                    i += 2;
                    continue;
                }
                if c == '\'' {
                    state = State::Out;
                }
            }
            State::Ansi => {
                if pair {
                    found.push(Place::Single);
                    i += 2;
                    continue;
                }
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == '\'' {
                    state = State::Out;
                }
            }
            State::Double => {
                if pair {
                    let escapes = chars
                        .get(i + 2)
                        .is_some_and(|n| matches!(n, '$' | '`' | '"'));
                    found.push(if escapes {
                        Place::DoubleSpecial
                    } else {
                        Place::Other
                    });
                    i += 2;
                    continue;
                }
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == '"' {
                    state = State::Out;
                }
            }
            State::Out => {
                if pair {
                    found.push(Place::Other);
                    i += 2;
                    continue;
                }
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == '$' && chars.get(i + 1) == Some(&'\'') {
                    state = State::Ansi;
                    i += 2;
                    continue;
                }
                if c == '\'' {
                    state = State::Single;
                } else if c == '"' {
                    state = State::Double;
                }
            }
        }
        i += 1;
    }
    found
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" || input.tool() != "Bash" {
        return Vec::new();
    }
    let command = input.command();
    if !command.contains("\\\\") {
        return Vec::new();
    }
    let (rest, bodies) = split_heredocs(&command);
    let mut where_ = Vec::new();
    if bodies.iter().any(|b| b.contains("\\\\")) {
        where_.push("heredoc の本文");
    }
    let places = places(&rest);
    if places.contains(&Place::Single) {
        where_.push("単一引用('…'・$'…')の中");
    }
    if places.contains(&Place::DoubleSpecial) {
        where_.push("二重引用の中で `$`・`` ` ``・`\"` の直前(縮んだ `\\` がその文字を打ち消す。`\"…\\\\$k\"` の `$k` が展開されない)");
    }
    let see = docs::pointer("(", "bash.md", ")");
    let finding = if where_.is_empty()
        && INLINE_SCRIPT.is_match(&rest)
        && first_time(&input.text("session_id"), &command)
    {
        Finding::new(
            ID,
            Kind::Deny,
            format!(
                "Bash ツールは実行の前に `\\\\` を `\\` 1 本に縮める{see}。\
                 インラインのスクリプト(node -e・python -c・sed・awk 等)の中の `\\\\` は、スクリプトが受け取る文字が変わる\
                 (置換文字列の `\\\\r\\\\n`、JS の `'\\\\n'` が壊れた)。中身を Write ツールでファイルに書いて実行する\
                 (node <file>・python <file>・sed -f <file>)。意図どおりなら同じコマンドを出し直せば通す。"
            ),
        )
    } else if where_.is_empty() {
        Finding::new(
            ID,
            Kind::Context,
            format!(
                "このコマンドの `\\\\` は、実行の前に `\\` 1 本に縮んだ{see}。\
                 二重引用の中や引用の外なら bash が同じ結果にすることが多いが、出力が意図どおりか確かめる。"
            ),
        )
    } else {
        Finding::new(
            ID,
            Kind::Deny,
            format!(
                "Bash ツールは実行の前に `\\\\` を `\\` 1 本に縮める{see}。{}の `\\\\` は、\
                 書いたとおりには渡らない。単一引用で書けるなら単一引用にする。でなければ、中身を Write ツールでファイルに書いてから、\
                 そのファイルを実行する・読ませる(スクリプトはファイルにして python <file>、データはファイルで渡す)。",
                where_.join("・")
            ),
        )
    };
    vec![finding]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_inline_scripts() {
        for command in [
            r#"node -e "x""#,
            r#"node --eval="x""#,
            r#"bun -p "x""#,
            r#"deno eval "x""#,
            r#"python -c "x""#,
            r#"py -3 -c "x""#,
            r#"python3 -I -c "x""#,
            "perl -pe 's/a/b/' f",
            r#"ruby -e "x""#,
            r#"sed -i "s/a/b/" f"#,
            "cd x && sed -n 1p f",
            "awk '{print}' f",
            r#"ls | gawk "x""#,
        ] {
            assert!(INLINE_SCRIPT.is_match(command), "{command}");
        }
        for command in [
            r#"grep -n "a" f"#,
            "echo a",
            "nodemon x",
            "python script.py",
            "node script.js",
            "used x",
            "git log --grep=sed x",
        ] {
            assert!(!INLINE_SCRIPT.is_match(command), "{command}");
        }
    }
}
