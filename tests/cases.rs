//! Runs every case file in tests/rules/tests against the built-in checks.
//! One test, so the cases' environment variables are set one file at a
//! time.

use std::path::Path;

#[test]
fn builtin_cases() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("rules")
        .join("tests");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let mut failed = 0;
    for file in files {
        println!("== {}", file.display());
        failed += yhooky::cases::run(&file).unwrap_or_else(|why| panic!("{why}"));
    }
    assert_eq!(failed, 0, "{failed} case(s) failed");
}

#[test]
fn language_check_lists_the_turns() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.jsonl");
    std::fs::write(
        &file,
        r#"{"type":"user","message":{"content":"次の作業へ"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Checking the files first, then the tests."}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Step two is committed. Next I will write the admin login screen and its list."}]}}
"#,
    )
    .unwrap();
    assert_eq!(
        yhooky::builtin::language::check(&file),
        vec![
            "turn 2: Step two is committed. Next I will write the admin login screen and its list."
        ]
    );
}

#[test]
fn stage_own_edit_stops_the_first_edit_of_a_changed_file() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        )
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    for file in ["sub/dirty.md", "clean.md", "begun.md", "stale.md"] {
        std::fs::write(repo.join(file), "one\n").unwrap();
    }
    git(&["add", "sub/dirty.md", "clean.md", "begun.md", "stale.md"]);
    git(&["commit", "-q", "-m", "x"]);
    std::fs::write(repo.join("sub/dirty.md"), "two\n").unwrap();
    std::fs::write(repo.join("begun.md"), "two\n").unwrap();
    std::fs::write(repo.join("stale.md"), "two\n").unwrap();
    std::fs::write(repo.join("untracked.md"), "one\n").unwrap();
    let store = repo.join(".git").join("stage-own-edit");
    std::fs::create_dir_all(&store).unwrap();
    let copy = |path: &str| store.join(sha1_smol::Sha1::from(path).digest().to_string());
    // A copy kept by `git stage-own-edit begin begun.md` just now.
    std::fs::write(copy("begun.md"), "two\n").unwrap();
    // A copy left by an earlier `begin` whose `stage` never ran, from
    // before the changes now in the file.
    std::fs::write(copy("stale.md"), "one\n").unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("tmp"), "x").unwrap();

    let run = |path: std::path::PathBuf, session: &str| {
        let raw = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Edit",
            "session_id": format!("{session}-{}", std::process::id()),
            "tool_input": { "file_path": path.to_string_lossy() },
        });
        let input = yhooky::input::Input::parse(&raw.to_string()).unwrap();
        yhooky::builtin::stage_own_edit::run(&input)
    };
    let first = run(repo.join("sub/dirty.md"), "s1");
    assert_eq!(first.len(), 1, "dirty tracked file");
    assert!(first[0].message.starts_with("sub/dirty.md には"));
    assert!(
        matches!(first[0].kind, yhooky::output::Kind::Deny),
        "stopped, not only noted"
    );
    assert!(
        run(repo.join("sub/dirty.md"), "s1").is_empty(),
        "same session again"
    );
    assert_eq!(
        run(repo.join("sub/dirty.md"), "s2").len(),
        1,
        "other session"
    );
    assert!(
        run(repo.join("clean.md"), "s1").is_empty(),
        "clean tracked file"
    );
    // s1's edit changes clean.md: its next edit is its own second edit.
    std::fs::write(repo.join("clean.md"), "two\n").unwrap();
    assert!(
        run(repo.join("clean.md"), "s1").is_empty(),
        "a file this session changed itself"
    );
    assert_eq!(
        run(repo.join("clean.md"), "s3").len(),
        1,
        "the same file for a session that never saw it clean"
    );
    assert!(
        run(repo.join("untracked.md"), "s1").is_empty(),
        "untracked file"
    );
    assert!(run(repo.join("new.md"), "s1").is_empty(), "new file");
    assert!(
        run(repo.join("begun.md"), "s1").is_empty(),
        "copy already kept"
    );
    // s1's edit changes begun.md away from the copy.
    std::fs::write(repo.join("begun.md"), "three\n").unwrap();
    assert!(
        run(repo.join("begun.md"), "s1").is_empty(),
        "the next edit after the one the copy let through"
    );
    let other = run(repo.join("begun.md"), "s2");
    assert_eq!(other.len(), 1, "another session after the begun edit");
    let stale = run(repo.join("stale.md"), "s1");
    assert_eq!(stale.len(), 1, "a copy that no longer matches the file");
    assert!(
        stale[0]
            .message
            .contains("store にある写しは今の file と違う"),
        "says the copy is stale: {}",
        stale[0].message
    );
    assert!(
        run(repo.join("stale.md"), "s1").is_empty(),
        "same session again after the stop"
    );
    assert!(
        run(outside.path().join("tmp"), "s1").is_empty(),
        "outside a repository"
    );
}

