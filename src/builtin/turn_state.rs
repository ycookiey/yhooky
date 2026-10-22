//! turn-state (Stop, PostToolUse Monitor).
//!
//! Records what a turn left running, for a tool that tells whether Claude
//! now waits for the user or for its own background work (a plugin reads
//! the file the turn's end leaves). It never answers: no note, no block.
//!
//! - PostToolUse of Monitor: appends the watch's task id
//!   (`tool_response.taskId`) to `<session>.monitors`. A Monitor shows in
//!   Stop's `background_tasks` as a plain `shell` task, so this is the only
//!   way to tell it apart.
//! - Stop: writes `<session>.stop.json`:
//!   `{ "at": <ms since the epoch>, "tasks": [...], "dropped": [...], "crons": <n> }`.
//!   `tasks` are the `background_tasks` still running, less the shell
//!   commands that run until stopped (dev servers, watchers), which go to
//!   `dropped`: a resident command wakes nobody by finishing. A Monitor's
//!   task is never dropped, as the watch is how Claude waits. Tasks an
//!   `ignore_task` rule names (a watch that only a person's input wakes,
//!   such as the comments on a published page) go to `dropped` too.
//!   `crons` counts `session_crons` (ScheduleWakeup, CronCreate, /loop).
//!
//! Folder: `$TURN_STATE_DIR`, else `~/.claude/state/turn-state`.
//!
//! Resident commands are read from `~/.claude/hooks/turn-state.toml`, kept
//! out of the rule folder whose files take only rules:
//!
//! ```toml
//! [[resident]]
//! pattern = '(pnpm|npm|yarn|bun)(\s+run)?\s+dev(\s|$)'  # matched at a head's start
//! except = ['\bbuild\b']                               # none may match the head
//!
//! [[ignore_task]]
//! type = 'monitor'                                     # the task's type, as written
//! description = '^live updates for artifact '          # matched anywhere in it
//! ```
//!
//! A command's heads are its parts split at `&&`, `||`, `;`, `|` and line
//! ends outside quotes, each without leading `VAR=value`, `timeout <n>`,
//! `nohup`, `exec`, `time`, `(` and `{`; parts that only set up (`cd`,
//! `sleep`, `rm`, `echo`, ...) are left out. Patterns see the head as
//! written, quotes and all.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input::Input;
use crate::output::Finding;
use crate::text::windows_path;
use crate::{home, log};

static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"'[^']*'|"(?:[^"\\]|\\.)*""#).unwrap());
static SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&&|\|\||[;|\n]").unwrap());
static PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:[({]\s*|[A-Za-z_][A-Za-z0-9_]*=\S*\s+|timeout\s+\S+\s+|nohup\s+|exec\s+|time\s+)",
    )
    .unwrap()
});
static SETUP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:cd|Set-Location|sleep|Start-Sleep|rm|mkdir|echo|export|taskkill|true|[A-Za-z_][A-Za-z0-9_]*=\S*|\$\w+\s*=.*)(?:\s|$)",
    )
    .unwrap()
});

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    resident: Vec<ResidentDef>,
    #[serde(default)]
    ignore_task: Vec<IgnoreDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IgnoreDef {
    #[serde(rename = "type")]
    kind: String,
    description: String,
}

/// A background task that is not work Claude waits for: one of `kind`
/// whose description `description` matches.
pub struct IgnoreTask {
    kind: String,
    description: Regex,
}

impl IgnoreTask {
    pub fn new(kind: &str, description: &str) -> Result<Self, regex::Error> {
        Ok(Self {
            kind: kind.to_string(),
            description: Regex::new(description)?,
        })
    }

    fn matches(&self, kind: &str, description: &str) -> bool {
        self.kind == kind && self.description.is_match(description)
    }
}

/// What `~/.claude/hooks/turn-state.toml` says to leave out of `tasks`.
#[derive(Default)]
pub struct Config {
    pub resident: Vec<Resident>,
    pub ignore: Vec<IgnoreTask>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResidentDef {
    pattern: String,
    #[serde(default)]
    except: Vec<String>,
}

/// A command that runs until stopped: a head that `pattern` matches from
/// its start and no `except` matches anywhere.
pub struct Resident {
    pattern: Regex,
    except: Vec<Regex>,
}

impl Resident {
    pub fn new(pattern: &str, except: &[&str]) -> Result<Self, regex::Error> {
        Ok(Self {
            pattern: Regex::new(&format!("^(?:{pattern})"))?,
            except: except
                .iter()
                .map(|e| Regex::new(e))
                .collect::<Result<_, _>>()?,
        })
    }

