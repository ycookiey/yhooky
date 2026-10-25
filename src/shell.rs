//! Reading a shell command as far as the checks need: which commands it
//! runs, in order, and in which nested list (subshell, substitution, a
//! shell started inside) each runs. Text is not a command when it is
//! quoted (a commit message, a PR body), a comment, or the body of a
//! heredoc that only feeds a file or a program's input; it is when it is
//! handed to a shell (`bash -c "…"`, `bash <<EOF`, `cat <<EOF | bash`) or
//! substituted (`$(…)`, backquotes, `<(…)`, `>(…)`). Every check that asks
//! what a command runs reads it here, so that they agree on what counts.
//!
//! The command is read from the start without going back: a quote or
//! parenthesis left open runs to the end, as bash would then run nothing.
//! Only what is handed to a shell inside is read again, as that shell's
//! commands, within a budget in proportion to the length; past it the
//! text is kept whole as one command, so that patterns still see it.
//! Positions are byte offsets that only stop on ASCII bytes or at the
//! end, so slicing never cuts a character.
//!
//! Whether a command runs what a pattern matches ([`runs_in`]) is also
//! decided by a plain match outside quotes ([`as_text_finds`]), with only
//! the text this reading shows to be data left out of it, so that a way of
//! handing text to a shell not followed here still counts.

use std::sync::LazyLock;

use regex::Regex;

/// Which shell's rules a command is read with.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dialect {
    Bash,
    /// Backquote escapes; `@'…'@` and `@"…"@` are strings; no heredocs.
    PowerShell,
}

/// Leading words that only set up how the rest runs.
static PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:[({]\s*|[A-Za-z_][A-Za-z0-9_]*=\S*\s+|then\s+|do\s+|else\s+|(?:[^\s/]*/)*(?:timeout(\s+-\S+)*\s+\S+|nohup|exec|time|command|nice(\s+-n\s*\S+|\s+-\S+)*|sudo(\s+-[ugpCDhrtU]\s*\S+|\s+-\S+)*|env(\s+-[uC]\s*\S+|\s+-[i0v]+|\s+--(unset|chdir)=\S+|\s+--(ignore-environment|null|debug)?)*|wsl(\s+-e)?|busybox|flock(\s+-\S+)*\s+\S+)\s+)",
    )
    .unwrap()
});
/// The name of a shell, which runs what it is given, also as the
/// variables that hold one (`"$SHELL"`, `$BASH`).
const SHELL: &str =
    r"(bash|sh|zsh|dash|ksh|fish|nu|pwsh|powershell|cmd|\$\{?SHELL\}?|\$\{?BASH\}?)(?:\.exe)?";
/// A shell at the start of a command, as written: by name or by path,
/// also in quotes (`/bin/bash`, `"C:/Program Files/Git/bin/bash.exe"`,
/// `"$SHELL"`). Where the name ends is checked in [`shell_head`].
static SHELL_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)^(?:"[^"]*[/\\]|'[^']*[/\\]|(?:[^\s"'\\]|\\.)*[/\\]|["'])?{SHELL}["']?"#
    ))
    .unwrap()
});
/// The options of a shell before what it is handed: `-x`, `/d`, and an
/// option with its value (`-o pipefail`, `-ExecutionPolicy Bypass`).
const SHELL_OPTIONS: &str = r#"(\s+[-/+]\S*(\s+[^-/+\s"'][^\s"']*)?)*"#;
/// A shell (or another command that runs its `-c` text as a shell
/// command: `su`, `flock`, `script`, `npx`) and its options, right before
/// the quoted text it is handed as its command (`bash -c `, `xargs sh -c
/// `, `cmd /d /c `, `pwsh -NoProfile -Command `): it may come after
/// another command that starts it.
static SHELL_RUNS_ARG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)(^|[\s/\\"'(;&|`]){SHELL}["']?{SHELL_OPTIONS}\s+(-[a-z]*c|/c|//c|-command)\s*$|(^|[\s/\\"'(;&|`])(su|runuser|sg|flock|script|npx)(\s+\S+)*?\s+-[a-z]*c\s*$"#
    ))
    .unwrap()
});

/// The shell that starts `command`: the end of its name (and closing
/// quote), and the name in lower case.
fn shell_head(command: &str) -> Option<(usize, String)> {
    let caps = SHELL_HEAD.captures(command)?;
    let end = caps.get(0)?.end();
    // The name is a whole word (`bash`, not `bashrc` or `shellcheck`).
    let next = command[end..].chars().next();
    if next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    Some((end, caps.get(1)?.as_str().to_lowercase()))
}
/// A command that runs what it is given as a command (`eval "$l"`).
static EVALS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^eval(\s|$)").unwrap());
/// The name in a function's definition (`f`, `function f`).
static FUNCTION_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(function\s+)?[A-Za-z_][A-Za-z0-9_.:-]*$").unwrap());
/// A command that reads a script from a file it is given (`source`, `.`).
static SOURCES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(source|\.)(\s|$)").unwrap());
/// Only options of a shell that reads its commands from its input (`-e`,
/// `-o pipefail`, `--rcfile f`), then optionally `-s`, `-` or `--` and the
/// arguments for the commands (`bash -s -- a b`, `bash /dev/stdin a`).
static READS_INPUT_OPTIONS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(([-+][a-z]*o\s+\S+|--(rcfile|init-file)\s+\S+|-\S*|\+\S+)(\s+|$))*((-[a-z]*s|-|--)(\s.*)?)?$",
    )
    .unwrap()
});
/// The same for PowerShell and cmd, whose options may take a value
/// (`-ExecutionPolicy Bypass -File -`, `/d`).
static READS_INPUT_OPTIONS_WITH_VALUES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(([-/]\S*)(\s+[^-/\s]\S*)?(\s+|$))*$").unwrap());
/// A redirection, with its target (`2>&1`, `> log`, `<<'EOF'`).
static REDIRECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d*(<<<|<<-?|<>|<|>>|>\||>|&>>|&>)&?-?\s*[^\s<>]*").unwrap());
/// A command that prints a heredoc it is given (`cat <<'EOF'`).
static PRINTS_INPUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^cat(\s|$)").unwrap());

/// How deep nested substitutions and quotes are followed; deeper ones are
/// passed over by counting parentheses.
const NESTING: usize = 64;
/// How deep shells started inside (`bash -c`, a heredoc given to a shell)
/// are followed.
const SHELLS: usize = 6;
/// How much text, as a multiple of the command's length, shells started
/// inside may read again in all: text handed to a shell is read once
/// where it is written and once more as the shell's, and nesting must not
/// multiply that.
const REREAD: usize = 8;

/// The end of the arithmetic `((…))` whose first `(` is at `open`, if it
/// is one: as bash reads it, the second `(` must close right before the
/// first does (`$((cd x) && y)` is a substitution holding a subshell).
fn arithmetic_end(bytes: &[u8], open: usize) -> Option<usize> {
    if bytes.get(open + 1) != Some(&b'(') {
        return None;
    }
    let inner = skip_parens(bytes, open + 1);
    (bytes.get(inner) == Some(&b')')).then_some(inner + 1)
}

fn escape(dialect: Dialect) -> u8 {
    match dialect {
        Dialect::Bash => b'\\',
        Dialect::PowerShell => b'`',
    }
}

/// The byte after the character that starts at `i`.
fn char_end(bytes: &[u8], i: usize) -> usize {
    let width = match bytes.get(i) {
        None => return bytes.len(),
        Some(0x00..=0x7f) => 1,
        Some(0xc0..=0xdf) => 2,
        Some(0xe0..=0xef) => 3,
        Some(_) => 4,
    };
    (i + width).min(bytes.len())
}

/// The byte after the character an escape at `i` escapes.
fn after_escape(bytes: &[u8], i: usize) -> usize {
    char_end(bytes, i + 1)
}

/// Whether a `#` at `i` starts a comment: at the start of a word.
fn starts_comment(bytes: &[u8], i: usize) -> bool {
    bytes[i] == b'#'
        && (i == 0 || bytes[i - 1].is_ascii_whitespace() || b";&|()".contains(&bytes[i - 1]))
}

/// Whether the comment whose `#` is at `i` is a whole line: only blanks
/// before it on its line, and the line before not continued with an
/// escape (`\`, or a backquote for PowerShell).
fn whole_line_comment(bytes: &[u8], i: usize, esc: u8) -> bool {
    let start = bytes[..i]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |p| p + 1);
    if !bytes[start..i].iter().all(|b| *b == b' ' || *b == b'\t') {
        return false;
    }
    let before = &bytes[..start.saturating_sub(1)];
    let before = before.strip_suffix(b"\r").unwrap_or(before);
    before.iter().rev().take_while(|b| **b == esc).count() % 2 == 0
}

/// The offset of the `\n` that ends the line at `i`, or the end.
fn line_end(bytes: &[u8], i: usize) -> usize {
    bytes[i..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(bytes.len(), |e| i + e)
}

/// The byte after the `'` that closes the one at `open`, or the end.
fn skip_single(bytes: &[u8], open: usize) -> usize {
    bytes[open + 1..]
        .iter()
        .position(|b| *b == b'\'')
        .map_or(bytes.len(), |e| open + 2 + e)
}

/// The byte after the `'` that closes a `$'…'` whose `'` is at `open`,
/// or the end: in it a backslash escapes (`$'don\'t'`).
fn skip_ansi_c(bytes: &[u8], open: usize) -> usize {
    let mut j = open + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j = after_escape(bytes, j),
            b'\'' => return j + 1,
            _ => j += 1,
        }
    }
    bytes.len()
}

/// The byte after the backquote that closes the one at `open`, or the end.
fn skip_backquote(bytes: &[u8], open: usize) -> usize {
    let mut j = open + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j = after_escape(bytes, j),
            b'`' => return j + 1,
            _ => j += 1,
        }
    }
    bytes.len()
}

/// The byte after a PowerShell here-string (`@'` or `@"` at `open`, ended
/// by a line that starts with the quote and `@`), or the end.
fn skip_here_string(bytes: &[u8], open: usize) -> usize {
    let close = [b'\n', bytes[open + 1], b'@'];
    bytes[open + 2..]
        .windows(3)
        .position(|w| w == close)
        .map_or(bytes.len(), |e| open + 2 + e + 3)
}

/// The byte after the `)` that closes the `(` at `open`, counting
/// parentheses only (for arithmetic, and past the nesting followed).
fn skip_parens(bytes: &[u8], open: usize) -> usize {
    let mut level = 0usize;
    for (j, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => level += 1,
            b')' => {
                level = level.saturating_sub(1);
                if level == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
    }
    bytes.len()
}

/// The byte after the `]` that closes the `[` at `open`, or the end.
fn skip_brackets(bytes: &[u8], open: usize) -> usize {
    let mut level = 0usize;
    for (j, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'[' => level += 1,
            b']' => {
                level = level.saturating_sub(1);
                if level == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
    }
    bytes.len()
}

/// The byte after the `"` that closes the one at `open`, or the end.
/// Substitutions inside are passed over whole, and so are the double
/// quotes nested in a `${…}` (`"${x:-"it's"}"`).
fn skip_double(bytes: &[u8], open: usize, dialect: Dialect, depth: usize) -> usize {
    let esc = escape(dialect);
    let mut j = open + 1;
    let mut braces = 0usize;
    while j < bytes.len() {
        match bytes[j] {
            b if b == esc => j = after_escape(bytes, j),
            b'$' if dialect == Dialect::Bash && bytes.get(j + 1) == Some(&b'{') => {
                braces += 1;
                j += 2;
            }
            b'}' if braces > 0 => {
                braces -= 1;
                j += 1;
            }
            b'"' if braces > 0 && depth < NESTING => j = skip_double(bytes, j, dialect, depth + 1),
            b'"' => return j + 1,
            b'$' if bytes.get(j + 1) == Some(&b'(') => {
                j = skip_substitution(bytes, j + 1, dialect, depth + 1);
            }
            b'`' if dialect == Dialect::Bash => j = skip_backquote(bytes, j),
            _ => j += 1,
        }
    }
    bytes.len()
}

/// The byte after the `)` that closes the `(` at `open`, or the end,
/// passing over quotes and comments inside.
fn skip_substitution(bytes: &[u8], open: usize, dialect: Dialect, depth: usize) -> usize {
    if depth > NESTING {
        return skip_parens(bytes, open);
    }
    let esc = escape(dialect);
    let mut level = 0usize;
    let mut j = open;
    while j < bytes.len() {
        match bytes[j] {
            b if b == esc => j = after_escape(bytes, j),
            b'\'' => j = skip_single(bytes, j),
            b'"' => j = skip_double(bytes, j, dialect, depth),
            b'`' if dialect == Dialect::Bash => j = skip_backquote(bytes, j),
            _ if starts_comment(bytes, j) => j = line_end(bytes, j),
            b'(' => {
                level += 1;
                j += 1;
            }
            b')' => {
                level = level.saturating_sub(1);
                j += 1;
                if level == 0 {
                    return j;
                }
            }
            _ => j += 1,
        }
    }
    bytes.len()
}

/// The byte ranges of the quoted strings in `text`, with their quotes.
fn quote_spans(text: &str, dialect: Dialect) -> Vec<(usize, usize)> {
    spans(text, dialect, false)
        .into_iter()
        .filter(|(start, _)| matches!(text.as_bytes()[*start], b'\'' | b'"'))
        .collect()
}

/// The byte ranges in `text` that are not shell words: quoted strings with
/// their quotes, comments, and with `substitutions` also `$(…)` and
/// backquotes.
fn spans(text: &str, dialect: Dialect, substitutions: bool) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let esc = escape(dialect);
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let end = match bytes[i] {
            b if b == esc => {
                i = after_escape(bytes, i);
                continue;
            }
            b'$' if dialect == Dialect::Bash && bytes.get(i + 1) == Some(&b'\'') => {
                skip_ansi_c(bytes, i + 1)
            }
            b'\'' => skip_single(bytes, i),
            b'"' => skip_double(bytes, i, dialect, 0),
            b'@' if dialect == Dialect::PowerShell
                && matches!(bytes.get(i + 1), Some(b'\'' | b'"')) =>
            {
                skip_here_string(bytes, i)
            }
            b'<' if dialect == Dialect::PowerShell && bytes.get(i + 1) == Some(&b'#') => text
                [i + 2..]
                .find("#>")
                .map_or(bytes.len(), |e| i + 2 + e + 2),
            b'`' if substitutions && dialect == Dialect::Bash => skip_backquote(bytes, i),
            b'$' if substitutions && bytes.get(i + 1) == Some(&b'(') => {
                skip_substitution(bytes, i + 1, dialect, 0)
            }
            _ if starts_comment(bytes, i) => line_end(bytes, i),
            _ => {
                i += 1;
                continue;
            }
        };
        found.push((i, end));
        i = end;
    }
    found
}

