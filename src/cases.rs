//! `yhooky test <cases.toml>`: checks rules against example inputs.
//!
//! A case file sits in a `tests` folder beside the rule files it checks,
//! and only the rules of that folder (the parent of `tests`) are loaded.
//! Each case names one rule and what that rule should do with its input; a
//! case naming a rule the folder does not have fails, and so does a folder
//! with no rules at all:
//!
//! ```toml
//! [setup]                 # optional
//! repos = { kit = "https://github.com/someone/os-tourney-kit.git" }
//! env = { SHARED = "{root}/kit" }
//! files = { "kit/scripts/x.sh" = "" }
//! cwd = "{root}/kit"
//!
//! [[case]]
//! name = "push"
//! rule = "push-check-reminder"
//! expect = "context"      # quiet, context, deny or block
//! command = "git push"    # the Bash command (tool_input.command)
//! input = { tool_name = "Edit", tool_input = { file_path = "a.md" } }  # merged over the default input
//! contains = "push前"      # text the message must hold
//! excludes = "push後"      # text no message may hold
//! ```
//!
//! `{root}` is a scratch folder (as Git Bash writes it, `/c/...`) holding
//! one git repository per `repos` entry, with that `origin`; `{root_win}`
//! is the same folder as Windows writes it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::findings;
use crate::input::Input;
use crate::output::Kind;
use crate::rules::Rules;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseFile {
    #[serde(default)]
    setup: Setup,
    case: Vec<Case>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Setup {
    #[serde(default)]
    repos: BTreeMap<String, String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Files to make under the scratch folder, by path, with their text.
    #[serde(default)]
    files: BTreeMap<String, String>,
    cwd: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    rule: String,
    expect: String,
    command: Option<String>,
    input: Option<toml::Value>,
    contains: Option<String>,
    excludes: Option<String>,
}

/// `root` as Git Bash writes it: `C:\x\y` as `/c/x/y`.
fn git_bash_path(root: &Path) -> String {
    let text = root.to_string_lossy().replace('\\', "/");
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        format!("/{}{}", (bytes[0] as char).to_ascii_lowercase(), &text[2..])
    } else {
        text
    }
}

fn merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                merge(b.entry(k).or_insert(Value::Null), v);
            }
        }
        (b, o) => *b = o,
    }
}

/// Every string in `value` with the placeholders filled in.
fn fill(value: Value, root: &str, root_win: &str) -> Value {
    match value {
        Value::String(s) => {
            Value::String(s.replace("{root_win}", root_win).replace("{root}", root))
        }
        Value::Array(a) => Value::Array(a.into_iter().map(|v| fill(v, root, root_win)).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, v)| (k, fill(v, root, root_win)))
                .collect(),
        ),
        other => other,
    }
}

/// Runs the cases in `file`; prints OK or NG for each and returns how many
/// failed.
pub fn run(file: &Path) -> Result<usize, String> {
    // Made absolute first: the parent's parent of `tests/x.toml` is an empty
    // path, which no folder can be read from.
    let file = &std::path::absolute(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let cases: CaseFile = toml::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let rules_dir = file
        .parent()
        .and_then(Path::parent)
        .ok_or("the case file must be in a tests folder beside the rules")?
        .to_path_buf();
    let rules = Rules::load(std::slice::from_ref(&rules_dir));
    if !rules.problems.is_empty() {
        return Err(rules.problems.join("\n"));
    }
    // With no rules every case would come out quiet, and the quiet ones
    // would pass without checking anything.
    if rules.rules.is_empty() && rules.builtins.is_empty() {
        return Err(format!("{}: no rules", rules_dir.display()));
    }

    let scratch = std::env::temp_dir().join(format!("yhooky-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let scratch: PathBuf = scratch
        .canonicalize()
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
        })
        .map_err(|e| e.to_string())?;
    let root = git_bash_path(&scratch);
    let root_win = scratch.to_string_lossy().to_string();
    for (name, origin) in &cases.setup.repos {
        let dir = scratch.join(name);
        let ok = Command::new("git")
            .args(["init", "-q"])
            .arg(&dir)
            .status()
            .is_ok_and(|s| s.success())
            && Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["remote", "add", "origin", origin])
                .status()
                .is_ok_and(|s| s.success());
        if !ok {
            return Err(format!("could not make the repository {name}"));
        }
    }
    for (path, body) in &cases.setup.files {
        let path = scratch.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, body).map_err(|e| e.to_string())?;
    }
    let fill_text = |s: &str| s.replace("{root_win}", &root_win).replace("{root}", &root);
    for (key, value) in &cases.setup.env {
        // SAFETY: the test command runs its cases one at a time, on one thread.
        unsafe { std::env::set_var(key, fill_text(value)) };
    }
    let cwd = cases
        .setup
        .cwd
        .as_deref()
        .map(fill_text)
        .unwrap_or_else(|| root_win.clone());

    let mut failed = 0;
    for case in &cases.case {
        // A case for a rule that is not there would always be quiet.
        if !rules.rules.iter().any(|r| r.id == case.rule) && !rules.builtins.contains(&case.rule) {
            failed += 1;
            crate::outln!(
                "NG unknown (want {}) {}: no rule {} in {}",
                case.expect,
                case.name,
                case.rule,
                rules_dir.display()
            );
            continue;
        }
        let mut input = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "cwd": cwd,
            "tool_input": {},
        });
        if let Some(over) = &case.input {
            let over = serde_json::to_value(over).map_err(|e| e.to_string())?;
            merge(&mut input, fill(over, &root, &root_win));
        }
        if let Some(command) = &case.command {
            input["tool_input"]["command"] = json!(fill_text(command));
        }
        let parsed = Input::parse(&input.to_string()).ok_or("bad input")?;
        let found: Vec<_> = findings(&parsed, &rules)
            .into_iter()
            .filter(|f| f.rule == case.rule)
            .collect();
        let got = match found.first().map(|f| f.kind) {
            None => "quiet",
            Some(Kind::Context) => "context",
            Some(Kind::Deny) => "deny",
            Some(Kind::Block) => "block",
        };
        let has = |text: &String| found.iter().any(|f| f.message.contains(&fill_text(text)));
        let holds =
            case.contains.as_ref().is_none_or(has) && !case.excludes.as_ref().is_some_and(has);
        if got == case.expect && holds {
            crate::outln!("OK {got:<7} {}", case.name);
        } else {
            failed += 1;
            let messages: Vec<&str> = found.iter().map(|f| f.message.as_str()).collect();
            crate::outln!(
                "NG {got:<7} (want {}) {} {:?}",
                case.expect,
                case.name,
                messages
            );
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(failed)
}
