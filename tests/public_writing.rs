//! public-repo-writing-reminder and the review push-check-reminder asks for
//! before a push to a public repository. One test, as it sets environment
//! variables the checks read. Whether a repository is public comes from
//! the cache src/public_repo.rs keeps, so no network is used.

use std::path::Path;
use std::process::Command;

use yhooky::input::Input;
use yhooky::output::{Finding, Kind};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn commit_file(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();
    git(dir, &["add", path]);
    git(dir, &["commit", "-q", "-m", "x"]);
}

/// Marks HEAD as pushed: origin's default branch points at it.
fn mark_pushed(dir: &Path) {
    git(dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        dir,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
}

fn cache(dir: &Path, repo: &str, answer: &str) {
    let hash = sha1_smol::Sha1::from(repo.as_bytes()).digest().to_string();
    std::fs::write(dir.join(&hash[..16]), answer).unwrap();
}

fn slash(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn writing(raw: serde_json::Value) -> Vec<Finding> {
    let input = Input::parse(&raw.to_string()).unwrap();
    yhooky::builtin::public_writing::run(&input)
}

fn edit(path: &Path, session: &str) -> Vec<Finding> {
    writing(serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "session_id": session,
        "tool_input": { "file_path": slash(path) },
    }))
}

fn shell(tool: &str, command: &str, cwd: &Path, session: &str) -> Vec<Finding> {
    writing(serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "session_id": session,
        "cwd": slash(cwd),
        "tool_input": { "command": command },
    }))
}

fn push(dir: &Path, session: &str) -> Vec<Finding> {
    let raw = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "session_id": session,
        "cwd": slash(dir),
        "tool_input": { "command": "git push" },
    });
    let input = Input::parse(&raw.to_string()).unwrap();
    yhooky::builtin::push_check::run(&input)
}

const REVIEW: &str = "レビュー用の subagent";

#[test]
fn public_repositories_get_notes_and_a_review_before_a_push() {
    let state = tempfile::tempdir().unwrap();
    let cache_dir = state.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    // SAFETY: this test is the only one in its binary.
    unsafe {
        std::env::set_var("YHOOKY_PUBLIC_CACHE_DIR", &cache_dir);
        std::env::set_var("PUBLIC_WRITING_STATE_DIR", state.path().join("writing"));
        std::env::set_var("PUSH_CHECK_STATE_DIR", state.path().join("push"));
    }
    cache(&cache_dir, "o/open", "public");
    cache(&cache_dir, "o/secret", "private");
    let root = tempfile::tempdir().unwrap();
    let open = root.path().join("open");
    let secret = root.path().join("secret");
    for (dir, url) in [
        (&open, "https://github.com/o/open.git"),
        (&secret, "git@github.com:o/secret.git"),
    ] {
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(dir)
                .status()
                .unwrap()
                .success()
        );
        git(dir, &["remote", "add", "origin", url]);
        commit_file(dir, "src/a.rs", "let a = 1;\n");
        mark_pushed(dir);
    }
    let noted = |found: &[Finding]| {
        found.len() == 1 && found[0].kind == Kind::Context && found[0].message.contains("public")
    };

    // Writing a file.
    assert!(noted(&edit(&open.join("README.md"), "s1")), "first write");
    assert!(
        edit(&open.join("src/a.rs"), "s1").is_empty(),
        "second write"
    );
    assert!(noted(&edit(&open.join("src/a.rs"), "s2")), "other session");
    assert!(edit(&secret.join("README.md"), "s1").is_empty(), "private");
    assert!(
        edit(&root.path().join("loose.md"), "s1").is_empty(),
        "outside a repository"
    );

    // Opening a PR or a release.
    let pr = shell(
        "Bash",
        "gh pr create --title x --body-file b.md",
        &open,
        "s1",
    );
    assert!(noted(&pr), "first PR after a write");
    assert!(pr[0].message.contains("非公開にできない"));
    assert!(
        shell("Bash", "gh pr create --title y", &open, "s1").is_empty(),
        "second PR"
    );
    assert!(
        shell("Bash", "git commit -m \"gh pr create\"", &open, "s3").is_empty(),
        "quoted in a commit message"
    );
    assert!(
        shell("Bash", "gh pr create -R o/secret --title x", &open, "s3").is_empty(),
        "private repository named by -R"
    );
    assert!(
        noted(&shell(
            "PowerShell",
            "gh release create v1 --repo o/open --notes-file n.md",
            &secret,
            "s3"
        )),
        "public repository named by --repo"
    );
    assert!(
        shell("Bash", "gh pr view 1", &open, "s4").is_empty(),
        "not a create"
    );

    // Pushing: a review only when the commits change text for readers.
    commit_file(&open, "README.md", "# open\n");
    let found = push(&open, "s1");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, Kind::Deny);
    assert!(found[0].message.contains(REVIEW), "README changed");
    assert!(push(&open, "s1").is_empty(), "same push again");

    mark_pushed(&open);
    commit_file(&open, "src/a.rs", "let a = 2;\n");
    let found = push(&open, "s1");
    assert_eq!(found.len(), 1, "still stopped once");
    assert!(!found[0].message.contains(REVIEW), "code only");

    mark_pushed(&open);
    commit_file(
        &open,
        "src/a.rs",
        "// Two, as the caller expects.\nlet a = 2;\n",
    );
    assert!(
        push(&open, "s1")[0].message.contains(REVIEW),
        "a comment added"
    );

    commit_file(&secret, "README.md", "# secret\n");
    let found = push(&secret, "s1");
    assert_eq!(found.len(), 1);
    assert!(!found[0].message.contains(REVIEW), "private repository");
}