/// `text` with `spans` turned into `x`s byte for byte (line ends kept
/// unless `newlines`), so that offsets still point into `text`.
fn blank(text: &str, spans: &[(usize, usize)], newlines: bool) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for &(start, end) in spans {
        for b in &mut bytes[start..end] {
            if newlines || *b != b'\n' {
                *b = b'x';
            }
        }
    }
    // Spans start and end on ASCII bytes, so only whole characters went.
    String::from_utf8(bytes).unwrap_or_default()
}

/// `text` (a Bash command) with each quoted string and comment turned into
/// `x`s, byte for byte (line ends kept), so that a pattern does not read
/// them and offsets still point into `text`.
pub fn unquoted(text: &str) -> String {
    unquoted_in(text, Dialect::Bash)
}

/// `text` (a Bash command) with each quoted string turned into `x`s, byte
/// for byte; unlike [`unquoted`], a `#` is kept as text.
pub fn without_quotes(text: &str) -> String {
    blank(text, &quote_spans(text, Dialect::Bash), false)
}

/// [`unquoted`] for a command of `dialect`.
pub fn unquoted_in(text: &str, dialect: Dialect) -> String {
    blank(text, &spans(text, dialect, false), false)
}

/// One part as a command: continued lines joined, and leading set-up
/// words removed (found with quotes and substitutions blanked, so
/// `out=$(git push)` stays whole); empty when nothing is left.
fn clean(part: &str, dialect: Dialect) -> String {
    let joined = match dialect {
        Dialect::Bash => part.replace("\\\r\n", " ").replace("\\\n", " "),
        Dialect::PowerShell => part.replace("`\r\n", " ").replace("`\n", " "),
    };
    let part = joined.trim().trim_end_matches('&').trim_end();
    let blanked = blank(part, &spans(part, dialect, true), true);
    static RUNNER: LazyLock<Regex> = LazyLock::new(|| {
        // Not `time`: `time cd x` is this shell's `cd`.
        Regex::new(
            r"^(?:[^\s/]*/)*(?:sudo|env|nice|nohup|exec|command|timeout|wsl|busybox|flock)(\s|$)",
        )
        .unwrap()
    });
    static CHANGES_FOLDER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(cd|pushd|Set-Location)(\s|$)").unwrap());
    let mut from = 0;
    let mut runner = false;
    while let Some(m) = PREFIX.find(&blanked[from..]) {
        let is_runner = RUNNER.is_match(m.as_str());
        let next = from + m.end();
        let next = next + blanked[next..].len() - blanked[next..].trim_start().len();
        // A command that starts no other (`sudo -i <<EOF`) is kept.
        if is_runner && blanked[next..].starts_with(['<', '>']) {
            break;
        }
        runner |= is_runner;
        from = next;
    }
    let rest = &part[from..];
    // `sudo cd x` starts a program, not this shell's `cd`: kept as written.
    if runner && CHANGES_FOLDER.is_match(rest) {
        return part.to_string();
    }
    rest.to_string()
}

/// The heredoc whose `<<` is at `i`, as `<<(-?)\s*(['"\\]?)(\w+)\2?` reads
/// it (`<<<` is a string, not a heredoc).
fn marker_at(text: &str, i: usize) -> Option<Marker> {
    let bytes = text.as_bytes();
    if bytes.get(i + 1) != Some(&b'<') || bytes.get(i + 2) == Some(&b'<') {
        return None;
    }
    if i > 0 && bytes[i - 1] == b'<' {
        return None;
    }
    let mut j = i + 2;
    let strip = bytes.get(j) == Some(&b'-');
    if strip {
        j += 1;
    }
    while bytes.get(j).is_some_and(|b| *b == b' ' || *b == b'\t') {
        j += 1;
    }
    // The word, as bash reads one: up to a blank or an operator; quotes
    // and backslashes in it (`'EOF'`, `"END-OF-BODY"`, `E\OF`) are taken
    // out and keep the body from being expanded.
    let mut end = j;
    let mut word = String::new();
    let mut quoted = false;
    while let Some(&b) = bytes.get(end) {
        match b {
            _ if b.is_ascii_whitespace() || b";&|<>()".contains(&b) => break,
            // `$'EOF'` and `$"EOF"` quote too; escapes in `$'…'` are not
            // followed.
            b'$' if matches!(bytes.get(end + 1), Some(b'\'' | b'"')) => {
                let close = text[end + 2..].find(bytes[end + 1] as char)?;
                if text[end + 2..end + 2 + close].contains('\\') {
                    return None;
                }
                end += 1;
            }
            b'\'' | b'"' => {
                let close = text[end + 1..].find(b as char)?;
                word.push_str(&text[end + 1..end + 1 + close]);
                end += close + 2;
                quoted = true;
            }
            b'\\' => {
                let next = after_escape(bytes, end);
                word.push_str(&text[end + 1..next]);
                end = next;
                quoted = true;
            }
            _ => {
                let next = char_end(bytes, end);
                word.push_str(&text[end..next]);
                end = next;
            }
        }
    }
    if word.is_empty() {
        return None;
    }
    Some(Marker {
        strip,
        word,
        end,
        expands: !quoted,
    })
}

/// A heredoc's `<<` marker.
struct Marker {
    /// Whether leading tabs are stripped (`<<-`).
    strip: bool,
    /// The word on the line that ends the body.
    word: String,
    /// The byte after the marker.
    end: usize,
    /// Whether the body is expanded (the word is not quoted), so that the
    /// `$(…)` in it run.
    expands: bool,
}

/// What a part hands a shell as quoted text after `-c`, `/c` or
/// `-Command` (`bash -c "…"`, `cmd //c "…"`, `xargs sh -c '…'`), with the
/// dialect to read it in. Another command's `-c` (`grep -c "…"`) means
/// something else, unless a shell it starts takes it (`git bisect run sh
/// -c "…"`, `cargo run -- bash -c "…"`).
fn given_to_shell(part: &str, dialect: Dialect) -> Vec<(String, Dialect)> {
    let bytes = part.as_bytes();
    let any = hands_to_shell(part);
    quote_spans(part, dialect)
        .into_iter()
        // A quote left open runs to the end, and is not handed over.
        .filter(|&(start, end)| end - start >= 2 && bytes[end - 1] == bytes[start])
        // Combined options (`-lc`, `-ec`) only for a shell: `rsync -avc
        // "…"` copies.
        .filter(|&(start, _)| {
            let before = before(part, start);
            let shell = SHELL_RUNS_ARG.is_match(before);
            RUNS_ARG.captures(before).is_some_and(|c| {
                shell
                    || any
                        && matches!(
                            c[2].to_lowercase().as_str(),
                            "-c" | "/c" | "//c" | "-command"
                        )
            })
        })
        .map(|(start, end)| {
            let named = SHELL_RUNS_ARG.captures(before(part, start));
            let name = named
                .and_then(|c| c.get(2))
                .map_or(String::new(), |m| m.as_str().to_lowercase());
            let inner_dialect = match name.as_str() {
                "pwsh" | "powershell" => Dialect::PowerShell,
                _ => Dialect::Bash,
            };
            let inner = &part[start + 1..end - 1];
            // The escaped quotes inside come out as quotes; what double
            // quotes substitute is the outer shell's.
            let inner = match (bytes[start], dialect) {
                (b'"', Dialect::Bash) => without_substitutions(&inner.replace("\\\"", "\"")),
                (b'"', Dialect::PowerShell) => inner.replace("`\"", "\""),
                _ => inner.to_string(),
            };
            (inner, inner_dialect)
        })
        .collect()
}

/// Whether a command may hand the quoted text after its `-c` (`/c`,
/// `-Command`) to a shell: any command, as many start a shell (`bash`,
/// `${SHELL:-bash}`, `busybox ash`, `npm exec`, `xargs sh`, `su`), but not
/// those whose `-c` means something else (`grep -c`, `git -c`, `tar -c`,
/// `python -c`).
fn hands_to_shell(part: &str) -> bool {
    static OTHER_C: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^(grep|egrep|fgrep|rg|ag|wc|uniq|head|tail|cut|sort|git|gcc|g\+\+|cc|clang|tar|zip|unzip|curl|wget|sed|awk|gawk|jq|yq|python[0-9.]*|py|node|deno|bun|perl|ruby|php|java|dotnet|cargo|rustc)(\.exe)?$",
        )
        .unwrap()
    });
    let first = part.split_whitespace().next().unwrap_or("");
    let first = first.trim_matches(['"', '\'']);
    let name = first.rsplit(['/', '\\']).next().unwrap_or(first);
    !OTHER_C.is_match(name)
}

/// What hands the next quoted argument to a shell as its command.
static RUNS_ARG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(^|\s)(-[a-z]*c|/c|//c|-command)\s*$").unwrap());

/// The text right before `at` that a shell and its options may take
/// (256 bytes at most, so that looking before each quote of a long command
/// stays in proportion to its length).
fn before(text: &str, at: usize) -> &str {
    let mut start = at.saturating_sub(256);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..at]
}

/// Whether a command is a shell that reads its commands from its input
/// (`bash`, `bash -s -- a`, `/bin/bash -euo pipefail`, `bash /dev/stdin`,
/// `source /dev/stdin`, `powershell -NoProfile -File -`, with any
/// redirections): `bash x.sh` runs a script.
fn reads_input(command: &str) -> bool {
    // A shell started by `sudo -i`, `sudo -s`, `su` or `script` (without
    // `-c`) reads its input, and `parallel` runs each line it reads.
    static SWITCHES_USER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(sudo(\s+-\S+)*\s+-[a-zA-Z]*[is]|su(\s+-\S*)*(\s+[A-Za-z_][\w-]*)?|script(\s+-[a-bd-z]+)*(\s+\S+)?|parallel(\s+-\S+)*)\s*$",
        )
        .unwrap()
    });
    if SWITCHES_USER.is_match(REDIRECT.replace_all(&unquoted(command), " ").trim()) {
        return true;
    }
    // `xargs bash -c`, `xargs -I{} sh -c '{}'`: each line read is run.
    static XARGS_SHELL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r#"(?i)^xargs\b.*?\s(?:\S*[/\\])?{SHELL}["']?(\s+-\S+)*\s+-[a-z]*c(\s+["']?\{{\}}["']?)?\s*$"#
        ))
        .unwrap()
    });
    if XARGS_SHELL.is_match(REDIRECT.replace_all(command, " ").trim()) {
        return true;
    }
    let Some((rest, name)) = after_runner(command).or_else(|| started_shell(command)) else {
        return false;
    };
    let rest = unquoted(rest).replace("/dev/stdin", "-");
    let rest = REDIRECT.replace_all(&rest, " ");
    let rest = rest.trim();
    match name.as_str() {
        "source" | "." => rest == "-",
        "pwsh" | "powershell" | "cmd" => READS_INPUT_OPTIONS_WITH_VALUES.is_match(rest),
        _ => READS_INPUT_OPTIONS.is_match(rest),
    }
}

/// Whether a command runs a script given as `<(…)`, with options only
/// before it (`bash <(cat <<'EOF' …)`, `bash < <(…)`, `source <(…)`, `.
/// <(…)`).
fn runs_script_from_substitution(command: &str) -> bool {
    static BEFORE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\s*-\S+)*\s*(\d*<\s*)?$").unwrap());
    let Some((rest, _)) = after_runner(command) else {
        return false;
    };
    rest.find("<(")
        .is_some_and(|at| BEFORE.is_match(&rest[..at]))
}

/// A shell that another command starts (`unbuffer bash`, `stdbuf -oL sh`),
/// as [`after_runner`] gives it: the first word that is a shell, after a
/// command whose `-c` would hand it text ([`hands_to_shell`]).
fn started_shell(command: &str) -> Option<(&str, String)> {
    if !hands_to_shell(command) {
        return None;
    }
    let mut at = 0;
    for word in command.split_whitespace().skip(1) {
        let start = command[at..].find(word)? + at;
        at = start + word.len();
        if let Some((end, name)) = shell_head(&command[start..]) {
            return Some((&command[start + end..], name));
        }
    }
    None
}

fn command_is_cat(command: &str) -> bool {
    PRINTS_INPUT.is_match(command)
}

/// What follows the shell (or `source`, `.`) that starts `command`, and
/// its name.
fn after_runner(command: &str) -> Option<(&str, String)> {
    if let Some((end, name)) = shell_head(command) {
        return Some((&command[end..], name));
    }
    let word = command.split_whitespace().next()?;
    SOURCES
        .is_match(command)
        .then(|| (&command[word.len()..], word.to_string()))
}

/// Whether the text right before a substitution leaves it at the place of
/// a command, so that what it prints runs (`$(cat <<EOF …)` alone, `set
/// -e; $(…)`).
fn at_command_place(before: &str) -> bool {
    before
        .trim_end()
        .chars()
        .next_back()
        .is_none_or(|c| matches!(c, ';' | '&' | '|' | '\n' | '('))
}