    fn matches(&self, head: &str) -> bool {
        self.pattern.is_match(head) && !self.except.iter().any(|e| e.is_match(head))
    }
}

/// The parts of `command` that run a program, as the module doc says.
pub fn heads(command: &str) -> Vec<&str> {
    all_heads(command)
        .into_iter()
        .filter(|head| !SETUP.is_match(head))
        .collect()
}

/// The parts of `command`, split and stripped as for [`heads`], with the
/// parts that only set up kept.
pub fn all_heads(command: &str) -> Vec<&str> {
    // Quoted text keeps its length so the separators found in the masked
    // copy cut the command at the same places.
    let masked = QUOTED.replace_all(command, |c: &regex::Captures| "x".repeat(c[0].len()));
    let mut parts = Vec::new();
    let mut start = 0;
    for sep in SEP.find_iter(&masked) {
        parts.push(&command[start..sep.start()]);
        start = sep.end();
    }
    parts.push(&command[start..]);
    parts
        .into_iter()
        .filter_map(|part| {
            let mut head = part.trim().trim_end_matches('&').trim_end();
            while let Some(m) = PREFIX.find(head) {
                head = head[m.end()..].trim_start();
            }
            (!head.is_empty()).then_some(head)
        })
        .collect()
}

pub fn is_resident(command: &str, rules: &[Resident]) -> bool {
    heads(command)
        .iter()
        .any(|head| rules.iter().any(|rule| rule.matches(head)))
}

fn config_path() -> Option<PathBuf> {
    home().map(|h| h.join(".claude").join("hooks").join("turn-state.toml"))
}

/// The rules in `path`; none, with a line in the log, when the file is
/// there but cannot be used. A rule whose pattern does not compile is left
/// out, with a line in the log.
fn load_config(path: &Path) -> Config {
    let Ok(text) = fs::read_to_string(path) else {
        return Config::default();
    };
    let parsed = match toml::from_str::<ConfigFile>(&text) {
        Ok(parsed) => parsed,
        Err(why) => {
            log(&format!("turn-state: {}: {why}", path.display()));
            return Config::default();
        }
    };
    let resident = parsed
        .resident
        .iter()
        .filter_map(|def| {
            let except: Vec<&str> = def.except.iter().map(String::as_str).collect();
            Resident::new(&def.pattern, &except)
                .map_err(|why| log(&format!("turn-state: {}: {why}", def.pattern)))
                .ok()
        })
        .collect();
    let ignore = parsed
        .ignore_task
        .iter()
        .filter_map(|def| {
            IgnoreTask::new(&def.kind, &def.description)
                .map_err(|why| log(&format!("turn-state: {}: {why}", def.description)))
                .ok()
        })
        .collect();
    Config { resident, ignore }
}

fn state_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("TURN_STATE_DIR") {
        return Some(PathBuf::from(windows_path(&dir)));
    }
    home().map(|h| h.join(".claude").join("state").join("turn-state"))
}

