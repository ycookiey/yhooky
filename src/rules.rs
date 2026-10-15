//! Rules written as data: "when this field of the input matches these
//! patterns, add this note or refuse". Read from every `*.toml` in the rule
//! folders, in file name order.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Deserialize;

use crate::input::Input;
use crate::once;
use crate::output::{Finding, Kind};
use crate::repo;
use crate::text::{any_line, comparable_path};

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    /// Built-in checks (src/builtin) to run, by name.
    #[serde(default)]
    pub builtins: Vec<String>,
    #[serde(default)]
    pub rule: Vec<RuleDef>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    pub id: String,
    /// The hook event: PreToolUse, PostToolUse, Stop, ...
    pub event: String,
    /// A pattern the tool's name must match (PreToolUse, PostToolUse).
    pub tools: Option<String>,
    /// The fields to read, as dotted paths; the first that is not empty.
    pub fields: Vec<String>,
    /// Changes to the text before matching: `slashes` turns `\` into `/`,
    /// `squeeze-slashes` turns runs of `/` into one.
    #[serde(default)]
    pub transform: Vec<String>,
    /// Patterns that must each match some line.
    #[serde(default)]
    pub all: Vec<String>,
    /// Patterns none of which may match any line.
    #[serde(default)]
    pub none: Vec<String>,
    /// Fields that must have these values, compared as `jq -r` prints them.
    #[serde(default)]
    pub equals: BTreeMap<String, String>,
    /// Folders the command must work in (repo::dir, `git -C` first).
    #[serde(default)]
    pub repo_dir_in: Vec<String>,
    /// An environment variable that, when set, replaces `repo_dir_in`: the
    /// folders separated by spaces.
    pub repo_dir_in_env: Option<String>,
    /// A pattern the `origin` URL of that folder must match.
    pub repo_remote: Option<String>,
    /// context, deny, deny-once (deny the first time per session and text,
    /// a note after) or block.
    pub action: String,
    /// The text. `{repo_dir}` is the folder; `{repo_name}` the last part of
    /// its `origin` URL, without `.git`.
    pub message: String,
    /// Of the rules sharing a group, only the first that matches counts.
    pub group: Option<String>,
}

#[derive(Default)]
pub struct Rules {
    pub builtins: Vec<String>,
    pub rules: Vec<RuleDef>,
    /// Files and rules that could not be read, with why.
    pub problems: Vec<String>,
}

impl Rules {
    /// A folder that does not exist is skipped: a project need not have
    /// rules. An empty path is a problem, though reading it fails the same
    /// way, and so is any other failure.
    pub fn load(dirs: &[PathBuf]) -> Self {
        let mut rules = Rules::default();
        for dir in dirs {
            if dir.as_os_str().is_empty() {
                rules.problems.push("a rule folder is an empty path".into());
                continue;
            }
            let entries = match fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    rules.problems.push(format!("{}: {e}", dir.display()));
                    continue;
                }
            };
            let mut files: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect();
            files.sort();
            for file in files {
                rules.add_file(&file);
            }
        }
        rules
    }

    fn add_file(&mut self, file: &Path) {
        let parsed = fs::read_to_string(file)
            .map_err(|e| e.to_string())
            .and_then(|text| toml::from_str::<RuleFile>(&text).map_err(|e| e.to_string()));
        match parsed {
            Ok(parsed) => {
                for name in parsed.builtins {
                    if !self.builtins.contains(&name) {
                        self.builtins.push(name);
                    }
                }
                for rule in parsed.rule {
                    match check(&rule) {
                        Ok(()) => self.rules.push(rule),
                        Err(why) => {
                            self.problems
                                .push(format!("{}: {}: {why}", file.display(), rule.id))
                        }
                    }
                }
            }
            Err(why) => self.problems.push(format!("{}: {why}", file.display())),
        }
    }

    /// What the rules for `input`'s event find, in order.
    pub fn run(&self, input: &Input) -> Vec<Finding> {
        let event = input.event();
        let mut found = Vec::new();
        let mut groups_done: Vec<&str> = Vec::new();
        let repo = Repo::default();
        for rule in self.rules.iter().filter(|r| r.event == event) {
            if rule
                .group
                .as_deref()
                .is_some_and(|g| groups_done.contains(&g))
            {
                continue;
            }
            if let Some(finding) = evaluate(rule, input, &repo) {
                if let Some(group) = &rule.group {
                    groups_done.push(group);
                }
                found.push(finding);
            }
        }
        found
    }
}