/// A command a shell command runs, and the nested list it runs in.
#[derive(Clone, Debug)]
pub struct Run {
    pub text: String,
    /// The lists it is in, outermost first: each subshell, substitution
    /// and shell started inside opens one.
    scope: Vec<u32>,
}

impl Run {
    /// Whether what this command does to the shell (a `cd`) reaches
    /// `later`: it runs in the list `later` runs in, or one holding it.
    pub fn reaches(&self, later: &Run) -> bool {
        later.scope.starts_with(&self.scope)
    }

    /// Whether it runs in the command's own list, not a nested one.
    pub fn outermost(&self) -> bool {
        self.scope.is_empty()
    }

    /// Whether what this command does to the shell reaches what runs in
    /// the list `scope` ([`Run::reaches`], for a list a [`Run::scope`]
    /// gave).
    pub fn reaches_scope(&self, scope: &[u32]) -> bool {
        scope.starts_with(&self.scope)
    }

    /// The lists it is in, outermost first.
    pub fn scope(&self) -> &[u32] {
        &self.scope
    }
}

struct Entry {
    run: Run,
    /// The command this one pipes into, when it does.
    piped_into: Option<usize>,
}

/// A heredoc body taken out of the command.
struct Body {
    /// From the line after the one that opened it, to after its end line.
    start: usize,
    end: usize,
    text: String,
    /// The part whose `<<` opened it.
    owner: usize,
    expands: bool,
    /// Whether it may not be where bash reads it ([`Parser::read_bodies`]).
    uncertain: bool,
}

/// The state of one list being read: where its current part started, the
/// part's number, the last command it gave, and whether that pipes on.
struct List {
    start: usize,
    part: usize,
    last: Option<usize>,
    piping: bool,
    /// After `name()`: the next `{` opens a function's body.
    function_next: bool,
    /// Inside a function's body: its own scope (a `cd` there does not
    /// reach what follows the definition), and how many `{` are open.
    function: Option<(Vec<u32>, usize)>,
}

/// What holds a nested list: the part it is in (`bash <(…)` holds the
/// `<(…)`), where the substitution starts, and the double quote it is in.
#[derive(Clone, Copy)]
struct Holder {
    part: usize,
    at: usize,
    quote: Option<usize>,
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    dialect: Dialect,
    pos: usize,
    entries: Vec<Entry>,
    bodies: Vec<Body>,
    /// Heredocs opened on this line, with the part that opened each, read
    /// at its end.
    pending: Vec<(Marker, usize, usize)>,
    /// For each part, the entry it gave, if any.
    parts: Vec<Option<usize>>,
    /// For each nested list (by its scope's last number), what holds it.
    holders: std::collections::HashMap<u32, Holder>,
    /// Where the double quote being read opened, if one is.
    quote_open: Option<usize>,
    /// How many `$(…)` are open where the reading is.
    substitutions: usize,
    next_scope: u32,
    shells: usize,
    /// How much more text shells started inside may read.
    budget: std::rc::Rc<std::cell::Cell<usize>>,
    /// The comments read, as byte ranges.
    comments: Vec<(usize, usize)>,
    /// How many more commands may be looked at to find what runs a
    /// heredoc (in proportion to the length): past it, a body is not
    /// followed, and [`runs_in`] still reads it as plain text.
    checks: std::cell::Cell<usize>,
    /// Whether a group, a loop or a subshell sends what its commands print
    /// somewhere other than the output (`} > s.sh`).
    output_moved: bool,
    /// Whether a comment was read that is not a whole line, or a
    /// PowerShell block comment: where it starts may not be where the shell
    /// sees one.
    odd_comments: bool,
    /// What [`Parser::parse`] gave.
    runs: Vec<Run>,
}

impl<'a> Parser<'a> {
    fn new(
        text: &'a str,
        dialect: Dialect,
        next_scope: u32,
        shells: usize,
        budget: std::rc::Rc<std::cell::Cell<usize>>,
    ) -> Self {
        Parser {
            text,
            bytes: text.as_bytes(),
            dialect,
            pos: 0,
            entries: Vec::new(),
            bodies: Vec::new(),
            pending: Vec::new(),
            parts: Vec::new(),
            holders: std::collections::HashMap::new(),
            quote_open: None,
            substitutions: 0,
            next_scope,
            shells,
            budget,
            comments: Vec::new(),
            checks: std::cell::Cell::new(0),
            output_moved: false,
            odd_comments: false,
            runs: Vec::new(),
        }
    }

    /// Takes one from [`Parser::checks`], if any is left.
    fn check(&self) -> bool {
        let left = self.checks.get();
        self.checks.set(left.saturating_sub(1));
        left > 0
    }

    /// A parser for a whole command.
    fn root(text: &'a str, dialect: Dialect, shells: usize) -> Self {
        let budget = text.len().saturating_mul(REREAD).max(1 << 16);
        Parser::new(text, dialect, 0, shells, std::rc::Rc::new(budget.into()))
    }

    fn new_part(&mut self) -> usize {
        self.parts.push(None);
        self.parts.len() - 1
    }

    fn child(&mut self, scope: &[u32]) -> Vec<u32> {
        let mut child = scope.to_vec();
        child.push(self.next_scope);
        self.next_scope += 1;
        child
    }

    fn peek(&self, i: usize) -> Option<u8> {
        self.bytes.get(i).copied()
    }

    /// The text from `start` to `end` without the heredoc bodies in it.
    fn without_bodies(&self, start: usize, end: usize) -> String {
        let mut text = String::new();
        let mut at = start;
        for body in &self.bodies {
            if body.end <= at || body.start >= end {
                continue;
            }
            text.push_str(&self.text[at..body.start.max(at)]);
            at = body.end.min(end);
        }
        if at < end {
            text.push_str(&self.text[at..end]);
        }
        text
    }

    /// Reads `text` as a shell started in `scope` would, while the budget
    /// for reading again lasts.
    fn sub(&mut self, text: &str, dialect: Dialect, scope: Vec<u32>) -> Vec<Run> {
        let left = self.budget.get();
        if self.shells >= SHELLS || left < text.len() {
            return vec![whole(text, scope)];
        }
        self.budget.set(left - text.len());
        let budget = self.budget.clone();
        let mut parser = Parser::new(text, dialect, self.next_scope, self.shells + 1, budget);
        let runs = parser.parse(&scope);
        self.next_scope = parser.next_scope;
        runs
    }

    /// The commands the substitutions in a heredoc body run, where the body
    /// is expanded (`$(…)` and backquotes, as in double quotes).
    fn expansions(&mut self, text: &str, scope: Vec<u32>) -> Vec<Run> {
        let left = self.budget.get();
        if left < text.len() {
            return vec![whole(text, scope)];
        }
        self.budget.set(left - text.len());
        let budget = self.budget.clone();
        let mut parser = Parser::new(text, Dialect::Bash, self.next_scope, self.shells, budget);
        let part = parser.new_part();
        parser.expanded(&scope, 0, part);
        self.next_scope = parser.next_scope;
        parser.entries.into_iter().map(|e| e.run).collect()
    }

    /// Ends the current part of `list` at `end`; `pipe` when it pipes into
    /// the next.
    fn finish(&mut self, list: &mut List, end: usize, scope: &[u32], pipe: bool) {
        let raw = self.without_bodies(list.start, end);
        let part = clean(&raw, self.dialect);
        // A function's body (`f() { cd x; }`) runs only when it is called,
        // so it gets a scope of its own.
        let opens = raw.trim_start().starts_with('{');
        if opens && list.function_next {
            list.function_next = false;
            list.function = Some((self.child(scope), 0));
        }
        if opens && let Some((_, open)) = &mut list.function {
            *open += 1;
        }
        let scope = match &list.function {
            Some((function, _)) => function.clone(),
            None => scope.to_vec(),
        };
        let scope = scope.as_slice();
        // What only closes a group or a loop (`}`, `done`, `fi`, `esac`),
        // with or without redirections, runs nothing; what pipes on from it
        // is what the last command inside prints (`{ cat <<EOF; } 2>&1 |
        // bash`, `for …; done | bash`).
        let blanked = unquoted_in(&part, self.dialect);
        let bare = REDIRECT.replace_all(&blanked, " ");
        let closes = matches!(bare.trim(), "" | "}" | "done" | "fi" | "esac");
        // `{ cat <<EOF …; } > s.sh`: what the commands inside print goes
        // elsewhere ([`Parser::printed_is_data`]).
        // And `{ cat <<EOF …; echo; } | bash`: only the last command inside
        // is linked to the pipe.
        if closes && (pipe || !matches!(output_of(&part), Output::On | Output::Null)) {
            self.output_moved = true;
        }
        if bare.trim() == "}"
            && let Some((_, open)) = &mut list.function
        {
            *open = open.saturating_sub(1);
            if *open == 0 {
                list.function = None;
            }
        }
        if !part.is_empty() && !closes {
            let index = self.entries.len();
            self.entries.push(Entry {
                run: Run {
                    text: part.clone(),
                    scope: scope.to_vec(),
                },
                piped_into: None,
            });
            self.parts[list.part] = Some(index);
            if list.piping
                && let Some(last) = list.last
            {
                self.entries[last].piped_into = Some(index);
            }
            list.last = Some(index);
            for (inner, dialect) in given_to_shell(&part, self.dialect) {
                let scope = self.child(scope);
                let runs = self.sub(&inner, dialect, scope);
                self.entries.extend(runs.into_iter().map(|run| Entry {
                    run,
                    piped_into: None,
                }));
            }
        }
        // A pipe goes on over an empty part: `a |` then a line end, or `|
        // (b)`.
        if !raw.trim().is_empty() || pipe {
            list.piping = pipe;
        }
        list.part = self.new_part();
    }

    /// Reads the bodies of the heredocs opened on the line that just ended,
    /// at nesting `depth`.
    fn read_bodies(&mut self, depth: usize) {
        for (marker, owner, opened) in std::mem::take(&mut self.pending) {
            let Marker {
                strip,
                word,
                expands,
                ..
            } = marker;
            let start = self.pos;
            let mut lines = Vec::new();
            while self.pos < self.bytes.len() {
                let line_start = self.pos;
                let end = line_end(self.bytes, self.pos);
                let line = &self.text[self.pos..end];
                self.pos = (end + 1).min(self.bytes.len());
                let last = line.trim_end_matches('\r');
                let stripped = if strip {
                    last.trim_start_matches('\t')
                } else {
                    last
                };
                if stripped == word {
                    break;
                }
                // In `$(…)`, bash also ends the body at `EOF)`, and the `)`
                // closes the substitution.
                if self.substitutions > 0
                    && let Some(rest) = stripped.strip_prefix(word.as_str())
                    && rest.trim_start().starts_with(')')
                {
                    self.pos = line_start + (line.len() - rest.len());
                    break;
                }
                lines.push(line);
            }
            self.bodies.push(Body {
                start,
                end: self.pos,
                text: lines.join("\n"),
                owner,
                expands,
                // A line end in a `$(…)` opened on the heredoc's line: bash
                // reads the body only after that line, so where it is is not
                // sure.
                uncertain: opened != depth,
            });
        }
    }

    /// Reads a nested list (a substitution or a subshell, which starts at
    /// `at`) up to `close`, held by the part numbered `holder`.
    fn nested(&mut self, scope: &[u32], close: u8, depth: usize, holder: usize, at: usize) {
        let child = self.child(scope);
        if depth >= NESTING {
            // Too deep to follow: kept whole, so that patterns still see it.
            let start = self.pos;
            self.pos = match close {
                b')' => skip_parens(self.bytes, start - 1),
                _ => skip_backquote(self.bytes, start - 1),
            };
            self.entries.push(Entry {
                run: whole(&self.text[start..self.pos], child),
                piped_into: None,
            });
            return;
        }
        // The list inside is not in the quote that holds it.
        let quote = self.quote_open.take();
        if let Some(&id) = child.last() {
            self.holders.insert(
                id,
                Holder {
                    part: holder,
                    at,
                    quote,
                },
            );
        }
        self.list(&child, Some(close), depth + 1);
        self.quote_open = quote;
    }

    /// Reads a `$(…)` (or `$((…))`) whose `$` is at `i`.
    fn dollar(&mut self, i: usize, scope: &[u32], depth: usize, holder: usize) {
        match arithmetic_end(self.bytes, i + 1) {
            Some(end) => self.pos = end,
            None => {
                self.pos = i + 2;
                self.substitutions += 1;
                self.nested(scope, b')', depth, holder, i);
                self.substitutions -= 1;
            }
        }
    }

    /// Reads a double-quoted string from its opening quote.
    fn double(&mut self, scope: &[u32], depth: usize, holder: usize) {
        let outer = self.quote_open.replace(self.pos);
        self.pos += 1;
        self.expanded_until(Some(b'"'), scope, depth, holder);
        self.quote_open = outer;
    }

    /// Reads expanded text (the inside of double quotes, an expanded
    /// heredoc body) to the end.
    fn expanded(&mut self, scope: &[u32], depth: usize, holder: usize) {
        self.expanded_until(None, scope, depth, holder);
    }

