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
