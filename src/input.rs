//! The JSON a hook gets on stdin.

use serde_json::Value;

pub struct Input {
    pub raw: String,
    pub json: Value,
}

impl Input {
    pub fn parse(raw: &str) -> Option<Self> {
        let json = serde_json::from_str(raw).ok()?;
        Some(Self {
            raw: raw.to_string(),
            json,
        })
    }

    /// The value at a dotted path (`tool_input.command`), if present.
    pub fn get(&self, path: &str) -> Option<&Value> {
        path.split('.')
            .try_fold(&self.json, |value, key| value.get(key))
    }

    /// The value at `path` as `jq -r '.<path> // ""'` prints it: a string as
    /// it is, `null`, `false` or a missing value as "", anything else as JSON.
    pub fn text(&self, path: &str) -> String {
        match self.get(path) {
            None | Some(Value::Null) | Some(Value::Bool(false)) => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        }
    }

    /// The first of `paths` that is not empty, as `.a // .b // ""` gives.
    pub fn first_text(&self, paths: &[String]) -> String {
        paths
            .iter()
            .map(|path| self.text(path))
            .find(|text| !text.is_empty())
            .unwrap_or_default()
    }

    pub fn event(&self) -> String {
        self.text("hook_event_name")
    }

    pub fn tool(&self) -> String {
        self.text("tool_name")
    }

    pub fn cwd(&self) -> String {
        self.text("cwd")
    }

    pub fn command(&self) -> String {
        self.text("tool_input.command")
    }

    /// The text a tool printed, from `tool_response`: its string fields
    /// joined by lines when it is an object, as Bash's result differs by
    /// version.
    pub fn response_text(&self, fields: &[&str]) -> String {
        match self.get("tool_response") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(Value::Object(map)) => fields
                .iter()
                .filter_map(|field| map.get(*field).and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
            Some(other) => other.to_string(),
        }
    }
}
