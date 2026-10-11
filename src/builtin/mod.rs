//! Checks a pattern cannot express: they parse quoting, look at files or
//! read the transcript. A rule file turns each on by name (`builtins`).

use std::fs;

use crate::home;
use crate::input::Input;
use crate::output::Finding;

pub mod askuser;
pub mod backslash;
pub mod head_trunc;
pub mod label;
pub mod language;
pub mod post_push;
pub mod rm_junction;
pub mod sendfile;

type Check = fn(&Input) -> Vec<Finding>;

/// Every built-in check, by the name a rule file uses.
pub const ALL: [(&str, Check); 8] = [
    ("backslash-shrink-guard", backslash::run),
    ("rm-junction-guard", rm_junction::run),
    ("askuser-label-guard", askuser::run),
    ("sendfile-to-ysidepaney", sendfile::run),
    ("post-push-reminder", post_push::run),
    ("head-truncation-reminder", head_trunc::run),
    ("label-guard", label::run),
    ("language-guard", language::run),
];

pub fn find(name: &str) -> Option<Check> {
    ALL.iter()
        .find(|(n, _)| *n == name)
        .map(|(_, check)| *check)
}

/// Saves the hook's input under ~/.claude/logs as `name`, to look into a
/// wrong block afterwards. Each block overwrites the last.
pub fn keep_input(input: &Input, name: &str) {
    let Some(dir) = home().map(|h| h.join(".claude").join("logs")) else {
        return;
    };
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(dir.join(name), &input.raw);
    }
}