#[test]
fn push_check_stops_the_first_push_per_session_repo_and_head() {
    use std::path::Path;
    use std::process::Command;
    let state = tempfile::tempdir().unwrap();
    // SAFETY: no other test reads this variable.
    unsafe { std::env::set_var("PUSH_CHECK_STATE_DIR", state.path()) };
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let other = root.path().join("other");
    let plain = root.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let commit = |dir: &Path| {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["commit", "-q", "--allow-empty", "-m", "x"])
            .status()
            .unwrap()
            .success();
        assert!(ok);
    };
    for dir in [&repo, &other] {
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(dir)
                .status()
                .unwrap()
                .success()
        );
        commit(dir);
    }
    let p = |path: &Path| {
        path.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/")
    };
    let check = |command: String, session: &str, cwd: &str| -> &'static str {
        let raw = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "session_id": session,
            "cwd": cwd,
            "tool_input": { "command": command },
        });
        let input = yhooky::input::Input::parse(&raw.to_string()).unwrap();
        if yhooky::builtin::push_check::run(&input).is_empty() {
            "pass"
        } else {
            "deny"
        }
    };
    let (r, o, pl, top) = (p(&repo), p(&other), p(&plain), p(root.path()));
    let cases = [
        (
            "not a push",
            format!("cd {r} && git status"),
            "s1",
            &pl,
            "pass",
        ),
        (
            "push as a word",
            format!("cd {r} && echo pushed"),
            "s1",
            &pl,
            "pass",
        ),
        (
            "first push",
            format!("cd {r} && git push origin HEAD:main"),
            "s1",
            &pl,
            "deny",
        ),
        (
            "same HEAD again",
            format!("cd {r} && git push origin HEAD:main 2>&1 | tail -1"),
            "s1",
            &pl,
            "pass",
        ),
        (
            "same HEAD with -C",
            format!("git -C {r} push"),
            "s1",
            &pl,
            "pass",
        ),
        (
            "other session",
            format!("cd {r} && git push"),
            "s2",
            &pl,
            "deny",
        ),
        ("other repo", format!("git -C {o} push"), "s1", &pl, "deny"),
        (
            "relative cd",
            "cd other && git push".to_string(),
            "s1",
            &top,
            "pass",
        ),
    ];
    for (name, command, session, cwd, want) in cases {
        assert_eq!(check(command, session, cwd), want, "{name}");
    }
    commit(&repo);
    let cases = [
        ("new HEAD", format!("cd {r} && git push"), "deny"),
        ("new HEAD again", format!("cd {r} && git push"), "pass"),
        ("no repository", format!("cd {pl} && git push"), "deny"),
        (
            "no repository again",
            format!("cd {pl} && git push"),
            "pass",
        ),
    ];
    for (name, command, want) in cases {
        assert_eq!(check(command, "s1", &pl), want, "{name}");
    }
}

#[test]
fn a_case_for_a_rule_not_there_fails() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tests")).unwrap();
    std::fs::write(
        dir.path().join("rules.toml"),
        r#"[[rule]]
id = "echo-note"
event = "PreToolUse"
fields = ["tool_input.command"]
all = ['^echo']
action = "context"
message = "note"
"#,
    )
    .unwrap();
    let file = dir.path().join("tests").join("x.toml");
    std::fs::write(
        &file,
        r#"[[case]]
name = "known"
rule = "echo-note"
expect = "context"
command = "echo hi"

[[case]]
name = "unknown"
rule = "no-such-rule"
expect = "quiet"
command = "echo hi"
"#,
    )
    .unwrap();
    assert_eq!(yhooky::cases::run(&file), Ok(1));
}