    fn expanded_until(&mut self, close: Option<u8>, scope: &[u32], depth: usize, holder: usize) {
        let esc = escape(self.dialect);
        let bash = self.dialect == Dialect::Bash;
        // In double quotes, a `${…}` holds quotes of its own
        // (`"${x:-"it's"}"`).
        let mut braces = 0usize;
        while let Some(b) = self.peek(self.pos) {
            let i = self.pos;
            match b {
                _ if b == esc => self.pos = after_escape(self.bytes, i),
                b'$' if bash && close.is_some() && self.peek(i + 1) == Some(b'{') => {
                    braces += 1;
                    self.pos = i + 2;
                }
                b'}' if braces > 0 => {
                    braces -= 1;
                    self.pos = i + 1;
                }
                b'"' if braces > 0 && depth < NESTING => self.double(scope, depth + 1, holder),
                _ if Some(b) == close => {
                    self.pos = i + 1;
                    return;
                }
                b'$' if self.peek(i + 1) == Some(b'(') => self.dollar(i, scope, depth, holder),
                b'$' if self.dialect == Dialect::Bash && self.peek(i + 1) == Some(b'[') => {
                    self.pos = skip_brackets(self.bytes, i + 1);
                }
                b'`' if self.dialect == Dialect::Bash => {
                    self.pos = i + 1;
                    self.nested(scope, b'`', depth, holder, i);
                }
                _ => self.pos = i + 1,
            }
        }
    }

    /// Reads a list of commands up to `close` (or the end), in `scope`.
    fn list(&mut self, scope: &[u32], close: Option<u8>, depth: usize) {
        let bash = self.dialect == Dialect::Bash;
        let esc = escape(self.dialect);
        let part = self.new_part();
        let mut list = List {
            start: self.pos,
            part,
            last: None,
            piping: false,
            function_next: false,
            function: None,
        };
        // How many `${` are open: in one, `<<` and `#` are text (`${x/<<EOF/}`,
        // `${x:- #}`).
        let mut braces = 0usize;
        loop {
            let i = self.pos;
            let Some(b) = self.peek(i) else {
                self.finish(&mut list, i, scope, false);
                return;
            };
            if Some(b) == close {
                self.finish(&mut list, i, scope, false);
                self.pos = i + 1;
                return;
            }
            let next = self.peek(i + 1);
            match b {
                _ if b == esc => self.pos = after_escape(self.bytes, i),
                b'\'' => self.pos = skip_single(self.bytes, i),
                b'"' => self.double(scope, depth, list.part),
                b'@' if !bash && matches!(next, Some(b'\'' | b'"')) => {
                    self.pos = skip_here_string(self.bytes, i);
                }
                // A PowerShell block comment, `<# … #>`.
                b'<' if !bash && next == Some(b'#') => {
                    self.pos = self.text[i + 2..]
                        .find("#>")
                        .map_or(self.bytes.len(), |e| i + 2 + e + 2);
                    self.odd_comments = true;
                }
                b'$' if bash && next == Some(b'{') => {
                    braces += 1;
                    self.pos = i + 2;
                }
                b'}' if braces > 0 => {
                    braces -= 1;
                    self.pos = i + 1;
                }
                _ if braces == 0 && starts_comment(self.bytes, i) => {
                    self.finish(&mut list, i, scope, false);
                    self.pos = line_end(self.bytes, i);
                    // Only a comment that is a whole line is surely one
                    // (`$(date)#`, `a\ #` and `=~ ^(#|a)` are words).
                    if whole_line_comment(self.bytes, i, esc) {
                        self.comments.push((i, self.pos));
                    } else {
                        self.odd_comments = true;
                    }
                    list.start = self.pos;
                }
                b'$' if next == Some(b'(') => self.dollar(i, scope, depth, list.part),
                b'$' if bash && next == Some(b'\'') => self.pos = skip_ansi_c(self.bytes, i + 1),
                // Old arithmetic, `$[1 << 2]`.
                b'$' if bash && next == Some(b'[') => self.pos = skip_brackets(self.bytes, i + 1),
                b'<' | b'>' if bash && next == Some(b'(') => {
                    self.pos = i + 2;
                    self.nested(scope, b')', depth, list.part, i);
                }
                b'`' if bash => {
                    self.pos = i + 1;
                    self.nested(scope, b'`', depth, list.part, i);
                }
                // A function's definition, `name() { … }`: the name is not
                // run, and the body that follows is read in its own scope.
                b'(' if bash
                    && self.text[i + 1..].trim_start().starts_with(')')
                    && FUNCTION_NAME.is_match(self.without_bodies(list.start, i).trim()) =>
                {
                    self.pos = i + 1 + self.text[i + 1..].find(')').unwrap_or(0) + 1;
                    list.start = self.pos;
                    list.function_next = true;
                }
                b'(' => match arithmetic_end(self.bytes, i).filter(|_| bash) {
                    // An arithmetic command, `(( x << 2 ))`.
                    Some(end) => self.pos = end,
                    None => {
                        self.finish(&mut list, i, scope, false);
                        self.pos = i + 1;
                        let holder = list.part;
                        let before = self.entries.len();
                        let piped_from = list.last.filter(|_| list.piping);
                        self.nested(scope, b')', depth, holder, i);
                        if self.entries.len() > before {
                            // What pipes into a subshell reaches its first
                            // command (`| (bash)`), and what pipes on from it
                            // is what its last command prints (`(cat <<EOF)
                            // | bash`).
                            if let Some(from) = piped_from {
                                self.entries[from].piped_into = Some(before);
                            }
                            list.last = Some(self.entries.len() - 1);
                        }
                        list.piping = false;
                        list.start = self.pos;
                    }
                },
                b')' => {
                    self.finish(&mut list, i, scope, false);
                    self.pos = i + 1;
                    list.start = self.pos;
                }
                b'<' if bash && braces == 0 => match marker_at(self.text, i) {
                    Some(marker) => {
                        self.pos = marker.end;
                        self.pending.push((marker, list.part, depth));
                    }
                    None => self.pos = i + 1,
                },
                // `|&` pipes standard error too.
                b'|' if next == Some(b'&') => {
                    self.finish(&mut list, i, scope, true);
                    self.pos = i + 2;
                    list.start = self.pos;
                }
                b'&' | b'|' => {
                    let doubled = next == Some(b);
                    // `2>&1`, `0<&3`, `&>` and `>|` redirect rather than separate.
                    let redirect = !doubled
                        && ((i > 0 && matches!(self.bytes[i - 1], b'>' | b'<'))
                            || (b == b'&' && next == Some(b'>')));
                    if redirect {
                        self.pos = i + 1;
                    } else {
                        let before = self.entries.len();
                        self.finish(&mut list, i, scope, b == b'|' && !doubled);
                        // What `&` sends to the background runs in a
                        // subshell of its own (`cd x & git push`).
                        if b == b'&' && !doubled && self.entries.len() > before {
                            let background = self.child(scope);
                            for entry in &mut self.entries[before..] {
                                entry.run.scope = background.clone();
                            }
                        }
                        self.pos = i + if doubled { 2 } else { 1 };
                        list.start = self.pos;
                    }
                }
                b';' => {
                    self.finish(&mut list, i, scope, false);
                    self.pos = i + 1;
                    list.start = self.pos;
                }
                // In a `${…}`, a line end is part of the word.
                b'\n' if braces > 0 => self.pos = i + 1,
                b'\n' => {
                    self.finish(&mut list, i, scope, false);
                    self.pos = i + 1;
                    self.read_bodies(depth);
                    list.start = self.pos;
                }
                _ => self.pos = i + 1,
            }
        }
    }

    /// The command after which what the heredoc opened by the entry
    /// `owner` runs, if it runs: a shell reading its input, the owner or
    /// one it pipes into (`cat <<EOF | tee log | bash`); or, for a `cat
    /// <<EOF` printing it, where what the substitution holding the `cat`
    /// prints runs as a command (`bash -c "$(cat <<EOF …)"`, `$(cat <<EOF
    /// …)` alone, `bash -c "set -e; $(…)"`) or as a script (`bash <(cat
    /// <<EOF …)`).
    fn shell_for(&self, owner: usize) -> Option<usize> {
        let mut at = Some(owner);
        while let Some(entry) = at {
            if !self.check() {
                return None;
            }
            if reads_input(&self.entries[entry].run.text) {
                return Some(entry);
            }
            at = self.entries[entry].piped_into;
        }
        let run = &self.entries[owner].run;
        if !PRINTS_INPUT.is_match(&run.text) {
            return None;
        }
        let holder = *self.holders.get(run.scope.last()?)?;
        let entry = self.parts[holder.part]?;
        let runs = if self.bytes[holder.at] == b'<' {
            runs_script_from_substitution(&self.entries[entry].run.text)
        } else {
            match holder.quote {
                // In double quotes: at a command's place in what a shell
                // is handed as its command.
                Some(open) => {
                    RUNS_ARG.is_match(before(self.text, open))
                        && hands_to_shell(&self.entries[entry].run.text)
                        && at_command_place(&self.text[open + 1..holder.at])
                }
                None => at_command_place(&self.text[..holder.at]),
            }
        };
        runs.then_some(entry)
    }

    /// For a heredoc the entry `owner` writes to a file (`cat <<EOF >
    /// s.sh`, `tee s.sh <<EOF`, `cat <<EOF | tee s.sh`, `install /dev/stdin
    /// s.sh <<EOF`), a later command that runs that file (`bash s.sh`,
    /// `bash -e ./s.sh`, `source s.sh`, `./s.sh`, `bash < s.sh`, `cat s.sh |
    /// bash`).
    fn runner_of_file(&self, owner: usize) -> Option<usize> {
        // Standard output to a file: not `2>`, and not `/dev/null`.
        static REDIRECTED: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"(?:^|[^0-9&])>>?\|?\s*("[^"]+"|'[^']+'|/dev/[^\s;&|<>]+|[^\s;&|<>]+)"#)
                .unwrap()
        });
        static TEE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"^tee(\s+-\S+)*\s+("[^"]+"|'[^']+'|[^\s;&|<>]+)"#).unwrap()
        });
        static INSTALL: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"^install\s.*?/dev/stdin\s+("[^"]+"|'[^']+'|[^\s;&|<>]+)"#).unwrap()
        });
        static DD: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"^dd\s(.*\s)?of=("[^"]+"|'[^']+'|[^\s;&|<>]+)"#).unwrap()
        });
        static READS_FROM: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r#"<\s*("[^"]+"|'[^']+'|[^\s;&|<>]+)"#).unwrap());
        // The same file, however it is written (`./f`, `"$PWD/f"`,
        // `./"$TMP/f"`).
        let plain = |file: &str| {
            let mut file = file.to_string();
            loop {
                let trimmed = file.trim_matches(['"', '\'']);
                let stripped = ["$PWD/", "${PWD}/", "$(pwd)/", "`pwd`/", "./"]
                    .iter()
                    .find_map(|p| trimmed.strip_prefix(p))
                    .unwrap_or(trimmed);
                if stripped == file {
                    return file;
                }
                file = stripped.to_string();
            }
        };
        let written = |entry: usize| {
            let text = &self.entries[entry].run.text;
            let named = [&*TEE, &*INSTALL, &*DD]
                .iter()
                .find_map(|re| re.captures(text))
                .map(|c| plain(c.get(c.len() - 1).map_or("", |m| m.as_str())));
            named.or_else(|| {
                REDIRECTED
                    .captures_iter(text)
                    .map(|c| plain(&c[1]))
                    .find(|file| !file.starts_with("/dev/"))
            })
        };
        let mut at = Some(owner);
        let mut file = None;
        while let Some(entry) = at {
            if !self.check() {
                return None;
            }
            file = written(entry);
            if file.is_some() {
                break;
            }
            at = self.entries[entry].piped_into;
        }
        let file = file?;
        let name = file.rsplit(['/', '\\']).next().unwrap_or(&file);
        let runs_it = |later: usize| {
            let command = &self.entries[later].run.text;
            // Only a command that names the file may run it: the checks
            // below are kept to those.
            if !command.contains(name) {
                return false;
            }
            let first = command.split_whitespace().next().unwrap_or("");
            if plain(first) == file {
                return true;
            }
            if let Some((rest, name)) = after_runner(command) {
                // PowerShell (`-File f`) and cmd (`/c f`) run a file named
                // among their words.
                if matches!(name.as_str(), "pwsh" | "powershell" | "cmd")
                    && rest.split_whitespace().any(|w| plain(w) == file)
                {
                    return true;
                }
                let blanked = REDIRECT.replace_all(rest, " ");
                let mut words = blanked.split_whitespace();
                // The first word after the options (`-o pipefail` takes a value).
                let script = loop {
                    let Some(word) = words.next() else {
                        break None;
                    };
                    if !word.starts_with(['-', '+']) {
                        break Some(word);
                    }
                    if word.ends_with(['o', 'O']) || matches!(word, "--rcfile" | "--init-file") {
                        words.next();
                    }
                };
                if script.is_some_and(|w| plain(w) == file) {
                    return true;
                }
                if READS_FROM
                    .captures(rest)
                    .is_some_and(|c| plain(&c[1]) == file)
                    && reads_input(command)
                {
                    return true;
                }
            }
            // `cat s.sh | bash`
            command.starts_with("cat ")
                && command
                    .split_whitespace()
                    .nth(1)
                    .is_some_and(|w| plain(w) == file)
                && self.entries[later]
                    .piped_into
                    .is_some_and(|next| reads_input(&self.entries[next].run.text))
        };
        (owner + 1..self.entries.len())
            .take_while(|_| self.check())
            .find(|&later| runs_it(later))
            .map(|later| match self.entries[later].piped_into {
                Some(next) if command_is_cat(&self.entries[later].run.text) => next,
                _ => later,
            })
    }

    /// Reads the whole text in `scope`; what heredocs given to a shell run
    /// follows that shell, and what an expanded heredoc substitutes follows
    /// the command it is given to.
    fn parse(&mut self, scope: &[u32]) -> Vec<Run> {
        self.list(scope, None, 0);
        self.checks.set(self.text.len().max(1 << 12));
        let evals = self.entries.iter().any(|e| EVALS.is_match(&e.run.text));
        let mut inserts: Vec<(usize, Vec<Run>)> = Vec::new();
        for i in 0..self.bodies.len() {
            let Some(owner) = self.parts[self.bodies[i].owner] else {
                // Given to what only closes a loop (`done <<EOF`): its lines
                // run when the loop runs them (`while read l; do eval
                // "$l"; done <<EOF`).
                if evals {
                    let text = self.bodies[i].text.clone();
                    let scope = self.child(scope);
                    let runs = self.sub(&text, Dialect::Bash, scope);
                    inserts.push((self.entries.len(), runs));
                }
                continue;
            };
            let text = self.bodies[i].text.clone();
            if let Some(shell) = self.shell_for(owner).or_else(|| self.runner_of_file(owner)) {
                let scope = self.entries[shell].run.scope.clone();
                let scope = self.child(&scope);
                let runs = self.sub(&text, Dialect::Bash, scope);
                inserts.push((shell + 1, runs));
            } else if self.bodies[i].expands {
                let scope = self.entries[owner].run.scope.clone();
                let runs = self.expansions(&text, scope);
                inserts.push((owner, runs));
            }
        }
        let mut runs: Vec<Run> = self.entries.iter().map(|e| e.run.clone()).collect();
        // Later places first, so the earlier ones stay where they are; the
        // sort is stable, so bodies for one place keep their order.
        inserts.sort_by_key(|(at, _)| *at);
        for (at, more) in inserts.into_iter().rev() {
            runs.splice(at..at, more);
        }
        self.runs = runs.clone();
        runs
    }
}

