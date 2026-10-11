//! What the rules found, and the one answer Claude Code gets for them all.

use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Adds a note to Claude's context and lets the call go on.
    Context,
    /// Refuses the tool call (PreToolUse).
    Deny,
    /// Keeps Claude from stopping (Stop).
    Block,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub rule: String,
    pub kind: Kind,
    pub message: String,
}

impl Finding {
    pub fn new(rule: &str, kind: Kind, message: impl Into<String>) -> Self {
        Self {
            rule: rule.to_string(),
            kind,
            message: message.into(),
        }
    }
}

fn joined(findings: &[Finding], kind: Kind) -> Option<String> {
    let texts: Vec<&str> = findings
        .iter()
        .filter(|f| f.kind == kind)
        .map(|f| f.message.as_str())
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n\n"))
}

/// The hook's answer for `event`: every refusal's reason and every note,
/// each kind joined in the order the rules ran. None when nothing was found.
pub fn answer(event: &str, findings: &[Finding]) -> Option<Value> {
    if findings.is_empty() {
        return None;
    }
    if event == "Stop" || event == "SubagentStop" {
        // A note has no place in a Stop answer: it is sent as the reason.
        let reason = joined(findings, Kind::Block).or_else(|| joined(findings, Kind::Context))?;
        return Some(json!({ "decision": "block", "reason": reason }));
    }
    let mut specific = serde_json::Map::new();
    specific.insert("hookEventName".into(), json!(event));
    if let Some(reason) = joined(findings, Kind::Deny) {
        specific.insert("permissionDecision".into(), json!("deny"));
        specific.insert("permissionDecisionReason".into(), json!(reason));
    }
    if let Some(context) = joined(findings, Kind::Context) {
        specific.insert("additionalContext".into(), json!(context));
    }
    Some(json!({ "hookSpecificOutput": specific }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_each_kind() {
        let found = [
            Finding::new("a", Kind::Context, "one"),
            Finding::new("b", Kind::Deny, "no"),
            Finding::new("c", Kind::Context, "two"),
        ];
        let out = answer("PreToolUse", &found).unwrap();
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(out["hookSpecificOutput"]["permissionDecisionReason"], "no");
        assert_eq!(out["hookSpecificOutput"]["additionalContext"], "one\n\ntwo");
    }

    #[test]
    fn nothing_found_is_no_answer() {
        assert!(answer("PreToolUse", &[]).is_none());
    }
}