#[test]
fn a_case_file_beside_no_rules_is_a_problem() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tests")).unwrap();
    let file = dir.path().join("tests").join("x.toml");
    std::fs::write(
        &file,
        r#"[[case]]
name = "quiet"
rule = "no-such-rule"
expect = "quiet"
command = "echo hi"
"#,
    )
    .unwrap();
    let why = yhooky::cases::run(&file).unwrap_err();
    assert!(why.ends_with("no rules"), "{why}");
}

#[test]
fn a_rule_folder_that_cannot_be_read_is_a_problem_unless_missing() {
    use std::path::PathBuf;
    use yhooky::rules::Rules;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rules.toml");
    std::fs::write(&file, "").unwrap();
    let problems = |d: PathBuf| Rules::load(&[d]).problems.len();
    assert_eq!(problems(dir.path().to_path_buf()), 0, "a folder");
    assert_eq!(problems(dir.path().join("none")), 0, "a missing folder");
    assert_eq!(problems(PathBuf::new()), 1, "an empty path");
    assert_eq!(problems(file), 1, "a file");
}

#[test]
fn private_word_guard_stops_a_push_with_listed_words() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        )
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(repo.join("pushed.md"), "old Hidden-Word\n").unwrap();
    git(&["add", "pushed.md"]);
    git(&["commit", "-q", "-m", "pushed"]);
    // What a remote already has is not looked at.
    git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    std::fs::write(repo.join("new.md"), "clean\n").unwrap();
    git(&["add", "new.md"]);
    git(&["commit", "-q", "-m", "clean"]);

    let list = dir.path().join("words.toml");
    // SAFETY: only this test reads the variables.
    unsafe {
        std::env::set_var("PRIVATE_WORDS_FILE", &list);
        std::env::set_var("PRIVATE_WORDS_STATE_DIR", dir.path().join("state"));
    }
    let push = |command: &str| {
        let raw = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "cwd": repo.to_string_lossy(),
            "tool_input": { "command": command },
        });
        yhooky::builtin::private_words::run(&yhooky::input::Input::parse(&raw.to_string()).unwrap())
    };
    assert!(push("git push").is_empty(), "no list file");
    std::fs::write(&list, "words = [\"hidden-word\"]\n").unwrap();
    assert!(push("git push").is_empty(), "only in a pushed commit");

    std::fs::write(repo.join("new.md"), "clean\nnew hidden-word\n").unwrap();
    git(&["commit", "-q", "-am", "add"]);
    let found = push("git push");
    assert_eq!(found.len(), 1, "in an unpushed commit");
    assert!(matches!(found[0].kind, yhooky::output::Kind::Deny));
    assert!(
        found[0].message.contains("new.md:2"),
        "{}",
        found[0].message
    );
    assert!(found[0].message.contains("「hidden-word」"));
    assert!(push("git push").is_empty(), "the same push sent again");
    assert!(push("git status").is_empty(), "not a push");
    std::fs::write(
        repo.join("new.md"),
        "clean\nnew hidden-word\nmore hidden-word\n",
    )
    .unwrap();
    git(&["commit", "-q", "-am", "more"]);
    assert_eq!(push("git push").len(), 1, "a new place found");

    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/someone/personal.git",
    ]);
    std::fs::write(
        &list,
        "words = [\"hidden-word\"]\nskip_repos = [\"Someone/Personal\"]\n",
    )
    .unwrap();
    assert!(push("git push").is_empty(), "a repository listed to skip");

    // The repository's own name is listed but not looked for inside it.
    std::fs::write(repo.join("new.md"), "clean\npersonal notes\n").unwrap();
    git(&["commit", "-q", "-am", "name"]);
    std::fs::write(&list, "words = [\"personal\"]\n").unwrap();
    assert!(push("git push").is_empty(), "its own name");
    std::fs::write(&list, "words = [\"notes\"]\n").unwrap();
    assert_eq!(push("git push").len(), 1, "another listed word");
}
