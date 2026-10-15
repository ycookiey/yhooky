//! askuser-label-guard (PreToolUse, AskUserQuestion).
//!
//! What the user's screen shows of a question (tried on 2026-10-15): the
//! question text and the labels always, with line breaks; an option's
//! description only when no option of the question has a preview; a
//! preview, but not after the answer; the text written before the call
//! only sometimes (longer text went missing, with or without a tool call in
//! between). So a long explanation goes into a ysidepaney note, and the
//! question names it.
//!
//! Refused: an option whose label does not start with an ID (A, B, C, ...),
//! which the explanation refers to, and a description in a question that
//! has previews, as nobody would read it.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::text::lines;

const ID: &str = "askuser-label-guard";

static LABEL_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][0-9]?([^A-Za-z0-9]|$)").unwrap());

fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None | Some(Value::Bool(false)) => String::new(),
        Some(other) => other.to_string(),
    }
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" || input.tool() != "AskUserQuestion" {
        return Vec::new();
    }
    let mut missing = Vec::new();
    let mut hidden = Vec::new();
    let questions = input.get("tool_input.questions").and_then(Value::as_array);
    for question in questions.into_iter().flatten() {
        let options: Vec<&Value> = question
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .collect();
        let has_preview = options
            .iter()
            .any(|o| !text_of(o.get("preview")).trim().is_empty());
        for option in &options {
            let label = text_of(option.get("label"));
            // jq -r prints a label with line breaks as several lines, and
            // each line is checked on its own.
            for line in lines(&label) {
                if !line.is_empty() && !LABEL_ID.is_match(line) {
                    missing.push(line.to_string());
                }
            }
            if has_preview && !text_of(option.get("description")).trim().is_empty() {
                hidden.push(label);
            }
        }
    }
    if missing.is_empty() && hidden.is_empty() {
        return Vec::new();
    }
    let mut problems = Vec::new();
    if !missing.is_empty() {
        let quoted: String = missing.iter().map(|l| format!("「{l}」")).collect();
        problems.push(format!(
            "label を A, B, C … の ID で始める(説明から選択肢を指すため)。ID で始まらない label: {quoted}"
        ));
    }
    if !hidden.is_empty() {
        let quoted: String = hidden.iter().map(|l| format!("「{l}」")).collect();
        problems.push(format!(
            "preview のある質問では description が表示されない。中身は question 文か preview に移し、description は空にする。\
             description のある選択肢: {quoted}"
        ));
    }
    vec![Finding::new(
        ID,
        Kind::Deny,
        format!(
            "{}。直してから呼び直す。長い説明は ysidepaney の note に出し(skill ysidepaney)、question 文には要点と note の題を書く。\
             質問の前の本文は、長いと表示されないことがあるので頼らない。",
            problems.join("。")
        ),
    )]
}