/// A session id as a file name: anything but letters, digits, `-` and `_`
/// becomes `_`.
fn file_stem(session: &str) -> String {
    session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn monitors(dir: &Path, stem: &str) -> HashSet<String> {
    fs::read_to_string(dir.join(format!("{stem}.monitors")))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Appends the Monitor's task id to `<stem>.monitors` in `dir`.
pub fn record_monitor(input: &Input, dir: &Path, stem: &str) -> std::io::Result<()> {
    let id = input.text("tool_response.taskId");
    if id.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{stem}.monitors")))?;
    writeln!(file, "{id}")
}

/// What Stop's input says is still running, split into the tasks that can
/// wake the session and the resident commands that cannot.
pub fn stop_state(input: &Input, watches: &HashSet<String>, config: &Config) -> Value {
    let mut tasks = Vec::new();
    let mut dropped = Vec::new();
    let empty = Vec::new();
    let running = input
        .get("background_tasks")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    for task in running {
        let text = |key: &str| task.get(key).and_then(Value::as_str).unwrap_or_default();
        let entry = json!({
            "id": text("id"),
            "type": text("type"),
            "description": text("description"),
            "command": text("command"),
        });
        let is_watch = watches.contains(text("id"));
        let is_resident_shell =
            text("type") == "shell" && !is_watch && is_resident(text("command"), &config.resident);
        let is_ignored = config
            .ignore
            .iter()
            .any(|rule| rule.matches(text("type"), text("description")));
        if is_resident_shell || is_ignored {
            dropped.push(entry);
        } else {
            tasks.push(entry);
        }
    }
    let crons = input
        .get("session_crons")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    json!({ "at": now_ms() as u64, "tasks": tasks, "dropped": dropped, "crons": crons })
}

/// Writes `<stem>.stop.json` in `dir` whole: a reader never sees half.
pub fn write_stop(state: &Value, dir: &Path, stem: &str) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{stem}.stop.json.tmp"));
    fs::write(&tmp, state.to_string())?;
    fs::rename(&tmp, dir.join(format!("{stem}.stop.json")))
}

pub fn run(input: &Input) -> Vec<Finding> {
    let event = input.event();
    let is_monitor = event == "PostToolUse" && input.tool() == "Monitor";
    if event != "Stop" && !is_monitor {
        return Vec::new();
    }
    let session = input.text("session_id");
    let Some(dir) = state_dir().filter(|_| !session.is_empty()) else {
        return Vec::new();
    };
    let stem = file_stem(&session);
    let written = if is_monitor {
        record_monitor(input, &dir, &stem)
    } else {
        let config = config_path().map(|p| load_config(&p)).unwrap_or_default();
        write_stop(
            &stop_state(input, &monitors(&dir, &stem), &config),
            &dir,
            &stem,
        )
    };
    if let Err(why) = written {
        log(&format!("turn-state: {}: {why}", dir.display()));
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Vec<Resident> {
        vec![
            Resident::new(
                r"(pnpm|npm|yarn|bun)(\s+(run|exec|-C\s+\S+|--filter\s+\S+))*\s+(dev(:\S+)?|storybook(:\S+)?)(\s|$)",
                &[r"\bbuild\b"],
            )
            .unwrap(),
            Resident::new(r"(bash\s+)?(\S*/)?scripts/dev[\w-]*\.sh(\s|$)", &[r"\.sh\s+-[rs](\s|$)"]).unwrap(),
            Resident::new(r"\S+\s+(@[A-Za-z][\w.-]*(\s|$)|-avd\s)", &[]).unwrap(),
            Resident::new(r"uv\s+run\s.*(live-feed|feed_server)", &[]).unwrap(),
        ]
    }

    #[test]
    fn heads_skip_setup_and_prefixes() {
        assert_eq!(
            heads(r#"cd "C:\a b" && PORT=3 timeout 90 pnpm dev 2>&1 | tail -5"#),
            vec!["pnpm dev 2>&1", "tail -5"]
        );
        assert_eq!(
            heads("sleep 3; ( npx vite --port 5 )"),
            vec!["npx vite --port 5 )"]
        );
    }

    #[test]
    fn heads_do_not_split_inside_quotes() {
        let command =
            r#"powershell -Command "Get-Process | Where-Object { $_.Name -like '*next dev*' }""#;
        assert_eq!(heads(command), vec![command]);
    }

    #[test]
    fn resident_commands() {
        let rules = rules();
        for command in [
            "cd /x && pnpm dev:web 2>&1",
            "pnpm --filter @a/web storybook",
            "bash scripts/dev.sh 2>&1",
            r#"EMULATOR=~/e.exe ; "$EMULATOR" @Pixel_9_Pro -no-snapshot-load"#,
            r#"uv run --with watchdog "/c/u/tools/live-feed/scripts/feed_server.py""#,
            r#"timeout 600 uv run --python 3.12 "/c/live-feed/x.py""#,
        ] {
            assert!(is_resident(command, &rules), "{command}");
        }
        for command in [
            "gh run watch 1 --exit-status > /dev/null 2>&1",
            "bash scripts/verify.sh --no-emu --no-storybook 2>&1",
            "bash scripts/dev.sh -r 2>&1 | tail -6",
            "pnpm -C packages/web exec storybook build > x",
            r#"powershell -Command "Get-Process | Where-Object { $_ -like '*pnpm dev*' }""#,
            r#"uv run "/c/u/tools/convert/scripts/convert.py" a.csv"#,
            "npx -y @cocal/google-calendar-mcp auth",
        ] {
            assert!(!is_resident(command, &rules), "{command}");
        }
    }

    #[test]
    fn stop_keeps_watches_and_drops_residents() {
        let input = Input::parse(
            r#"{"hook_event_name":"Stop","session_id":"s","background_tasks":[
                {"id":"a","type":"shell","status":"running","description":"dev","command":"pnpm dev"},
                {"id":"b","type":"shell","status":"running","description":"build","command":"pnpm build"},
                {"id":"c","type":"shell","status":"running","description":"tick","command":"pnpm dev"},
                {"id":"d","type":"subagent","status":"running","description":"research"},
                {"id":"e","type":"monitor","status":"running","description":"live updates for artifact https://x (re-armed)"},
                {"id":"f","type":"monitor","status":"running","description":"deploy log"}
            ],"session_crons":[{"id":"x"}]}"#,
        )
        .unwrap();
        let watches: HashSet<String> = ["c".to_string()].into();
        let config = Config {
            resident: rules(),
            ignore: vec![IgnoreTask::new("monitor", "^live updates for artifact ").unwrap()],
        };
        let state = stop_state(&input, &watches, &config);
        let ids = |key: &str| -> Vec<String> {
            state[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["id"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(ids("tasks"), vec!["b", "c", "d", "f"]);
        assert_eq!(ids("dropped"), vec!["a", "e"]);
        assert_eq!(state["crons"], 1);
    }

    #[test]
    fn files_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = Input::parse(
            r#"{"hook_event_name":"PostToolUse","tool_name":"Monitor","session_id":"s/1","tool_response":{"taskId":"m1","persistent":false}}"#,
        )
        .unwrap();
        let stem = file_stem("s/1");
        assert_eq!(stem, "s_1");
        record_monitor(&monitor, dir.path(), &stem).unwrap();
        assert!(monitors(dir.path(), &stem).contains("m1"));
        write_stop(&json!({"at": 1, "tasks": []}), dir.path(), &stem).unwrap();
        let text = fs::read_to_string(dir.path().join("s_1.stop.json")).unwrap();
        assert_eq!(text, r#"{"at":1,"tasks":[]}"#);
    }
}
