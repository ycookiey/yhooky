//! The pointer a message gives to the docs folder named by YHOOKY_DOCS_DIR.
//! One test, as it sets the environment variable the checks read.

use yhooky::input::Input;

fn backslash_note() -> String {
    let raw = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "s",
        "tool_name": "Bash",
        "tool_input": { "command": "echo a\\\\b" },
    });
    let input = Input::parse(&raw.to_string()).unwrap();
    let found = yhooky::builtin::backslash::run(&input);
    assert_eq!(found.len(), 1);
    found[0].message.clone()
}

#[test]
fn points_to_the_docs_folder_only_when_set() {
    // SAFETY: the only test in this file, so no other thread reads the
    // environment meanwhile.
    unsafe { std::env::remove_var("YHOOKY_DOCS_DIR") };
    assert_eq!(yhooky::docs::path("bash.md"), None);
    assert_eq!(yhooky::docs::pointer("(", "bash.md", ")"), "");
    let note = backslash_note();
    assert!(note.contains("1 本に縮んだ。"), "{note}");
    assert!(!note.contains("bash.md"), "{note}");

    unsafe { std::env::set_var("YHOOKY_DOCS_DIR", "  ") };
    assert_eq!(yhooky::docs::path("bash.md"), None, "blank counts as unset");

    unsafe { std::env::set_var("YHOOKY_DOCS_DIR", "/home/u/docs/") };
    assert_eq!(
        yhooky::docs::path("bash.md").as_deref(),
        Some("/home/u/docs/bash.md")
    );
    assert_eq!(
        yhooky::docs::pointer("詳しくは ", "public-repo.md", "。"),
        "詳しくは /home/u/docs/public-repo.md。"
    );
    let note = backslash_note();
    assert!(
        note.contains("1 本に縮んだ(/home/u/docs/bash.md)。"),
        "{note}"
    );
}
