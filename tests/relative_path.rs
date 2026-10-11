//! Case files given by a path relative to the working folder. In a file of
//! its own: it changes the working folder, which the other tests' threads
//! would share.

use std::path::Path;

#[test]
fn case_file_relative_to_the_rules_or_tests_folder() {
    let rules = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("rules");
    // From the rules folder, `tests/x.toml`: its parent's parent is an
    // empty path.
    std::env::set_current_dir(&rules).unwrap();
    assert_eq!(
        yhooky::cases::run(Path::new("tests/label-guard.toml")),
        Ok(0)
    );
    // From the tests folder, `x.toml`.
    std::env::set_current_dir(rules.join("tests")).unwrap();
    assert_eq!(yhooky::cases::run(Path::new("label-guard.toml")), Ok(0));
}
