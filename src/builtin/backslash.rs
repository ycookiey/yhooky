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
//! either way, so there it only reminds.

use crate::input::Input;
use crate::output::{Finding, Kind};

const ID: &str = "backslash-shrink-guard";

/// The heredocs a line opens: (strip leading tabs, end word), as the
/// pattern `<<(-?)\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\2` finds them.
fn heredocs(line: &str) -> Vec<(bool, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] != '<' || chars[i + 1] != '<' {
            i += 1;
            continue;
        }
        let mut j = i + 2;
        let strip = chars.get(j) == Some(&'-');
        if strip {
            j += 1;
        }
        while chars.get(j).is_some_and(|c| c.is_whitespace()) {
            j += 1;
        }
        let quote = chars.get(j).copied().filter(|c| *c == '\'' || *c == '"');
        let start = j + usize::from(quote.is_some());
        let mut end = start;
        if chars
            .get(end)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
        {
            end += 1;
            while chars
                .get(end)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                end += 1;
            }
        }
        let closed = match quote {
            Some(q) => chars.get(end) == Some(&q),
            None => true,
        };
        if end > start && closed {
            found.push((strip, chars[start..end].iter().collect()));
            i = end + usize::from(quote.is_some());
        } else {
            // As the pattern would, try again from the next character.
            i += 1;
        }
    }
    found
}

/// The command without heredoc bodies, and the bodies.
fn split_heredocs(command: &str) -> (String, Vec<String>) {
    let lines: Vec<&str> = command.split('\n').collect();
    let mut kept = Vec::new();
    let mut bodies = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        kept.push(line);
        i += 1;
        for (strip, word) in heredocs(line) {
            let mut body = Vec::new();
            while i < lines.len() {
                let end = if strip {
                    lines[i].trim_start_matches('\t')
                } else {
                    lines[i]
                };
                if end == word {
                    kept.push(lines[i]);
                    i += 1;
                    break;
                }
                body.push(lines[i]);
                i += 1;
            }
            bodies.push(body.join("\n"));
        }
    }
    (kept.join("\n"), bodies)
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
    let finding = if where_.is_empty() {
        Finding::new(
            ID,
            Kind::Context,
            "このコマンドの `\\\\` は、実行の前に `\\` 1 本に縮んだ(~/.claude/docs/bash.md)。\
             二重引用の中や引用の外なら bash が同じ結果にすることが多いが、出力が意図どおりか確かめる。",
        )
    } else {
        Finding::new(
            ID,
            Kind::Deny,
            format!(
                "Bash ツールは実行の前に `\\\\` を `\\` 1 本に縮める(~/.claude/docs/bash.md)。{}の `\\\\` は、\
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
    fn finds_heredoc_words() {
        assert_eq!(
            heredocs("cat <<'EOF' > a"),
            vec![(false, "EOF".to_string())]
        );
        assert_eq!(heredocs("cat <<- \"X\""), vec![(true, "X".to_string())]);
        assert_eq!(heredocs("cat <<'EOF"), Vec::<(bool, String)>::new());
        assert_eq!(
            heredocs("a <<A b <<B"),
            vec![(false, "A".into()), (false, "B".into())]
        );
    }
}
