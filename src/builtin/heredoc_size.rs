//! heredoc-size-guard (PreToolUse, Bash).
//!
//! A long heredoc whose body holds single quotes can fail before bash runs
//! anything: the Bash tool's wrapping of the command ends with `unexpected
//! EOF while looking for matching '` (a 290-line Python script written with
//! `cat > file <<'EOF'`, 2026-10-16; the same text ran from a file). Short
//! heredocs did not fail. Such a command is stopped once per session and
//! command, pointing to the Write tool; the same command sent again goes
//! through. Which were stopped is kept as empty files in
//! `$HEREDOC_GUARD_STATE_DIR`, else `<temp>/claude-heredoc-guard`.

use super::backslash::split_heredocs;
use crate::docs;
use crate::input::Input;
use crate::once;
use crate::output::{Finding, Kind};

const ID: &str = "heredoc-size-guard";

/// Bodies longer than this many lines count as long.
const LONG: usize = 100;

/// The line count of the longest heredoc body that holds a single quote.
pub fn long_quoted_body(command: &str) -> Option<usize> {
    let (_, bodies) = split_heredocs(command);
    bodies
        .iter()
        .filter(|b| b.contains('\''))
        .map(|b| b.split('\n').count())
        .filter(|&n| n > LONG)
        .max()
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" || input.tool() != "Bash" {
        return Vec::new();
    }
    let command = input.command();
    if !command.contains("<<") {
        return Vec::new();
    }
    let Some(lines) = long_quoted_body(&command) else {
        return Vec::new();
    };
    let dir = once::state_dir("HEREDOC_GUARD_STATE_DIR", "claude-heredoc-guard");
    let session = input.text("session_id");
    if !once::first_time(&dir, &format!("{session}\n{command}")) {
        return Vec::new();
    }
    let see = docs::pointer("(", "bash.md", ")");
    vec![Finding::new(
        ID,
        Kind::Deny,
        format!(
            "heredoc の本文が長く({lines} 行)、単一引用符を含む。Bash ツールがコマンドを包む処理で \
             `unexpected EOF while looking for matching` になり、何も実行されないことがある。\
             中身を Write ツールでファイルに書き、`python <file>` などで実行する{see}。\
             このまま試すなら、同じコマンドを出し直せば通る。"
        ),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heredoc(lines: usize, line: &str) -> String {
        let body = vec![line; lines].join("\n");
        format!("cat > a.py <<'EOF'\n{body}\nEOF")
    }

    #[test]
    fn finds_long_quoted_bodies() {
        assert_eq!(long_quoted_body(&heredoc(150, "x = 'a'")), Some(150));
        assert_eq!(long_quoted_body(&heredoc(100, "x = 'a'")), None, "not long");
        assert_eq!(
            long_quoted_body(&heredoc(150, "x = \"a\"")),
            None,
            "no single quote"
        );
        assert_eq!(long_quoted_body("echo 'a'"), None, "no heredoc");
        let two = format!("{}\n{}", heredoc(20, "'"), heredoc(120, "'"));
        assert_eq!(long_quoted_body(&two), Some(120));
    }
}
