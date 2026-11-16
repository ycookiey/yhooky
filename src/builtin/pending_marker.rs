//! pending-marker-reminder (UserPromptSubmit).
//!
//! Reminds a session of work it left pending, on every prompt, until the
//! work is done. The work is written down by a script the session runs, as
//! a marker: a JSON file in `.claude/hooks/pending/` of the project (the
//! folder the project's rules are read from, see `find_project`). The script
//! removes the marker when the work is done. A marker holds:
//!
//! - `remind`: the note for the session that left the marker, added on
//!   every prompt. A marker without it is skipped
//! - `session_id`: that session (a script reads it from
//!   `CLAUDE_CODE_SESSION_ID`)
//! - `transcript`: that session's transcript, whose last change tells
//!   whether the session is still going (optional)
//! - `orphan`: the note for other sessions once the marker looks left
//!   behind (optional; without it, other sessions are never told)
//! - `stale_hours`: how long that session may stay quiet before the marker
//!   looks left behind (default 3)
//!
//! Other sessions get nothing while the session that left the marker is
//! going: the work is that session's, and doing it from another session
//! would do it twice. A marker looks left behind when its transcript (else
//! the marker itself) has not changed for `stale_hours`, or when it names no
//! session; then each other session gets `orphan` once (kept under
//! `$PENDING_MARKER_STATE_DIR`, else `claude-pending-marker` in the
//! temporary folder).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::input::Input;
use crate::once;
use crate::output::{Finding, Kind};
use crate::text::windows_path;

const ID: &str = "pending-marker-reminder";
const DEFAULT_STALE_HOURS: f64 = 3.0;

/// The markers in `project`, by file name.
fn markers(project: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(project.join(".claude").join("hooks").join("pending")) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn text<'a>(marker: &'a Value, key: &str) -> &'a str {
    marker.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Whether the session that left `marker` (at `file`) has been quiet for
/// longer than the marker allows, as of `now`.
fn left_behind(marker: &Value, file: &Path, now: SystemTime) -> bool {
    let transcript = text(marker, "transcript");
    let last = (!transcript.is_empty())
        .then(|| modified(Path::new(&windows_path(transcript))))
        .flatten()
        .or_else(|| modified(file));
    let Some(last) = last else { return true };
    let hours = marker
        .get("stale_hours")
        .and_then(Value::as_f64)
        .filter(|h| *h >= 0.0)
        .unwrap_or(DEFAULT_STALE_HOURS);
    now.duration_since(last).unwrap_or(Duration::ZERO) > Duration::from_secs_f64(hours * 3600.0)
}

