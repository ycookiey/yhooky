//! `yhooky notices` prints THIRD-PARTY-NOTICES.md and
//! DEPENDENCY-LICENSES.txt, which are built into the executable, and
//! DEPENDENCY-LICENSES.txt lists the crates the executable is built from.

use std::collections::BTreeSet;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn prints_the_notices_built_in() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_yhooky"))
        .arg("notices")
        .output()
        .unwrap();
    assert!(out.status.success());
    let printed = String::from_utf8(out.stdout).unwrap();
    for file in ["THIRD-PARTY-NOTICES.md", "DEPENDENCY-LICENSES.txt"] {
        let text = std::fs::read_to_string(root().join(file)).unwrap();
        assert!(printed.contains(text.trim()), "{file} is not printed");
    }
}

/// When the reader closes the pipe after the first few bytes, `yhooky
/// notices` still exits successfully, without a panic.
#[test]
fn stops_quietly_when_the_reader_stops() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_yhooky"))
        .arg("notices")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = [0u8; 16];
    child.stdout.take().unwrap().read_exact(&mut first).unwrap();
    // The read end is dropped here, with most of the output unread.
    let out = child.wait_with_output().unwrap();
    let errors = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{errors}");
    assert!(!errors.contains("panicked"), "{errors}");
}

/// The crates the executable is built from, as `name vX.Y.Z`: the
/// package's normal dependencies (not dev or build ones) for any target,
/// as `cargo tree` resolves them, without the package itself.
fn built_from() -> BTreeSet<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let out = std::process::Command::new(cargo)
        .args([
            "tree",
            "--edges",
            "normal",
            "--target",
            "all",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--offline",
        ])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(out.status.success(), "cargo tree failed");
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        // `name v1.0.0 (proc-macro)`, `name v1.0.0 (*)`, `name v1.0.0 (C:\…)`
        .filter_map(|line| line.split(" (").next())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| *line != concat!(env!("CARGO_PKG_NAME"), " v", env!("CARGO_PKG_VERSION")))
        .map(str::to_string)
        .collect()
}

/// The crates DEPENDENCY-LICENSES.txt lists match those [`built_from`]
/// gives.
#[test]
fn dependency_licenses_list_the_crates_built_in() {
    let text = std::fs::read_to_string(root().join("DEPENDENCY-LICENSES.txt")).unwrap();
    let listed: BTreeSet<String> = text
        .lines()
        .filter_map(|line| line.strip_prefix("  crate: "))
        .filter_map(|line| {
            let (name, version) = line.trim_end().split_once(' ')?;
            Some(format!("{name} v{version}"))
        })
        .collect();
    let built = built_from();
    let missing: Vec<_> = built.difference(&listed).collect();
    let extra: Vec<_> = listed.difference(&built).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "DEPENDENCY-LICENSES.txt is out of date (regenerate it with \
         `cargo about generate about.hbs -o DEPENDENCY-LICENSES.txt`); \
         missing {missing:?}, not built in {extra:?}"
    );
}
