//! yhooky hook                 run as a hook: input on stdin, answer on stdout
//! yhooky explain              what each rule finds for the input on stdin, as JSON lines
//! yhooky check                read every rule file and report the ones that cannot be used
//! yhooky test <cases.toml>... check rules against example inputs (src/cases.rs)
//! yhooky language-check <f>   the turns of a past transcript language-guard would block
//! yhooky show-file <path>...  open each file's folder in Explorer with the file selected,
//!                             minimized (src/explorer.rs; started by sendfile-to-ysidepaney,
//!                             so the hook need not wait)
//!
//! As a hook it never stands in the way: when anything goes wrong (bad
//! input, a rule file it cannot read, a panic) it prints nothing and exits
//! 0, and logs why to ~/.claude/logs/yhooky.log.

use std::io::Read;
use std::panic;
use std::path::Path;
use std::process::ExitCode;

use yhooky::input::Input;
use yhooky::output::answer;
use yhooky::rules::Rules;
use yhooky::{builtin, findings, log, rule_dirs};

fn read_input() -> Option<Input> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok()?;
    Input::parse(&raw)
}

fn hook() {
    let Some(input) = read_input() else {
        log("input is not JSON");
        return;
    };
    let rules = Rules::load(&rule_dirs());
    for problem in &rules.problems {
        log(problem);
    }
    let found = findings(&input, &rules);
    // Printed only once every rule has run, so a failure cannot leave half
    // an answer.
    if let Some(out) = answer(&input.event(), &found) {
        println!("{out}");
    }
}

fn explain() {
    let Some(input) = read_input() else { return };
    let rules = Rules::load(&rule_dirs());
    for finding in findings(&input, &rules) {
        println!("{}", serde_json::to_string(&finding).unwrap_or_default());
    }
}

fn check() -> ExitCode {
    let dirs = rule_dirs();
    let rules = Rules::load(&dirs);
    for dir in &dirs {
        println!("rules: {}", dir.display());
    }
    println!(
        "{} rules, built-in checks: {}",
        rules.rules.len(),
        rules.builtins.join(", ")
    );
    let mut bad = rules.problems.clone();
    bad.extend(
        rules
            .builtins
            .iter()
            .filter(|n| builtin::find(n).is_none())
            .map(|n| format!("unknown builtin {n}")),
    );
    for problem in &bad {
        println!("problem: {problem}");
    }
    if bad.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("hook") | None => {
            panic::set_hook(Box::new(|info| log(&format!("panic: {info}"))));
            let _ = panic::catch_unwind(hook);
            ExitCode::SUCCESS
        }
        Some("explain") => {
            explain();
            ExitCode::SUCCESS
        }
        Some("check") => check(),
        Some("test") => {
            let mut failed = 0;
            for file in &args[1..] {
                match yhooky::cases::run(Path::new(file)) {
                    Ok(n) => failed += n,
                    Err(why) => {
                        println!("problem: {why}");
                        failed += 1;
                    }
                }
            }
            if failed == 0 {
                println!("all cases passed");
                ExitCode::SUCCESS
            } else {
                println!("{failed} case(s) failed");
                ExitCode::FAILURE
            }
        }
        Some("language-check") => {
            let Some(file) = args.get(1) else {
                eprintln!("usage: yhooky language-check <transcript.jsonl>");
                return ExitCode::FAILURE;
            };
            let found = builtin::language::check(Path::new(file));
            if found.is_empty() {
                println!("英語主体の返答はありません");
            }
            for line in found {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Some("show-file") => {
            let shown = args[1..]
                .iter()
                .filter(|path| yhooky::explorer::show_selected(path))
                .count();
            if args.len() > 1 && shown == args.len() - 1 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some(other) => {
            eprintln!("unknown command {other}; see the top of src/main.rs");
            ExitCode::FAILURE
        }
    }
}