/// Commands that only pass on (or count) what they read, and write no file
/// of their own choosing (`tee` writes the ones it is given): a heredoc
/// fed through them goes where their output goes.
static PASSES_ON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(cat|tee|tr|head|tail|cut|wc|grep|nl|fold)(\s|$)").unwrap());
/// Commands that read a message, a body or notes from their input as text
/// (`git commit -F -`, `gh pr create --body-file -`).
static READS_TEXT_INPUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(git(\s+-\S+(\s+[^-\s]\S*)?)*\s+(commit|tag|notes)\s(.*\s)?(-F|--file)|gh\s(.*\s)?(--body-file|-F|--notes-file|--input))(=|\s+)-(\s|$)",
    )
    .unwrap()
});
/// Commands that take text as an argument (a message, a PR body) and do
/// not run it.
static TAKES_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(git(\s+-\S+(\s+[^-\s]\S*)?)*\s+(commit|tag|notes)|gh)(\s|$)").unwrap()
});
/// `gh alias`, which makes commands of the text it is given.
static GH_ALIAS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^gh\s+alias(\s|$)").unwrap());
/// Files of text to read, not to run (a PR body, notes): other files may
/// be run by their place or name (hooks, `Makefile`, `package.json`).
static TEXT_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\.(md|markdown|txt|rst|patch|diff)$").unwrap());
/// Commands that run nothing they are given or read, and leave this shell
/// as it was: only when every command is one is any text shown to be data
/// ([`Parser::data`]).
static INERT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(cat|tee|tr|head|tail|cut|wc|grep|nl|fold|rm|mkdir|cd|pushd|popd|echo|printf|true|false|sleep|pwd|git(\s+-C\s+\S+)*\s+(add|commit|push|status|log|show|branch|rev-parse)|gh\s+(pr|issue|release|api|label|search|run))(\s|$)",
    )
    .unwrap()
});
/// Options with which git runs a program (an editor, `--upload-pack`) or
/// takes settings that may (`-c`).
static GIT_RUNS_PROGRAM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^git\s(.*\s)?(-c|-e|--edit|-t|--template|--exec|--upload-pack|--receive-pack)(\s|=|$)",
    )
    .unwrap()
});
/// Arithmetic, whose inside is not read here (`$(( $(…) ))`).
static ARITHMETIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\(\(|\$\[|\(\(").unwrap());
/// An assignment (`GIT_EDITOR=…`), which may change what a command runs.
static ASSIGNS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[\s;&|(])[A-Za-z_][A-Za-z0-9_]*\+?=").unwrap());
static TEE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^tee(\s|$)").unwrap());

/// Where a command's standard output goes, by its redirections.
enum Output {
    /// Where the command's own output goes (the terminal, a pipe).
    On,
    /// Nowhere (`> /dev/null`, `>&-`).
    Null,
    File(String),
    /// Somewhere not followed (`>&3`, `> >(bash)`, `1<>f`).
    Unknown,
}

fn output_of(text: &str) -> Output {
    let blanked = unquoted(text);
    let mut out = Output::On;
    for m in REDIRECT.find_iter(&blanked) {
        let raw = &text[m.start()..m.end()];
        let fd = raw.bytes().take_while(u8::is_ascii_digit).count();
        let rest = &raw[fd..];
        if rest.starts_with("<>") && &raw[..fd] == "1" {
            out = Output::Unknown;
            continue;
        }
        if rest.starts_with('<') {
            continue;
        }
        let both = rest.starts_with('&');
        if !both && fd > 0 && &raw[..fd] != "1" {
            continue;
        }
        let target = rest
            .trim_start_matches('&')
            .trim_start_matches(['>', '|'])
            .trim();
        out = match target {
            // Not standard error: it may be sent to a file (`2>s.sh >&2`).
            "&1" | "/dev/stdout" | "/dev/tty" => Output::On,
            "&-" | "/dev/null" => Output::Null,
            _ if target.is_empty() || target.starts_with(['&', '(']) => Output::Unknown,
            _ if target.starts_with("/dev/") => Output::Unknown,
            _ => Output::File(target.to_string()),
        };
    }
    out
}

/// The files `tee` writes, as written; none when an option is not known
/// (`tee -- -s.sh` writes `-s.sh`).
fn tee_files(text: &str) -> Option<Vec<&str>> {
    let unquoted = unquoted(text);
    let blanked = REDIRECT.replace_all(&unquoted, |c: &regex::Captures| " ".repeat(c[0].len()));
    let mut files = Vec::new();
    let mut at = 0;
    for word in blanked.split_whitespace().skip(1) {
        let start = blanked[at..].find(word).map_or(at, |e| at + e);
        at = start + word.len();
        match word {
            "-a" | "-i" | "-p" | "--append" | "--ignore-interrupts" => {}
            _ if word.starts_with('-') => return None,
            _ => files.push(&text[start..at]),
        }
    }
    Some(files)
}

/// What the proofs below read of the whole command.
struct Outer {
    /// Whether it names something under `.git` or `hooks`, where a file
    /// written may be run (`.git/hooks/pre-push`).
    near_hooks: bool,
}

impl Parser<'_> {
    /// The byte ranges of `self.text` (read with [`Parser::parse`]) that
    /// are data whatever a pattern finds in them, as far as it can be shown
    /// (see [`runs_in`]), in a command simple enough ([`Parser::simple`]):
    /// comments that are whole lines, and heredoc bodies with a match of
    /// `re` that only feed text to a command that reads it as text (`git
    /// commit -F -`, `gh pr create --body "$(cat <<'EOF' …)"`), are printed
    /// (`cat <<'EOF'`), or are written to a file of text.
    fn data(&self, re: &Regex) -> Vec<(usize, usize)> {
        if !self.simple() {
            return Vec::new();
        }
        let mut data = self.comments.clone();
        let candidates: Vec<usize> = (0..self.bodies.len())
            .filter(|&i| crate::text::any_line(re, &self.bodies[i].text))
            .collect();
        if candidates.is_empty() {
            return data;
        }
        let ranges: Vec<(usize, usize)> = self.bodies.iter().map(|b| (b.start, b.end)).collect();
        let lower = blank(self.text, &ranges, false).to_lowercase();
        let outer = Outer {
            near_hooks: lower.contains(".git") || lower.contains("hooks"),
        };
        // Following a pipe costs the length of each command in it: past the
        // budget, a body is not shown to be data.
        let mut budget = self.text.len().saturating_mul(REREAD).max(1 << 16);
        for i in candidates {
            if self.body_is_data(i, &outer, &mut budget) {
                data.push((self.bodies[i].start, self.bodies[i].end));
            }
        }
        data
    }

    /// Whether the command is simple enough for any of its text to be
    /// shown to be data: Bash, every command it runs [`INERT`] (so no
    /// function, alias, `hash`, `exec`, `eval` or `source`) with git running
    /// no program ([`GIT_RUNS_PROGRAM`]), and no assignment, arithmetic,
    /// comment inside a line, or heredoc whose place is not sure. Bash runs
    /// whatever it is given, so past this the text is read as plain text.
    fn simple(&self) -> bool {
        if self.dialect != Dialect::Bash
            || self.odd_comments
            || self.bodies.iter().any(|b| b.uncertain)
        {
            return false;
        }
        let ranges: Vec<(usize, usize)> = self.bodies.iter().map(|b| (b.start, b.end)).collect();
        let outside = blank(self.text, &ranges, false);
        if ASSIGNS.is_match(&unquoted(&outside)) || ARITHMETIC.is_match(&outside) {
            return false;
        }
        self.runs.iter().all(|run| {
            let text = unquoted(&run.text);
            INERT.is_match(&text) && !GIT_RUNS_PROGRAM.is_match(&text)
        })
    }

    fn body_is_data(&self, i: usize, outer: &Outer, budget: &mut usize) -> bool {
        let body = &self.bodies[i];
        if body.uncertain {
            return false;
        }
        // What an expanded body substitutes runs (read in `parse`), also
        // over continued lines (`$\` then `(…)`).
        if body.expands {
            let joined = body.text.replace("\\\r\n", "").replace("\\\n", "");
            if joined.contains("$(") || joined.contains('`') || joined.contains("${") {
                return false;
            }
        }
        let Some(mut at) = self.parts[body.owner] else {
            return false;
        };
        loop {
            let text = &self.entries[at].run.text;
            if *budget < text.len() {
                return false;
            }
            *budget -= text.len();
            let blanked = unquoted(text);
            if READS_TEXT_INPUT.is_match(&blanked) && !GH_ALIAS.is_match(&blanked) {
                return true;
            }
            if !PASSES_ON.is_match(&blanked)
                || ["$(", "`", "<(", ">("].iter().any(|s| text.contains(s))
            {
                return false;
            }
            if TEE.is_match(&blanked)
                && !tee_files(text)
                    .is_some_and(|files| files.iter().all(|f| file_is_data(f, outer)))
            {
                return false;
            }
            match output_of(text) {
                Output::Null => return true,
                Output::File(file) => return file_is_data(&file, outer),
                Output::Unknown => return false,
                Output::On => match self.entries[at].piped_into {
                    Some(next) => at = next,
                    None => return self.printed_is_data(at),
                },
            }
        }
    }

    /// Whether what the entry `at` prints is data: at the top of the
    /// command, or substituted into an argument of a command that takes
    /// text ([`TAKES_TEXT`]).
    fn printed_is_data(&self, at: usize) -> bool {
        let scope = &self.entries[at].run.scope;
        let Some(last) = scope.last() else {
            return !self.output_moved;
        };
        // A function's body or the background has no holder.
        let Some(holder) = self.holders.get(last) else {
            return false;
        };
        if !matches!(self.bytes[holder.at], b'$' | b'`') {
            return false;
        }
        self.parts[holder.part].is_some_and(|entry| {
            let text = unquoted(&self.entries[entry].run.text);
            TAKES_TEXT.is_match(&text) && !GH_ALIAS.is_match(&text)
        })
    }
}

/// Whether a file a heredoc body is written to stays data: named as
/// written (no variable, `~` or glob), a file of text ([`TEXT_FILE`]), with
/// nothing under `.git` or `hooks` named (the command is
/// [`Parser::simple`], so nothing in it runs the file).
fn file_is_data(file: &str, outer: &Outer) -> bool {
    let path = file.trim_matches(['"', '\'']);
    if path.starts_with("/dev/") {
        return matches!(path, "/dev/null" | "/dev/stdout" | "/dev/tty");
    }
    !path.is_empty()
        && !path.contains(['$', '`', '~', '*', '?', '[', '{', '(', '"', '\''])
        && TEXT_FILE.is_match(path)
        && !outer.near_hooks
}

/// Commands whose `-c` takes text that runs no code (a count, an archive or
/// a cookie file to write): the quoted text after it is data.
fn c_takes_text(head: &str) -> bool {
    static C_IS_TEXT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^(grep|egrep|fgrep|rg|ag|wc|uniq|head|tail|cut|sort|tar|zip|unzip|curl|wget|jq|yq|gcc|g\+\+|cc|clang)(\.exe)?$",
        )
        .unwrap()
    });
    let first = head.split_whitespace().next().unwrap_or("");
    let first = first.trim_matches(['"', '\'']);
    C_IS_TEXT.is_match(first.rsplit(['/', '\\']).next().unwrap_or(first))
}

/// Whether `found` accepts a line of `text` read as plain text, with
/// quoted strings taken out unless they come after `-c` (`/c`,
/// `-Command`) and may run (`bash -c "…"`, `git -c 'alias.p=!git push'`):
/// the plain match under the reading here, so that a form it does not
/// follow is still found (see [`runs_in`]).
pub fn as_text_finds(text: &str, found: &dyn Fn(&str) -> bool) -> bool {
    as_text_finds_in(text, found, 0)
}

fn as_text_finds_in(text: &str, found: &dyn Fn(&str) -> bool, depth: usize) -> bool {
    static QUOTED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"'[^']*'|"(?:[^"\\]|\\.)*""#).unwrap());
    static RUNS_ARG_ALONE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(^|\s)(-c|/c|-command)\s*$").unwrap());
    let mut masked = String::with_capacity(text.len());
    let mut last = 0;
    for quoted in QUOTED.find_iter(text) {
        masked.push_str(&text[last..quoted.start()]);
        let inner = &quoted.as_str()[1..quoted.len() - 1];
        let lead = before(text, quoted.start());
        if depth < NESTING && RUNS_ARG_ALONE.is_match(lead) {
            let head = lead
                .rfind([';', '&', '|', '\n', '('])
                .map_or(lead, |e| &lead[e + 1..]);
            let head = clean(head, Dialect::Bash);
            if (!c_takes_text(&head) || SHELL_RUNS_ARG.is_match(lead))
                && as_text_finds_in(inner, found, depth + 1)
            {
                return true;
            }
        }
        // Same length, no line breaks: lines keep their places.
        masked.extend(
            quoted
                .as_str()
                .chars()
                .map(|c| if c == '\n' { '\n' } else { 'x' }),
        );
        last = quoted.end();
    }
    masked.push_str(&text[last..]);
    crate::text::lines(&masked).any(found)
}

/// The parts of `command` for what only [`as_text_finds`] finds, split
/// plainly: at `&&`, `||`, `;`, `|` and line ends outside quotes, with
/// leading `(`, `{`, `VAR=value`, `timeout <n>`, `nohup`, `exec` and
/// `time` taken off.
pub fn as_text_parts(command: &str) -> Vec<&str> {
    static QUOTED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"'[^']*'|"(?:[^"\\]|\\.)*""#).unwrap());
    static SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&&|\|\||[;|\n]").unwrap());
    static LEADING: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(?:[({]\s*|[A-Za-z_][A-Za-z0-9_]*=\S*\s+|timeout\s+\S+\s+|nohup\s+|exec\s+|time\s+)",
        )
        .unwrap()
    });
    // Quoted text keeps its length, so the separators found in the masked
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
            while let Some(m) = LEADING.find(head) {
                head = head[m.end()..].trim_start();
            }
            (!head.is_empty()).then_some(head)
        })
        .collect()
}

/// `text`, past the budget for reading it again, kept whole as one
/// command, its quotes taken out, so that a pattern still finds what is
/// in it.
fn whole(text: &str, scope: Vec<u32>) -> Run {
    Run {
        text: text.replace(['\'', '"', '`'], " "),
        scope,
    }
}

/// `text` (a double-quoted argument) with its substitutions turned into
/// `$xxx…`: the shell around expands them before handing the text on,
/// and has read them already. What they give is not known, as a
/// variable's value is not (`cd $(git rev-parse --show-toplevel)`).
fn without_substitutions(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let end = match bytes[i] {
            b'\\' => {
                i = after_escape(bytes, i);
                continue;
            }
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                skip_substitution(bytes, i + 1, Dialect::Bash, 0)
            }
            b'`' => skip_backquote(bytes, i),
            _ => {
                i += 1;
                continue;
            }
        };
        found.push((i, end));
        i = end;
    }
    let mut blanked = blank(text, &found, true).into_bytes();
    for (start, _) in found {
        blanked[start] = b'$';
    }
    String::from_utf8(blanked).unwrap_or_default()
}

