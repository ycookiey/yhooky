//! head-truncation-reminder (PostToolUse, Bash).
//!
//! When the output reached the line limit of the command's last `| head`
//! (so lines may have been cut), asks to count before concluding there is
//! nothing else. Quiet when the output stopped short of the limit.

use std::sync::LazyLock;

use regex::Regex;

use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::text::{line_count, lines, matches};

const ID: &str = "head-truncation-reminder";

static LEADING_CD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[[:space:]]*cd[[:space:]]+[^;&|[:space:]]+[[:space:]]*&&").unwrap()
});
static MORE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r";|&&|\|\|").unwrap());
static HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\|[[:space:]]*head([[:space:]]+(-n[[:space:]]*[0-9]+|-[0-9]+))?").unwrap()
});
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]+").unwrap());

/// `line` without the `cd <dir> &&` at its start, as many times as there are.
fn without_leading_cd(line: &str) -> String {
    let mut line = line.to_string();
    while let Some(m) = LEADING_CD.find(&line) {
        line = line[m.end()..].to_string();
    }
    line
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PostToolUse" || input.tool() != "Bash" {
        return Vec::new();
    }
    // `$(...)` takes the newlines off the end of what jq printed.
    let command = input.command().trim_end_matches('\n').to_string();
    // The output belongs to that head only for one pipeline, with at most
    // `cd <dir> &&` before it.
    let body: Vec<String> = lines(&command).map(without_leading_cd).collect();
    let body = body.join("\n");
    if line_count(&body) > 1 || MORE.is_match(&body) {
        return Vec::new();
    }
    let Some(last) = matches(&HEAD, &command).pop() else {
        return Vec::new();
    };
    let limit: usize = matches(&NUMBER, last)
        .pop()
        .and_then(|n| n.parse().ok())
        .unwrap_or(10);
    let output = input.response_text(&["stdout", "output"]);
    let output = output.trim_end_matches('\n');
    if line_count(output) < limit {
        return Vec::new();
    }
    vec![Finding::new(
        ID,
        Kind::Context,
        format!(
            "出力が head の上限({limit} 行)に届いた。切られた行がありうる。「ほかにない」「これで全部」と結論する前に、\
             件数を数える(| wc -l、grep -c)か、上限を外して確かめる。"
        ),
    )]
}