/// The notes for `session` from the markers in `project`, as of `now`;
/// which sessions were told of a marker left behind is kept in `state`.
pub fn check(project: &Path, session: &str, now: SystemTime, state: &Path) -> Vec<Finding> {
    let mut found = Vec::new();
    for file in markers(project) {
        let Some(marker) = fs::read_to_string(&file)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        else {
            continue;
        };
        let remind = text(&marker, "remind");
        if remind.is_empty() {
            continue;
        }
        let owner = text(&marker, "session_id");
        if !owner.is_empty() && owner == session {
            found.push(Finding::new(ID, Kind::Context, remind));
            continue;
        }
        if !owner.is_empty() && !left_behind(&marker, &file, now) {
            continue;
        }
        let orphan = text(&marker, "orphan");
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if !orphan.is_empty()
            && !session.is_empty()
            && once::first_time(&state.to_path_buf(), &format!("{session}\n{name}"))
        {
            found.push(Finding::new(ID, Kind::Context, orphan));
        }
    }
    found
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "UserPromptSubmit" {
        return Vec::new();
    }
    let Some(project) = crate::project_dir() else {
        return Vec::new();
    };
    let state = once::state_dir("PENDING_MARKER_STATE_DIR", "claude-pending-marker");
    check(
        &project,
        &input.text("session_id"),
        SystemTime::now(),
        &state,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _root: tempfile::TempDir,
        project: PathBuf,
        state: PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".claude").join("hooks").join("pending")).unwrap();
        let state = root.path().join("state");
        Fixture {
            _root: root,
            project,
            state,
        }
    }

    impl Fixture {
        fn marker(&self, name: &str, marker: Value) {
            let path = self.project.join(".claude/hooks/pending").join(name);
            fs::write(path, marker.to_string()).unwrap();
        }

        fn transcript(&self, name: &str) -> String {
            let path = self.project.join(name);
            fs::write(&path, "").unwrap();
            path.to_string_lossy().replace('\\', "/")
        }

        fn notes(&self, session: &str, now: SystemTime) -> Vec<String> {
            check(&self.project, session, now, &self.state)
                .into_iter()
                .map(|f| f.message)
                .collect()
        }
    }

    fn hours(n: u64) -> Duration {
        Duration::from_secs(n * 3600)
    }

    #[test]
    fn reminds_the_session_that_left_it_every_time() {
        let f = fixture();
        let transcript = f.transcript("a.jsonl");
        f.marker(
            "m.json",
            serde_json::json!({"session_id": "a", "transcript": transcript,
                               "remind": "mine", "orphan": "left"}),
        );
        let now = SystemTime::now();
        assert_eq!(f.notes("a", now), ["mine"]);
        assert_eq!(f.notes("a", now), ["mine"]);
        // Even long after: the session that left it is the one to finish it.
        assert_eq!(f.notes("a", now + hours(10)), ["mine"]);
    }

    #[test]
    fn says_nothing_to_others_while_that_session_goes_on() {
        let f = fixture();
        let transcript = f.transcript("a.jsonl");
        f.marker(
            "m.json",
            serde_json::json!({"session_id": "a", "transcript": transcript,
                               "remind": "mine", "orphan": "left"}),
        );
        assert!(f.notes("b", SystemTime::now() + hours(2)).is_empty());
    }

    #[test]
    fn tells_each_other_session_once_when_left_behind() {
        let f = fixture();
        let transcript = f.transcript("a.jsonl");
        f.marker(
            "m.json",
            serde_json::json!({"session_id": "a", "transcript": transcript,
                               "remind": "mine", "orphan": "left"}),
        );
        let later = SystemTime::now() + hours(4);
        assert_eq!(f.notes("b", later), ["left"]);
        assert!(f.notes("b", later).is_empty());
        assert_eq!(f.notes("c", later), ["left"]);
    }

    #[test]
    fn takes_stale_hours_from_the_marker() {
        let f = fixture();
        let transcript = f.transcript("a.jsonl");
        f.marker(
            "m.json",
            serde_json::json!({"session_id": "a", "transcript": transcript,
                               "remind": "mine", "orphan": "left", "stale_hours": 0.5}),
        );
        assert_eq!(f.notes("b", SystemTime::now() + hours(1)), ["left"]);
    }

    #[test]
    fn without_a_transcript_goes_by_the_marker_itself() {
        let f = fixture();
        f.marker(
            "m.json",
            serde_json::json!({"session_id": "a", "transcript": "/no/such/file.jsonl",
                               "remind": "mine", "orphan": "left"}),
        );
        let now = SystemTime::now();
        assert!(f.notes("b", now).is_empty());
        assert_eq!(f.notes("b", now + hours(4)), ["left"]);
    }

    #[test]
    fn a_marker_naming_no_session_looks_left_behind() {
        let f = fixture();
        f.marker(
            "m.json",
            serde_json::json!({"remind": "mine", "orphan": "left"}),
        );
        assert_eq!(f.notes("b", SystemTime::now()), ["left"]);
    }

    #[test]
    fn skips_markers_without_remind_or_orphan_and_other_files() {
        let f = fixture();
        f.marker("no-remind.json", serde_json::json!({"orphan": "left"}));
        f.marker("no-orphan.json", serde_json::json!({"remind": "mine"}));
        f.marker(
            "note.txt",
            serde_json::json!({"remind": "mine", "orphan": "left"}),
        );
        fs::write(f.project.join(".claude/hooks/pending/broken.json"), "{").unwrap();
        assert!(f.notes("b", SystemTime::now()).is_empty());
    }

    #[test]
    fn nothing_without_the_folder() {
        let root = tempfile::tempdir().unwrap();
        let none = check(root.path(), "a", SystemTime::now(), &root.path().join("s"));
        assert!(none.is_empty());
    }
}
