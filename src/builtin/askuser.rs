//! askuser-label-guard (PreToolUse, AskUserQuestion).
//!
//! The user's screen does not show an option's description (the label, the
//! preview and the text before it are shown). Details go in the text before
//! or the preview, tied to labels that start with an ID (A, B, C, ...). An
//! option whose label does not start with one is refused. Whether the text
//! explains the options is not checked: the transcript does not always hold
//! that text yet, and checking would refuse explained options.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::text::lines;

const ID: &str = "askuser-label-guard";

static LABEL_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][0-9]?([^A-Za-z0-9]|$)").unwrap());

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" || input.tool() != "AskUserQuestion" {
        return Vec::new();
    }
    let mut missing = Vec::new();
    let questions = input.get("tool_input.questions").and_then(Value::as_array);
    for question in questions.into_iter().flatten() {
        let options = question.get("options").and_then(Value::as_array);
        for option in options.into_iter().flatten() {
            let label = match option.get("label") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Null) | None | Some(Value::Bool(false)) => continue,
                Some(other) => other.to_string(),
            };
            // jq -r prints a label with line breaks as several lines, and
            // each line is checked on its own.
            for line in lines(&label) {
                if !line.is_empty() && !LABEL_ID.is_match(line) {
                    missing.push(line.to_string());
                }
            }
        }
    }
    if missing.is_empty() {
        return Vec::new();
    }
    let quoted: String = missing.iter().map(|l| format!("「{l}」")).collect();
    vec![Finding::new(
        ID,
        Kind::Deny,
        format!(
            "AskUserQuestion の選択肢の description はユーザーの UI に表示されない(label・preview・直前の本文は表示される)。\
             各選択肢の中身・トレードオフは直前の本文か preview で説明し、label を A, B, C … の ID で始めて本文の説明と\
             対応づけてから呼び直す。ID で始まらない label: {quoted}"
        ),
    )]
}