/// A rule's patterns compile and its action is known.
fn check(rule: &RuleDef) -> Result<(), String> {
    for pattern in rule
        .tools
        .iter()
        .chain(&rule.all)
        .chain(&rule.none)
        .chain(rule.repo_remote.iter())
    {
        Regex::new(pattern).map_err(|e| e.to_string())?;
    }
    kind(&rule.action).ok_or_else(|| format!("unknown action {}", rule.action))?;
    for t in &rule.transform {
        if t != "slashes" && t != "squeeze-slashes" {
            return Err(format!("unknown transform {t}"));
        }
    }
    if rule.fields.is_empty() {
        return Err("no fields".into());
    }
    Ok(())
}

fn kind(action: &str) -> Option<Kind> {
    match action {
        "context" => Some(Kind::Context),
        // Deny the first time, a note after (see `evaluate`).
        "deny" | "deny-once" => Some(Kind::Deny),
        "block" => Some(Kind::Block),
        _ => None,
    }
}

/// For `deny-once`: whether this session meets this rule with this text for
/// the first time. A note reaches Claude only with the tool's result, so a
/// rule about something that cannot be taken back (a message sent) stops
/// the first call instead, and lets the same call through when sent again.
fn first_meeting(rule: &RuleDef, input: &Input, text: &str) -> bool {
    let dir = once::state_dir("RULE_ONCE_STATE_DIR", "claude-rule-once");
    let session = input.text("session_id");
    once::first_time(&dir, &format!("{session}\n{}\n{text}", rule.id))
}

/// The folder and remote, found once and only when a rule needs them.
#[derive(Default)]
struct Repo {
    dir: OnceCell<String>,
    origin: OnceCell<String>,
}

impl Repo {
    fn dir(&self, input: &Input) -> &str {
        self.dir.get_or_init(|| repo::dir(input, repo::First::GitC))
    }

    fn origin(&self, input: &Input) -> &str {
        self.origin.get_or_init(|| repo::origin(self.dir(input)))
    }
}

fn is_match(pattern: &str, text: &str) -> bool {
    Regex::new(pattern).is_ok_and(|re| any_line(&re, text))
}

fn evaluate(rule: &RuleDef, input: &Input, repo: &Repo) -> Option<Finding> {
    if let Some(tools) = &rule.tools
        && !Regex::new(tools).is_ok_and(|re| re.is_match(&input.tool()))
    {
        return None;
    }
    for (field, want) in &rule.equals {
        if input.text(field) != *want {
            return None;
        }
    }
    let mut text = input.first_text(&rule.fields);
    for t in &rule.transform {
        text = match t.as_str() {
            "slashes" => text.replace('\\', "/"),
            _ => squeeze_slashes(&text),
        };
    }
    if !rule.all.iter().all(|p| is_match(p, &text)) || rule.none.iter().any(|p| is_match(p, &text))
    {
        return None;
    }
    let from_env = rule
        .repo_dir_in_env
        .as_ref()
        .and_then(|name| std::env::var(name).ok());
    let dirs: Vec<String> = match &from_env {
        Some(list) => list.split_whitespace().map(String::from).collect(),
        None => rule.repo_dir_in.clone(),
    };
    if !dirs.is_empty() {
        let here = comparable_path(repo.dir(input));
        if !dirs.iter().any(|d| comparable_path(d) == here) {
            return None;
        }
    }
    let mut repo_name = String::new();
    if let Some(pattern) = &rule.repo_remote {
        let origin = repo.origin(input);
        if !Regex::new(pattern).is_ok_and(|re| re.is_match(origin)) {
            return None;
        }
        let last = origin.rsplit('/').next().unwrap_or(origin);
        repo_name = last.strip_suffix(".git").unwrap_or(last).to_string();
    }
    let mut message = rule.message.clone();
    if message.contains("{repo_dir}") {
        message = message.replace("{repo_dir}", repo.dir(input));
    }
    message = message.replace("{repo_name}", &repo_name);
    if rule.action == "deny-once" {
        return Some(if first_meeting(rule, input, &text) {
            Finding::new(
                &rule.id,
                Kind::Deny,
                format!("{message}(一度だけ止めた。このまま進めるなら、同じ操作を出し直せば通る)"),
            )
        } else {
            Finding::new(&rule.id, Kind::Context, message)
        });
    }
    Some(Finding::new(&rule.id, kind(&rule.action)?, message))
}

fn squeeze_slashes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if !(c == '/' && out.ends_with('/')) {
            out.push(c);
        }
    }
    out
}
