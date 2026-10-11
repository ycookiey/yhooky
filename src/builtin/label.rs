//! label-guard (Stop).
//!
//! When the last reply uses Greek letters or Roman numerals as labels,
//! keeps Claude from stopping and asks for A, B, C, ... instead. Code
//! blocks, quoted lines and inline code are left out, so markers mentioned
//! there do not count.

use std::sync::LazyLock;

use regex::Regex;

use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::text::lines;

const ID: &str = "label-guard";

/// A Greek letter as a label. At the start of a line it needs a space or a
/// delimiter after it, so terms like "α粒子" pass; anywhere, a strong
/// delimiter or a Japanese label word after it counts.
static GREEK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^[[:space:]]*[-*+]?[[:space:]]*[αβγδεζ][[:space:].):、：]|[αβγδεζ][.):]|[αβγδεζ](案|方式|パターン|プラン|型))")
        .unwrap()
});
static ROMAN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[[:space:]]*[-*+]?[[:space:]]*(i{1,3}|iv|v|vi{1,3})[.)]").unwrap()
});
static QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[[:space:]]*>").unwrap());
static INLINE_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`]*`").unwrap());

fn prose(text: &str) -> Vec<String> {
    let mut kept = Vec::new();
    let mut in_fence = false;
    for line in lines(text) {
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || QUOTE.is_match(line) {
            continue;
        }
        kept.push(INLINE_CODE.replace_all(line, "").into_owned());
    }
    kept
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "Stop" {
        return Vec::new();
    }
    let text = input.text("last_assistant_message");
    if text.is_empty() {
        return Vec::new();
    }
    let kept = prose(&text);
    let mut violations = Vec::new();
    if kept.iter().any(|l| GREEK.is_match(l)) {
        violations.push("Greek letters (α, β, γ, ...)");
    }
    if kept.iter().any(|l| ROMAN.is_match(l)) {
        violations.push("Roman numerals (i., ii., iii., ...)");
    }
    if violations.is_empty() {
        return Vec::new();
    }
    super::keep_input(input, "label-guard-last-block.json");
    vec![Finding::new(
        ID,
        Kind::Block,
        format!(
            "Forbidden label style detected in your previous response: {}. Rewrite that response now, replacing the \
             offending labels with A, B, C, ... . Deliver only the corrected response.",
            violations.join(", ")
        ),
    )]
}