/// The commands `command` runs, in order, with the list each runs in.
pub fn runs_of(command: &str, dialect: Dialect) -> Vec<Run> {
    Parser::root(command, dialect, 0).parse(&[])
}

/// The commands `command` (Bash) runs, in order, each as written (quotes
/// kept; continued lines joined; heredoc bodies left out) without leading
/// `VAR=value`, `timeout <n>`, `nohup`, `exec`, `time`, `then`, `do`,
/// `else`, `(` and `{`. Parts are split at `&&`, `||`, `;`, `|`, `&`,
/// parentheses and line ends outside quotes; see the module doc for what
/// else counts.
pub fn commands(command: &str) -> Vec<String> {
    commands_in(command, Dialect::Bash)
}

/// [`commands`] for a command of `dialect`.
pub fn commands_in(command: &str, dialect: Dialect) -> Vec<String> {
    runs_of(command, dialect)
        .into_iter()
        .map(|run| run.text)
        .collect()
}

/// Whether `command` (Bash) runs a command that `re` matches, quoted text
/// in it blanked.
pub fn runs(re: &Regex, command: &str) -> bool {
    runs_in(re, command, Dialect::Bash)
}

/// [`runs`] for a command of `dialect`. What the reading here finds
/// counts, and so does a match anywhere outside quotes ([`as_text_finds`];
/// quoted text after `-c` is read too), but not in text shown to be data
/// (`Parser::data`): the reading here may not follow every way a shell
/// can be handed text, and the plain match stays under it.
pub fn runs_in(re: &Regex, command: &str, dialect: Dialect) -> bool {
    let mut parser = Parser::root(command, dialect, 0);
    if parser
        .parse(&[])
        .iter()
        .any(|run| re.is_match(&unquoted_in(&run.text, dialect)))
    {
        return true;
    }
    let data = parser.data(re);
    as_text_finds(&blank(command, &data, false), &|line| re.is_match(line))
}

