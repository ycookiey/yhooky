//! Whether a file lies in a repository others can read.
//!
//! The `origin` URL comes from the repository's config, read as a file (a
//! git process takes about 100 ms on Windows). A GitHub repository is looked
//! up with `gh api` and the answer kept for a day under
//! `$YHOOKY_PUBLIC_CACHE_DIR`, else `<temp>/claude-public-repo`; when the
//! lookup fails, or the remote is not on GitHub, the repository counts as
//! public, and a failed lookup is kept for ten minutes so a missing network
//! does not cost every call. A repository without `origin` is not public.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

const KNOWN_FOR: Duration = Duration::from_secs(24 * 60 * 60);
const UNKNOWN_FOR: Duration = Duration::from_secs(10 * 60);
const GH_TIMEOUT: Duration = Duration::from_secs(5);

/// The config file of the repository holding `path`: `.git/config`, or for
/// a linked worktree (`.git` is a file) the config of the main repository.
pub fn git_config(path: &Path) -> Option<PathBuf> {
    let mut dir = if path.is_dir() { path } else { path.parent()? };
    loop {
        let git = dir.join(".git");
        if git.is_dir() {
            return Some(git.join("config"));
        }
        if git.is_file() {
            let text = fs::read_to_string(&git).ok()?;
            let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
            let gitdir = dir.join(gitdir);
            let common = match fs::read_to_string(gitdir.join("commondir")) {
                Ok(common) => gitdir.join(common.trim()),
                Err(_) => gitdir,
            };
            return Some(common.join("config"));
        }
        dir = dir.parent()?;
    }
}

/// The `url` of `[remote "origin"]` in a git config.
pub fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line == r#"[remote "origin"]"#;
        } else if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// `owner/repo` of a GitHub URL (https, ssh or scp-like).
pub fn github_repo(url: &str) -> Option<String> {
    let rest = [
        "https://github.com/",
        "http://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ]
    .iter()
    .find_map(|prefix| url.strip_prefix(prefix))?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(repo), None) if !owner.is_empty() && !repo.is_empty() => {
            Some(format!("{owner}/{repo}"))
        }
        _ => None,
    }
}

fn cache_dir() -> PathBuf {
    match std::env::var("YHOOKY_PUBLIC_CACHE_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(crate::text::windows_path(&dir)),
        _ => std::env::temp_dir().join("claude-public-repo"),
    }
}

/// What `gh api` says: Some(true) for public, Some(false) for private.
fn ask_github(repo: &str) -> Option<bool> {
    let mut child = Command::new("gh")
        .args(["api", &format!("repos/{repo}"), "--jq", ".private"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if start.elapsed() > GH_TIMEOUT => {
                let _ = child.kill();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    match out.trim() {
        "false" => Some(true),
        "true" => Some(false),
        _ => None,
    }
}

/// Whether `repo` (`owner/repo` on GitHub) is public, from the cache when
/// it is fresh enough.
fn github_public(repo: &str) -> bool {
    let hash = sha1_smol::Sha1::from(repo.as_bytes()).digest().to_string();
    let file = cache_dir().join(&hash[..16]);
    if let (Ok(text), Ok(meta)) = (fs::read_to_string(&file), fs::metadata(&file)) {
        let age = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .unwrap_or(Duration::MAX);
        let fresh_for = if text == "unknown" {
            UNKNOWN_FOR
        } else {
            KNOWN_FOR
        };
        if age < fresh_for {
            return text != "private";
        }
    }
    let answer = ask_github(repo);
    let text = match answer {
        Some(true) => "public",
        Some(false) => "private",
        None => "unknown",
    };
    if fs::create_dir_all(cache_dir()).is_ok() {
        let _ = fs::write(&file, text);
    }
    answer != Some(false)
}

/// Whether the file at `path` lies in a repository others can read.
pub fn is_public(path: &Path) -> bool {
    let Some(config) = git_config(path).and_then(|c| fs::read_to_string(c).ok()) else {
        return false;
    };
    let Some(url) = origin_url(&config) else {
        return false;
    };
    match github_repo(&url) {
        Some(repo) => github_public(&repo),
        // Not on GitHub: cannot be looked up, so counted as public.
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_origin() {
        let config = "[core]\n\turl = no\n[remote \"upstream\"]\n\turl = a\n[remote \"origin\"]\n\tfetch = x\n\turl = git@github.com:o/r.git\n";
        assert_eq!(
            origin_url(config).as_deref(),
            Some("git@github.com:o/r.git")
        );
        assert_eq!(origin_url("[remote \"upstream\"]\n url = a\n"), None);
    }

    #[test]
    fn names_github_repos() {
        for url in [
            "https://github.com/o/r",
            "https://github.com/o/r.git",
            "git@github.com:o/r.git",
            "ssh://git@github.com/o/r/",
        ] {
            assert_eq!(github_repo(url).as_deref(), Some("o/r"), "{url}");
        }
        assert_eq!(github_repo("https://gitlab.com/o/r"), None);
        assert_eq!(github_repo("https://github.com/o"), None);
    }

    #[test]
    fn finds_the_config_of_a_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        fs::create_dir_all(main.join(".git/worktrees/w")).unwrap();
        fs::write(main.join(".git/config"), "").unwrap();
        fs::write(main.join(".git/worktrees/w/commondir"), "../..\n").unwrap();
        let w = dir.path().join("w");
        fs::create_dir_all(w.join("docs")).unwrap();
        fs::write(
            w.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/w").display()),
        )
        .unwrap();
        let found = git_config(&w.join("docs/README.md")).unwrap();
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(main.join(".git/config")).unwrap()
        );
        assert_eq!(
            git_config(&main.join("a.md")).unwrap(),
            main.join(".git/config")
        );
    }

    #[test]
    fn uses_the_cached_answer() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests in this module that read the variable run after it is set.
        unsafe { std::env::set_var("YHOOKY_PUBLIC_CACHE_DIR", dir.path()) };
        let hash = sha1_smol::Sha1::from("o/secret".as_bytes())
            .digest()
            .to_string();
        fs::write(dir.path().join(&hash[..16]), "private").unwrap();
        assert!(!github_public("o/secret"));
        let hash = sha1_smol::Sha1::from("o/open".as_bytes())
            .digest()
            .to_string();
        fs::write(dir.path().join(&hash[..16]), "public").unwrap();
        assert!(github_public("o/open"));
    }
}