/// The command (Bash) without its heredoc bodies and their end lines, and
/// the bodies, as bash reads them (a `<<` in quotes, a comment, a
/// here-string or arithmetic opens none).
pub(crate) fn split_heredocs(command: &str) -> (String, Vec<String>) {
    let mut parser = Parser::root(command, Dialect::Bash, SHELLS);
    parser.list(&[], None, 0);
    let kept = parser.without_bodies(0, command.len());
    (kept, parser.bodies.into_iter().map(|b| b.text).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heredocs(command: &str) -> (String, Vec<String>) {
        split_heredocs(command)
    }

    #[test]
    fn takes_out_heredocs() {
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "cat <<'EOF' > a\nx\nEOF\ncat <<- \"X\"\n\ty\n\tX\nrest",
                "cat <<'EOF' > a\ncat <<- \"X\"\nrest",
                &["x", "\ty"],
            ),
            (
                "a <<A b <<B\n1\nA\n2\nB\nend",
                "a <<A b <<B\nend",
                &["1", "2"],
            ),
            (
                "cat > a <<'EOF'\r\nx\r\nEOF\r\ngit push",
                "cat > a <<'EOF'\r\ngit push",
                &["x\r"],
            ),
            (
                "cat > a <<'EOF'\nline \\\nEOF\ngit push",
                "cat > a <<'EOF'\ngit push",
                &["line \\"],
            ),
            (
                "gh pr create --body \"$(cat <<'EOF'\nbody's (text\nEOF\n)\"",
                "gh pr create --body \"$(cat <<'EOF'\n)\"",
                &["body's (text"],
            ),
            (
                "gh pr create --body \"Here's: $(cat <<'EOF'\nSay \"hi\nEOF\n)\"",
                "gh pr create --body \"Here's: $(cat <<'EOF'\n)\"",
                &["Say \"hi"],
            ),
            (
                "gh pr create --title \"it's\" --body \"$(cat <<'EOF'\nSay \"hi\nEOF\n)\"",
                "gh pr create --title \"it's\" --body \"$(cat <<'EOF'\n)\"",
                &["Say \"hi"],
            ),
            (
                "gh pr create --body \"$(cat <<'EOF'\nit's `x\nEOF\n)\"",
                "gh pr create --body \"$(cat <<'EOF'\n)\"",
                &["it's `x"],
            ),
            (
                "x=`cat <<EOF\nbody\nEOF\n`\ngit push",
                "x=`cat <<EOF\n`\ngit push",
                &["body"],
            ),
            (
                "cat <<\\EOF | bash\ngit push\nEOF",
                "cat <<\\EOF | bash\n",
                &["git push"],
            ),
            (
                "cat <<EOF \"a\nb\"\nbody\nEOF\ngit push",
                "cat <<EOF \"a\nb\"\ngit push",
                &["body"],
            ),
            (
                "echo $(( 1 << 2 )) <<EOF\nx\nEOF\ngit push",
                "echo $(( 1 << 2 )) <<EOF\ngit push",
                &["x"],
            ),
        ];
        for (command, kept, bodies) in cases {
            let (k, b) = heredocs(command);
            assert_eq!(
                (k.as_str(), b),
                (
                    *kept,
                    bodies.iter().map(|s| s.to_string()).collect::<Vec<_>>()
                ),
                "{command}"
            );
        }
        for (command, why) in [
            ("git commit -m \"use <<EOF\"\ngit push", "in quotes"),
            ("echo 'a<<b'\ngit push", "in single quotes"),
            ("# cat <<EOF\ngit push", "in a comment"),
            ("cat <<<\"EOF\"\ngit push", "a here-string"),
            ("echo $((1<<EOF))\ngit push", "arithmetic"),
            ("echo \"$((1<<EOF))\"\ngit push", "arithmetic in quotes"),
            ("(( x<<n ))\ngit push", "an arithmetic command"),
            ("cat <<'EOF", "not closed"),
        ] {
            assert!(heredocs(command).1.is_empty(), "{why}: {command}");
        }
    }

    #[test]
    fn blanks_quotes_byte_for_byte() {
        let text = r#"git commit -m "日本語 \"x\"" && echo 'a b'"#;
        let blanked = unquoted(text);
        assert_eq!(blanked.len(), text.len());
        assert!(blanked.starts_with("git commit -m xxx"));
        assert!(blanked.ends_with("&& echo xxxxx"));
        assert_eq!(unquoted("a\\\"b"), "a\\\"b", "escaped quote");
        let quoted = r#""$(echo "git push")""#;
        assert_eq!(
            unquoted(&format!("gh pr create --body {quoted}")),
            format!("gh pr create --body {}", "x".repeat(quoted.len())),
            "a substitution in quotes"
        );
        assert_eq!(
            clean("A=\"x\ny\" cmd", Dialect::Bash),
            "cmd",
            "a value on two lines"
        );
    }

    #[test]
    fn lists_what_runs() {
        let cases: &[(&str, &[&str])] = &[
            ("git push", &["git push"]),
            (
                "cd /x && PORT=3 timeout 9 git push 2>&1 | tail -3",
                &["cd /x", "git push 2>&1", "tail -3"],
            ),
            ("(cd x && git push)", &["cd x", "git push"]),
            ("a; b || c & d", &["a", "b", "c", "d"]),
            ("a 0<&3 2>&1 &>x >|y", &["a 0<&3 2>&1 &>x >|y"]),
            ("git push \\\n  origin main", &["git push    origin main"]),
            (
                "out=$(git push 2>&1); echo \"$out\"",
                &["git push 2>&1", "out=$(git push 2>&1)", "echo \"$out\""],
            ),
            (
                "echo `git rev-parse HEAD`",
                &["git rev-parse HEAD", "echo `git rev-parse HEAD`"],
            ),
            (
                "echo \"`git rev-parse HEAD`\"",
                &["git rev-parse HEAD", "echo \"`git rev-parse HEAD`\""],
            ),
            (
                r#"bash -c "cd x && git push""#,
                &[r#"bash -c "cd x && git push""#, "cd x", "git push"],
            ),
            ("cmd //c 'git push'", &["cmd //c 'git push'", "git push"]),
            (
                "git commit -F - <<'EOF'\nfix: git push\nEOF\ngit status",
                &["git commit -F - <<'EOF'", "git status"],
            ),
            (
                "cat <<'EOF' | bash\ncd x\ngit push\nEOF\necho done",
                &["cat <<'EOF'", "bash", "cd x", "git push", "echo done"],
            ),
            ("bash <<'EOF'\ngit push\nEOF", &["bash <<'EOF'", "git push"]),
            ("bash x.sh <<'EOF'\ngit push\nEOF", &["bash x.sh <<'EOF'"]),
            (
                "git commit -F - <<'EOF' && bash x.sh\nfix: git push\nEOF",
                &["git commit -F - <<'EOF'", "bash x.sh"],
            ),
            (
                "gh pr create --body \"$(cat <<'EOF'\ncd x && git push\nEOF\n)\"",
                &["cat <<'EOF'", "gh pr create --body \"$(cat <<'EOF'\n)\""],
            ),
            ("if true; then git push; fi", &["if true", "git push"]),
            (
                "# check the repo's state\ngit status && git push",
                &["git status", "git push"],
            ),
            ("echo 'open && git push", &["echo 'open && git push"]),
            (
                r#"grep -c "a\|b" f && bash -lc "git push""#,
                &[r#"grep -c "a\|b" f"#, r#"bash -lc "git push""#, "git push"],
            ),
            (
                "comm -12 <(git diff --name-only | sort) <(git ls-files | sort)",
                &[
                    "git diff --name-only",
                    "sort",
                    "git ls-files",
                    "sort",
                    "comm -12 <(git diff --name-only | sort) <(git ls-files | sort)",
                ],
            ),
            (
                r#"gh pr create --body "$(echo "a; git push")""#,
                &[
                    r#"echo "a; git push""#,
                    r#"gh pr create --body "$(echo "a; git push")""#,
                ],
            ),
            (
                r#"git commit -m "日本語; git push" && git push"#,
                &[r#"git commit -m "日本語; git push""#, "git push"],
            ),
            ("echo $(git push", &["git push", "echo $(git push"]),
            (
                "out=$(cat <<'EOF' > f\nc\nEOF\ngit push; echo ok)",
                &[
                    "cat <<'EOF' > f",
                    "git push",
                    "echo ok",
                    "out=$(cat <<'EOF' > f\ngit push; echo ok)",
                ],
            ),
            (
                "cat <<A > f; bash <<B\nx\nA\ngit push\nB",
                &["cat <<A > f", "bash <<B", "git push"],
            ),
            (
                "bash <<A; cat <<B > f\ngit push\nA\nrm x\nB",
                &["bash <<A", "git push", "cat <<B > f"],
            ),
        ];
        for (command, want) in cases {
            assert_eq!(commands(command), *want, "{command}");
        }
    }

    #[test]
    fn reads_powershell() {
        let ps = |c: &str| commands_in(c, Dialect::PowerShell);
        assert_eq!(
            ps("gh pr create --title t `\n  --body b; Write-Host done"),
            vec!["gh pr create --title t    --body b", "Write-Host done"]
        );
        assert_eq!(
            ps("$b = @'\nit's \"a\" `x\n'@\ngh pr create --body $b"),
            vec!["$b = @'\nit's \"a\" `x\n'@", "gh pr create --body $b"]
        );
        assert_eq!(
            ps("echo \"a`\"b\"; git push"),
            vec!["echo \"a`\"b\"", "git push"]
        );
        assert_eq!(
            ps("cat <<EOF\ngit push"),
            vec!["cat <<EOF", "git push"],
            "no heredocs"
        );
    }

    #[test]
    fn does_not_cut_characters() {
        for command in [
            "x $(日",
            "a <(日",
            "a `日",
            "\\日 && \"\\日",
            "\"$(\"$(\"$(日",
            "cat <<日\n日\n",
            "echo '日' # 日'\n日",
            "@'日",
            "a <<\\",
        ] {
            let _ = commands(command);
            let _ = commands_in(command, Dialect::PowerShell);
            let _ = unquoted(command);
            let _ = split_heredocs(command);
        }
        let deep = "$(".repeat(2000) + &")".repeat(2000);
        let _ = commands(&deep);
        let open = "\"$(".repeat(2000);
        let _ = commands(&open);
    }

    /// Small random inputs never panic, and blanking keeps the length.
    #[test]
    fn survives_random_input() {
        let atoms = [
            "'", "\"", "\\", "$(", "$((", "(", ")", "<<", "<<-", "<<<", "EOF", "'EOF'", "\n", "#",
            "&", "|", "&&", ">", "2>&1", "`", "日本", "😀", " ", "bash -c ", "cat ", "git push",
            ";", "{", "}", "<(", "\r\n", "\\\n", "@'", "'@", "@\"",
        ];
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let len = (next() % 20) as usize;
            let text: String = (0..len)
                .map(|_| atoms[(next() % atoms.len() as u64) as usize])
                .collect();
            let _ = commands(&text);
            let _ = commands_in(&text, Dialect::PowerShell);
            let _ = split_heredocs(&text);
            assert_eq!(unquoted(&text).len(), text.len(), "{text:?}");
        }
    }

    /// The time grows in proportion to the length, also for nested and
    /// repeated shapes: release build only, `cargo test --release --
    /// --ignored`.
    #[test]
    #[ignore]
    fn takes_time_in_proportion() {
        // Shells nested in what they are handed, closed: each is read again
        // as the shell's, within a budget.
        let mut nested = String::from("git push");
        for _ in 0..40 {
            nested = format!("bash -c \"$({nested})\"");
        }
        let start = std::time::Instant::now();
        let _ = commands(&nested);
        let took = start.elapsed();
        assert!(took.as_secs_f64() < 1.0, "{took:?} for nested shells");
        let push = Regex::new(r"(^|[^[:alnum:]_-])git\s+push(\s|$)").unwrap();
        for shape in [
            "bash -c \"$(bash -c \"$(x)\")\" ",
            "((",
            "$((",
            "cat <<E\n$(x)\nE\n",
            "\"$((",
            "$(\"",
            "\"$('",
            "\"$(\"$(",
            "$(( \"",
            "bash -c \"$(",
            "$(a)\"$(",
            "\"$(",
            "a=b ",
            "$(a) ",
            "a <<E\n",
            "bash <<E\nx\nE\n",
            "cat <<'EOF'\n'\nEOF\n",
            "cat > f <<'E'\ngit push\nE\n",
            "cat > f <<'E'\ngit push\nE\ngh pr create -F f\n",
            "cat > f <<'E'\ngit push\nE\necho f\n",
            "cat <<E |\nx\nE\n",
            "done <<E\nx\nE\neval x\n",
            "gh pr create --body \"$(cat <<'E'\ngit push\nE\n)\"\n",
            "x -c 'y' ",
            "bash -c \"",
            "\"${\"",
            "# git push\n",
            "cd a\ngit add b\n",
            "(cd a)\ngit add b\n",
        ] {
            let time = |n: usize| {
                let text = shape.repeat(n / shape.len());
                let start = std::time::Instant::now();
                let _ = commands(&text);
                let _ = split_heredocs(&text);
                let _ = runs(&push, &text);
                let _ = crate::repo::parts_with_dirs(&text, "C:/here");
                start.elapsed()
            };
            let (small, large) = (time(10_000), time(100_000));
            assert!(large.as_secs_f64() < 1.0, "{shape:?}: {large:?} for 100KB");
            assert!(
                large.as_secs_f64() < small.as_secs_f64() * 30.0 + 0.02,
                "{shape:?}: {small:?} for 10KB, {large:?} for 100KB"
            );
        }
    }

    #[test]
    fn runs_reads_quotes_as_text() {
        let push = Regex::new(r"(^|[^[:alnum:]_-])git\s+push(\s|$)").unwrap();
        for command in [
            "git push",
            r#"bash -c "git push""#,
            "x=$(git push)",
            "cat <<'EOF' | bash\ngit push\nEOF",
            "git commit -m \"use <<EOF\"\ngit push",
            "echo 'a<<b'\ngit push",
            "# cat <<EOF\ngit push",
            "echo $((1<<2)); git push",
            "echo \"$((1<<EOF))\"\ngit push",
            "(( x<<n ))\ngit push",
            "cat > a <<'EOF'\r\nx\r\nEOF\r\ngit push",
            "cat > a <<'EOF'\nline \\\nEOF\ngit push",
            "x=$(cat <<EOF > f\nbody\nEOF\ngit push\n)",
            "diff <(cat <<EOF\na\nEOF\ngit push\n) f",
            "cat <<'EOF' \\\n  | bash\ngit push\nEOF",
            "bash \\\n  <<'EOF'\ngit push\nEOF",
            "cat <<A > f; bash <<B\nx\nA\ngit push\nB",
            "cat <<EOF \"a\nb\"\nbody\nEOF\ngit push",
            "bash -c \"bash <<EOF\ngit push\nEOF\"",
            "x=`cat <<EOF\nbody\nEOF\n`\ngit push",
            "cat <<\\EOF | bash\ngit push\nEOF",
            "cat <<EOF|bash\ngit push\nEOF",
            "echo $(( 1 << 2 )) <<EOF\nx\nEOF\ngit push",
            "f() { cat <<EOF\nx\nEOF\n}; git push",
            "if (( a << b )); then\n git push\nfi",
            "x=$((cd /c/x && git push) 2>&1)",
            "((cd x && git push) && y)",
            "cat <<'EOF' | bash 2>&1\ngit push\nEOF",
            "bash 2>&1 <<'EOF'\ngit push\nEOF",
            "cat <<'EOF' |& bash\ngit push\nEOF",
            "bash -s -- a <<'EOF'\ngit push\nEOF",
            "cat <<'EOF' | tee log | bash\ngit push\nEOF",
            "bash -c \"$(cat <<'EOF'\ngit push\nEOF\n)\"",
            "bash <(cat <<'EOF'\ngit push\nEOF\n)",
            "cat <<EOF\n$(git push)\nEOF",
            "cat > a <<EOF\nwhen: `git push`\nEOF",
            "bash -c \"x=$(cat <<'EOF'\nmsg\nEOF\n); git push\"",
            "bash -c \"$(cat <<'EOF'\nmsg\nEOF\n) && git push\"",
            "bash<<'EOF'\ngit push\nEOF",
            "cat a|bash<<EOF\ngit push\nEOF",
            "bash -euo pipefail <<EOF\ngit push\nEOF",
            "cat <<EOF | bash -o pipefail\ngit push\nEOF",
            "bash - <<EOF\ngit push\nEOF",
            "(cat <<EOF\ngit push\nEOF\n) | bash",
            "{ cat <<EOF\ngit push\nEOF\n} | bash",
            "sudo bash -c 'git push'",
            "env X=1 bash -c 'git push'",
            "echo $[1<<x]\ngit push",
            "/bin/bash -c \"git push\"",
            "\"C:/Program Files/Git/bin/bash.exe\" -c 'git push'",
            "cat <<'EOF' | /bin/bash\ngit push\nEOF",
            "xargs -I{} sh -c 'cd {} && git push'",
            "find . -execdir sh -c 'git push' \\;",
            "nice -n 5 bash -c 'git push'",
            "command bash -c 'git push'",
            "sudo -u bob bash -c 'git push'",
            "env -u X bash -c 'git push'",
            "{ cat <<'EOF'\ngit push\nEOF\n} 2>&1 | bash",
            "(cat <<'EOF'\ngit push\nEOF\n) 2>/dev/null | bash",
            "for x in a; do cat <<'EOF'\ngit push\nEOF\ndone | bash",
            "$(cat <<'EOF'\ngit push\nEOF\n)",
            "bash -c \"set -e; $(cat <<'EOF'\ngit push\nEOF\n)\"",
            "bash -c '$(cat <<EOF\ngit push\nEOF\n)'",
            "bash -c \"$(bash -c \"$(cat <<'EOF'\ngit push\nEOF\n)\")\"",
            "su -c 'git push' bob",
            "flock /tmp/l -c 'git push'",
            "script -q -c 'git push' /dev/null",
            "npx -c 'git push'",
            "source <(cat <<'EOF'\ngit push\nEOF\n)",
            ". <(cat <<'EOF'\ngit push\nEOF\n)",
            "source /dev/stdin <<'EOF'\ngit push\nEOF",
            "bash /dev/stdin <<'EOF'\ngit push\nEOF",
            "cat <<'EOF' > /tmp/s.sh\ngit push\nEOF\nbash /tmp/s.sh",
            "cat <<'EOF' | /usr/bin/env bash\ngit push\nEOF",
            "cat <<'EOF' | bash>log\ngit push\nEOF",
            "bash>log <<'EOF'\ngit push\nEOF",
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -File - << 'PSEOF'\ngit push\nPSEOF",
            "powershell -NoProfile -ExecutionPolicy Bypass -Command \"git push\"",
            "cmd.exe /d /c \"git push\"",
            "pwsh -WorkingDirectory X -Command 'git push'",
            "cat <<PR-BODY\nx\nPR-BODY\ngit push",
            "cat <<'END-OF-BODY' > f\nx\nEND-OF-BODY\ngit push",
            "cat <<END.TXT\nx\nEND.TXT\ngit push",
            "git commit -m \"$(cat <<'EOF'\nmsg\nEOF)\" && git push",
            "echo $'don\\'t' && git push",
            "nu -c 'git push'",
            "fish -c 'git push'",
            "bash -o pipefail -c 'git push'",
            "bash -O extglob -c 'git push'",
            "\"$SHELL\" -c 'git push'",
            "$BASH -c 'git push'",
            "tee s.sh <<'EOF'\ngit push\nEOF\nbash s.sh",
            "cat <<'EOF' | tee s.sh\ngit push\nEOF\nbash ./s.sh",
            "install /dev/stdin s.sh <<'EOF'\ngit push\nEOF\n. ./s.sh",
            "cat <<'EOF' > s.sh\ngit push\nEOF\nbash < s.sh",
            "cat <<'EOF' > s.sh\ngit push\nEOF\ncat s.sh | bash",
            "cat <<'EOF' > s.sh\ngit push\nEOF\nbash -e -o pipefail s.sh",
            "cat <<'EOF' > s.sh\ngit push\nEOF\nbash \"$PWD/s.sh\"",
            "cat <<'EOF' |\ngit push\nEOF\nbash",
            "cat <<'EOF' | (bash)\ngit push\nEOF",
            "cat <<'EOF' | \"$SHELL\"\ngit push\nEOF",
            "bash < <(cat <<'EOF'\ngit push\nEOF\n)",
            "bash /dev/stdin arg <<'EOF'\ngit push\nEOF",
            "${SHELL:-bash} -c 'git push'",
            "npm exec -c 'git push'",
            "git-bash.exe -c 'git push'",
            "env -S 'bash -e' -c 'git push'",
            "busybox ash -c 'git push'",
            "cat > s.sh <<'EOF'\ngit push\nEOF\nbash $(pwd)/s.sh",
            "cat 2>/dev/null > s.sh <<EOF\ngit push\nEOF\nbash s.sh",
            "/c/Program\\ Files/Git/bin/bash.exe <<'EOF'\ngit push\nEOF",
            "xargs -0 -I{} bash -c '{}' <<'EOF'\ngit push\nEOF",
            "cat <<'EOF' | xargs -0 bash -c\ngit push\nEOF",
            "cat <<'EOF' | xargs -I{} sh -c {}\ngit push\nEOF",
            "dd of=s.sh status=none <<'EOF'\ngit push\nEOF\nbash s.sh",
            "cat > s.ps1 <<'EOF'\ngit push\nEOF\npowershell -ExecutionPolicy Bypass -File s.ps1",
            "cat > s.cmd <<'EOF'\ngit push\nEOF\ncmd //c s.cmd",
            "tee \"$TMP/s.sh\" <<'EOF'\ngit push\nEOF\nbash ./\"$TMP/s.sh\"",
            "dd of=s.sh status=none <<'EOF'\ngit push\nEOF\nchmod +x s.sh && ./s.sh",
            "cat > s.sh <<'EOF'\ngit push\nEOF\n\"$SHELL\" s.sh",
            "busybox sh <<'EOF'\ngit push\nEOF",
            "sudo -i <<EOF\ngit push\nEOF",
            "script -q /dev/null <<'EOF'\ngit push\nEOF",
            "cat <<'EOF' | parallel\ngit push\nEOF",
            "while IFS= read -r l; do eval \"$l\"; done <<'EOF'\ngit push\nEOF",
            "bash 0< <(cat <<'EOF'\ngit push\nEOF\n)",
            "flock /tmp/l bash <<'EOF'\ngit push\nEOF",
            "unbuffer bash <<'EOF'\ngit push\nEOF",
            "stdbuf -oL sh -s <<'EOF'\ngit push\nEOF",
            "echo \"${x:-\"it's\"}\"; git push",
        ] {
            assert!(runs(&push, command), "{command}");
        }
        assert!(
            commands("echo \"${x:-\"it's\"}\"; git push").contains(&"git push".to_string()),
            "quotes in ${{…}} in double quotes"
        );
        let gh = Regex::new(r"(^|[^[:alnum:]_-])gh\s+pr\s+create(\s|$)").unwrap();
        for command in [
            "& 'C:\\Program Files\\Git\\bin\\bash.exe' -c 'gh pr create'",
            "<# a note #> gh pr create",
            "cd \"C:\\x\\\"; gh pr create --fill",
            "cd C:\\x <# c #>; gh pr create --fill",
        ] {
            assert!(runs_in(&gh, command, Dialect::PowerShell), "{command}");
        }
        // Past the budget for reading again, what a shell is handed is
        // kept whole, so that the push in it is still found.
        let budget = std::rc::Rc::new(std::cell::Cell::new(0));
        let text = "bash -c 'cd x && git push'";
        let kept: Vec<String> = Parser::new(text, Dialect::Bash, 0, 0, budget)
            .parse(&[])
            .into_iter()
            .map(|r| r.text)
            .collect();
        assert!(kept.contains(&"cd x && git push".to_string()), "{kept:?}");
        assert!(
            runs_in(
                &push,
                "bash -c \"echo `\"a`\"; git push\"",
                Dialect::PowerShell
            ),
            "quotes escaped for PowerShell come out for bash"
        );
        for command in [
            r#"git commit -m "then git push""#,
            "echo 'git push'",
            "git commit -F - <<'EOF'\ngit push\nEOF",
            "cat > a.md <<'EOF'\ngit push\nEOF",
            "msg=$(echo \"line1\ngit push is done\")",
            "cat <<'EOF'\n$(git push)\nEOF",
            "cat <<E\\OF\n$(git push)\nEOF",
            "# then git push\ngit status",
            r#"grep -c "git push" log"#,
            "rsync -avc \"git push\" dst/",
            "gh pr create --body \"$(cat <<'EOF'\nrun: git push\nEOF\n)\"",
            "git commit -m \"$(cat <<'EOF'\nit's git push\nEOF\n)\"",
            "cat <<'EOF' | gh pr create --body-file -\ngit push\nEOF",
            "cat > /tmp/b.md <<'EOF'\ngit push\nEOF\ngh pr create --body-file /tmp/b.md && rm /tmp/b.md",
            "cat > docs/a.md <<'EOF'\ngit push\nEOF\ngit add docs/a.md && git commit -m x",
            "cat <<'EOF' | tee a.md > /dev/null\ngit push\nEOF",
            // A double-quoted argument, read in either way as text.
            "bash x.sh \"$(cat <<'EOF'\ngit push\nEOF\n)\"",
            "bash -lc \"gh pr create --body \\\"$(cat <<'EOF'\ngit push\nEOF\n)\\\"\"",
            "X=\"$(cat <<'EOF'\ngit push\nEOF\n)\" bash -c 'echo'",
            "bash -c \"echo hi\" \"$(cat <<'EOF'\ngit push\nEOF\n)\"",
            "gh alias set p --shell \"$(cat <<'EOF'\ngit push\nEOF\n)\" && gh p",
        ] {
            assert!(!runs(&push, command), "{command}");
        }
        // Not shown to be data, so the plain match reads them: a script or
        // a shell may be handed the text.
        for command in [
            "bash x.sh <<'EOF'\ngit push\nEOF",
            "cat <<EOF | bash x.sh\ngit push\nEOF",
            "sh -c \"$(cat)\" <<'EOF'\ngit push\nEOF",
            "echo 'open && git push",
            "cat > s.sh <<'EOF'\ngit push\nEOF\nif bash s.sh; then :; fi",
            "F=s.sh; cat > \"$F\" <<'EOF'\ngit push\nEOF\nbash \"${F}\"",
            "cat > /c/a/s.sh <<'EOF'\ngit push\nEOF\ncd /c/a && ./s.sh",
            "cat > s.sh <<'EOF'\ngit push\nEOF\nfind . -name s.sh -exec bash {} \\;",
            "cat > s.sh <<'EOF'\ngit push\nEOF\nfor f in *.sh; do bash \"$f\"; done",
            "cat > .git/hooks/post-commit <<'EOF'\ngit push\nEOF\ngit commit -m x",
            "cat > Makefile <<'EOF'\nall:\n\tgit push\nEOF\nmake",
            "${SHELL:-bash} <<'EOF'\ngit push\nEOF",
            "{ bash; } <<'EOF'\ngit push\nEOF",
            "bash /dev/fd/0 <<'EOF'\ngit push\nEOF",
            "cmd <<'EOF'\necho it's\ngit push\nEOF",
            "out=\"$(case $b in main) echo \"it's\";; esac)\"; git push",
            "git -c 'alias.p=!git push' p",
            "exec > >(bash)\ncat <<'EOF'\ngit push\nEOF",
            "f() { cat <<'EOF'\ngit push\nEOF\n}; f | bash",
            "{ cat <<'EOF'\ngit push\nEOF\n} > s.sh\nbash s.sh",
            "for x in a; do cat <<'EOF'\ngit push\nEOF\ndone > s.sh\nbash s.sh",
            "if true; then cat <<'EOF'\ngit push\nEOF\nfi > s.sh\nbash s.sh",
            "cat > Makefile.toml <<'EOF'\ngit push\nEOF\ncargo make x",
            // A script written and not run is not shown to be data.
            "cat > a.sh <<'EOF'\ngit push\nEOF",
            "cat <<EOF >f.txt|| bash\ngit push\nEOF",
            // Not comments: a `#` inside a word, or in `${…}`, `[[ =~ ]]`.
            "echo $(date)#; git push",
            "echo a\\ #; git push",
            "echo a\\\n#; git push",
            "echo ${x:- #}; git push",
            "[[ a =~ ^(#|a)$ ]] && git push",
            "echo a\r#; git push",
            // The body is read after the line, not inside the `$(…)`.
            "cat <<'EOF' > notes.md; x=$(\ngit push\n)\nhello\nEOF",
            "cat <<$'EOF' > notes.md\nhi\nEOF\ngit push",
            "cat ${x/<<EOF/} > notes.md\ngit push",
            "cat <<'EOF' > notes.md; echo ${x:-\n}; git push\nhello\nEOF",
            "cat <<EOF\n$\\\n(git push origin main)\nEOF",
            "sort -o s.sh <<'EOF'\ngit push\nEOF\nbash s.sh",
            "cat <<'EOF' 1<>s.sh\ngit push\nEOF\nbash s.sh",
            "{ cat <<'EOF'\ngit push\nEOF\necho x; } | bash",
            "eval 'exec > >(bash)'\ncat <<'EOF'\ngit push\nEOF",
            "cat > notes.md <<'EOF'\ngit push\nEOF\nbash note[s].md",
            "cat > notes.md <<'EOF'\ngit push\nEOF\nf=notes; bash $f.md",
            "cat > notes.md <<'EOF'\ngit push\nEOF\nGIT_EXTERNAL_DIFF=bash git diff --no-index notes.md o.md",
            "cat > CMakeLists.txt <<'EOF'\nexecute_process(COMMAND git push origin main)\nEOF\ncmake .",
            "env -S 'bash -s' cat <<'EOF'\ngit push\nEOF",
            "python -c \"import subprocess; subprocess.run(\\\"git push origin main\\\", shell=True)\"",
            "git -c \"Alias.p=!git push\" p",
            "git -c \"core.editor=sh -c 'git push'\" commit --amend",
            // Not simple enough for any text to be shown to be data.
            "cat() { bash; }; cat <<'EOF'\ngit push\nEOF",
            "shopt -s expand_aliases\nalias cat=bash\ncat <<'EOF'\ngit push\nEOF",
            "hash -p /usr/bin/bash cat; cat <<'EOF'\ngit push\nEOF",
            "function f {\ncat <<'EOF'\ngit push\nEOF\n}\nf | bash",
            "cat > notes.md <<'EOF'\ngit push\nEOF\necho $(( $(bash notes.md) + 0 ))",
            "cat > notes.md <<'EOF'\ngit push\nEOF\necho a\\ #; bash notes.md",
            "[[ a =~ (\n#a) ]] || git push",
            "GIT_EDITOR=sh git commit -e -F - <<'EOF'\ngit push\nEOF",
            "git -c core.editor=sh commit -e -F - <<'EOF'\ngit push\nEOF",
            "git commit -F - <<'EOF' && git log -1 --format=%B | bash\ngit push\nEOF",
            "cat > notes.md <<'EOF'\ngit push\nEOF\ngit fetch --upload-pack='sh notes.md #' .",
            "cat <<'EOF' 2>s.sh >&2\ngit push\nEOF\nbash s.sh",
            "tee /dev/stderr <<'EOF' 2>s.sh >/dev/null\ngit push\nEOF\nbash s.sh",
            "tee -- -s.sh <<'EOF'\ngit push\nEOF\nbash -- -s.sh",
            "ex$()ec > >(bash)\ncat <<'EOF'\ngit push\nEOF",
            "git bisect run sh -c 'git push'",
            "cargo run -- bash -c 'git push'",
        ] {
            assert!(runs(&push, command), "{command}");
        }
    }

    #[test]
    fn cd_reaches_only_its_list() {
        let runs = runs_of("cd /a && (cd /b && x) && $(cd /c) && y", Dialect::Bash);
        let find = |t: &str| runs.iter().find(|r| r.text == t).unwrap();
        assert!(find("cd /a").reaches(find("y")));
        assert!(find("cd /a").reaches(find("x")));
        assert!(find("cd /b").reaches(find("x")));
        assert!(!find("cd /b").reaches(find("y")));
        assert!(!find("cd /c").reaches(find("y")));
    }

    /// The checks that ask what a command runs agree: text that only
    /// mentions a push is not one, wherever it is written, and a push run
    /// through a shell or a substitution is found, with the folder it runs
    /// in.
    #[test]
    fn checks_agree_on_what_runs() {
        use crate::builtin::post_push::{post_push_dir, runs_git_push};
        use crate::builtin::public_writing::gh_dir;
        use crate::builtin::push_check::{commits_before_push, held_back, push_dir};
        let here = "C:/here";
        for mention in [
            r#"git commit -m "then cd /c/x && git push""#,
            "git commit -F - <<'EOF'\nfix: cd /c/x && git push\nEOF",
            "cat > a.md <<'EOF'\ncd /c/x && git push\nEOF",
            r#"gh issue create --body "cd /c/x && git push""#,
            "# cd /c/x && git push later\ngit status",
            r#"grep -c "git push" log"#,
        ] {
            assert!(!runs_git_push(mention), "{mention}");
            assert!(!commits_before_push(mention), "{mention}");
        }
        for (command, dir, commits) in [
            (r#"bash -c "cd /c/x && git push""#, "/c/x", false),
            ("cat <<'EOF' | bash\ncd /c/x\ngit push\nEOF", "/c/x", false),
            ("out=$(cd /c/x && git push 2>&1)", "/c/x", false),
            ("(cd /c/x && git push)", "/c/x", false),
            ("(cd /c/x) && git push", here, false),
            ("x=$((cd /c/x && git push) 2>&1)", "/c/x", false),
            (
                "bash -c \"cd $(git rev-parse --show-toplevel) && git push\"",
                here,
                false,
            ),
            ("cmd //c \"cd /d C:/x && git push\"", "C:/x", false),
            ("cd /c/x; f() { cd /c/y; }; git push", "/c/x", false),
            ("ls | xargs -I{} sh -c 'cd {} && git push'", here, false),
            ("sudo cd /c/z; git push", here, false),
            ("time cd /c/z; git push", "/c/z", false),
            ("cd /c/z & git push", here, false),
            (
                "x=$(cat <<'EOF' | bash\ncd /c/y\nEOF\n); git push",
                here,
                false,
            ),
            ("cd /c/x && bash -c 'git push'", "/c/x", false),
            (r#"bash -lc "git commit -m x && git push""#, here, true),
            (
                "cd /c/x && git push && gh pr create --body \"$(cat <<'EOF'\ncd /c/y\nEOF\n)\"",
                "/c/x",
                false,
            ),
            ("git commit -m \"use <<EOF\"\ngit push", here, true),
            (
                "git commit -m \"$(cat <<'EOF'\nmsg\nEOF\n)\" && git push",
                here,
                true,
            ),
            // Found only by the plain match: the folder from the plainly
            // split parts.
            (
                "cat > s.sh <<'EOF'\ngit push\nEOF\n(cd /c/a && if bash s.sh; then :; fi)",
                "/c/a",
                false,
            ),
            ("(cd /c/a && git -c \"alias.p=!git push\" p)", "/c/a", false),
            ("git -C /c/b -c \"alias.p=!git push\" p", "/c/b", false),
            ("cd /c/a; echo $(date)#& git push\ncd /c/b", "/c/a", false),
        ] {
            assert!(runs_git_push(command), "{command}");
            assert_eq!(push_dir(command, here), dir, "{command}");
            assert_eq!(post_push_dir(command, here), dir, "{command}");
            assert_eq!(commits_before_push(command), commits, "{command}");
        }
        assert_eq!(held_back("git push 2>&1 | tail -3"), "");
        assert_ne!(held_back(r#"git add a && git push"#), "");
        assert_eq!(
            gh_dir(
                "cd /c/x && gh pr create --body \"$(cat <<'EOF'\ncd /c/y\nEOF\n)\"",
                here,
                Dialect::Bash
            ),
            "/c/x"
        );
        assert_eq!(
            gh_dir(
                "Set-Location C:/x; gh pr create --body b",
                here,
                Dialect::PowerShell
            ),
            "C:/x"
        );
    }
}
