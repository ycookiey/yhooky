//! label-undefined-guard (Stop).
//!
//! When the last reply uses an option ID (one to three capital letters and
//! one or two digits, such as A1 or DH2) without saying in that same reply
//! what the option is, keeps Claude from stopping once and asks to say, for
//! each ID, what the option does, its effect and which is recommended. An
//! ID defined only in an earlier reply leaves the reader guessing.
//!
//! What is read: `last_assistant_message` only. Uses count outside code
//! blocks, inline code, quoted lines, URLs and Markdown links; definitions
//! count there too. An ID is a whole ASCII word of that shape, so `v1`,
//! `0.15.0`, hashes, `H264` and `P2P` are not IDs, nor is a word that is
//! part of a path, anchor, file name or version (`#A1`, `A1.md`, `A1-2`).
//! Words of that shape that are not option IDs (`UTF8`, `MP3`, `F5`, paper
//! sizes before a word such as 用紙, ...) are skipped, and so are the words
//! in `LABEL_UNDEFINED_GUARD_SKIP` (separated by commas or spaces).
//!
//! An ID counts as defined when, anywhere in the reply:
//! - it starts a list item, heading or table row (after list markers,
//!   `#`, `|` and emphasis), unless a particle such as で or を comes
//!   right after it in a list item;
//! - it stands alone on a line, or starts a line in bold, as a heading for
//!   the lines after it;
//! - it stands in parentheses right after words it names (保存(A1));
//! - it starts a line and a delimiter (`:` `(` `.` `)` `=` `-` `|`) comes
//!   right after it;
//! - it starts a table cell and text other than a particle follows it in
//!   the cell, or it fills the cell and the next cell is not empty;
//! - `(`, `「` or `:` right after it opens an explanation of two
//!   characters or more (other than a bare word such as 推奨);
//! - at least 15 characters follow it on its line, up to the end of the
//!   sentence or the next ID.
//!
//! IDs joined into a list or a range (`A1・A2`, `A1, A2`, `A1〜A3`) are
//! judged as one: when the list is defined by the above, every ID in it,
//! and in each range, is.
//!
//! A rewrite is never blocked again (`stop_hook_active`), so the guard
//! cannot loop.

use std::sync::LazyLock;

use regex::Regex;

use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::text::lines;

const ID: &str = "label-undefined-guard";
/// Characters after an ID that count as its explanation.
const EXPLANATION_CHARS: usize = 15;
/// IDs listed in the message at most.
const LISTED: usize = 10;

/// Words of the ID shape that name something else.
const SKIP: &[&str] = &[
    "UTF8", "UTF16", "UTF32", "MP2", "MP3", "MP4", "OS2", "PS1", "PS2", "PS3", "PS4", "PS5", "IE6",
    "IE7", "IE8", "IE9", "IE10", "IE11", "ES3", "ES5", "ES6", "CO2", "MD5", "SHA1", "SHA2", "SHA3",
    "IPV4", "IPV6", "X11", "V8", "S3", "EC2", "DDR3", "DDR4", "DDR5", "USB2", "USB3", "GPT3",
    "GPT4", "GPT5", "H1", "H2", "H3", "H4", "H5", "H6", "F1", "F2", "F3", "F4", "F5", "F6", "F7",
    "F8", "F9", "F10", "F11", "F12", "Q1", "Q2", "Q3", "Q4", "L1", "L2", "L3", "N1", "N2", "N3",
    "N4", "N5", "M1", "M2", "M3", "M4", "BO1", "BO3", "BO5", "BO7", "INT4", "INT8", "FP4", "FP8",
    "INT16", "INT32", "INT64", "FP16", "FP32", "FP64", "BF16",
];

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9_]+").unwrap());
static SHAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z]{1,3}[0-9]{1,2}$").unwrap());
/// Words that number a day, a volume, a level and so on (DAY1, LV5).
static SKIP_NUMBERED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(DAY|VOL|FIG|SEC|LV|EP|NO)[0-9]+$").unwrap());
/// A paper size when a word about paper follows: right after it (A4判,
/// A4 用紙) or a page count soon after (A4 で 2 ページ). A1 and B1 are left
/// as IDs, being the commonest.
static PAPER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(A3|A4|A5|B4|B5)$").unwrap());
static PAPER_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(判|[[:space:]]*(用紙|サイズ|の紙|(?i:paper|size))|[^。]{0,12}?[0-9〜~\-]+[[:space:]]*(ページ|枚|(?i:pages?)))").unwrap()
});
static LEFT_OUT: LazyLock<[Regex; 2]> = LazyLock::new(|| {
    [
        Regex::new(r"\[[^\]]*\]\([^)]*\)").unwrap(),
        Regex::new(r"https?://[^[:space:]]+").unwrap(),
    ]
});
/// What may come before the first word of a list item, heading or table row.
static LEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[[:space:]]*((?:[-*+]|[0-9]+[.)])[[:space:]]+|#+[[:space:]]*|\|[[:space:]]*)?(?:\[[ xX]\][[:space:]]*)?(\*\*|__|\*|_)?")
        .unwrap()
});
/// Emphasis and spaces between an ID and what follows it.
static CLOSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[[:space:]]*(?:\*\*|__|\*|_)?[[:space:]]*").unwrap());
static DELIMITER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[:：(（.)）=|\-—–]").unwrap());
/// A particle right after an ID: the item uses the ID rather than says it.
static PARTICLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(で|を|に|が|と|も|へ|や|か|なら|より)").unwrap());
/// A bare word in parentheses that says nothing about what the option is.
static BARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(推奨|おすすめ|お勧め|本命|採用|既定|仮|案|済|済み|未定|(?i:recommended|default))$",
    )
    .unwrap()
});
static SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[。！？!?]").unwrap());
/// What stands between the two ends of a range of IDs (A1〜A3).
static RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[[:space:]]*[〜～~\-–—][[:space:]]*$").unwrap());
/// What stands between IDs of a list or a range (A1・A2, A1〜A3).
static JOIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[[:space:]]*[・、,/〜～~\-–—][[:space:]]*$").unwrap());

/// The words of `line` that are option IDs, with their byte ranges.
fn ids<'a>(line: &'a str, skip: &[String]) -> Vec<(&'a str, usize, usize)> {
    let mut found = Vec::new();
    for m in WORD.find_iter(line) {
        let word = m.as_str();
        if !SHAPE.is_match(word)
            || SKIP.contains(&word)
            || SKIP_NUMBERED.is_match(word)
            || skip.iter().any(|s| s == word)
        {
            continue;
        }
        let before = line[..m.start()].chars().next_back();
        let mut after = line[m.end()..].chars();
        let (next, then) = (after.next(), after.next());
        // Part of a path, an anchor, a name, a file name or a version.
        if before.is_some_and(|c| matches!(c, '.' | '\\' | '#' | '@' | '$' | '%')) {
            continue;
        }
        // After a hyphen, part of a name (intro-A1), unless it ends a range
        // (A1-A3).
        if let Some(head) = line[..m.start()].strip_suffix('-') {
            let word_before = head
                .rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
                .unwrap_or("");
            if !word_before.is_empty() && !SHAPE.is_match(word_before) {
                continue;
            }
        }
        if next == Some('.') && then.is_some_and(|c| c.is_ascii_alphanumeric())
            || next == Some('-') && then.is_some_and(|c| c.is_ascii_digit())
        {
            continue;
        }
        // A cell of a grid, such as a map's (A4②).
        if next.is_some_and(|c| ('①'..='⑳').contains(&c)) {
            continue;
        }
        if PAPER.is_match(word) && PAPER_WORD.is_match(&line[m.end()..]) {
            continue;
        }
        found.push((word, m.start(), m.end()));
    }
    found
}

/// Whether the ID at `start..end` of `line` says what it is. `until` is
/// where the next ID starts.
fn defines(line: &str, start: usize, end: usize, until: usize) -> bool {
    let rest = &line[end..];
    let after = &rest[CLOSE.find(rest).map_or(0, |m| m.end())..];
    if let Some(lead) = LEAD.captures(line)
        && lead.get(0).is_some_and(|m| m.end() == start)
    {
        let marker = lead.get(1).map_or("", |m| m.as_str()).trim();
        let bold = lead.get(2).is_some_and(|m| m.len() == 2);
        // A heading, a table row, or a line that starts in bold or is the
        // ID alone, as a heading for the lines after it.
        if marker.starts_with('#')
            || marker.starts_with('|')
            || marker.is_empty() && bold
            || after.trim().is_empty()
        {
            return true;
        }
        if !marker.is_empty() && !PARTICLE.is_match(after) {
            return true;
        }
        if DELIMITER.is_match(after) {
            return true;
        }
    }
    if starts_table_cell(line, start, end) {
        return true;
    }
    // A name for the words before it: 保存(A1).
    let mut back = line[..start].chars().rev();
    if back.next().is_some_and(|c| matches!(c, '(' | '（'))
        && back.next().is_some_and(|c| !c.is_whitespace())
        && line[end..].starts_with([')', '）'])
    {
        return true;
    }
    if let Some(open) = after.chars().next()
        && matches!(open, '(' | '（' | '「' | '『' | ':' | '：')
    {
        let body = &after[open.len_utf8()..];
        let body = match open {
            '(' | '（' => body.split([')', '）']).next().unwrap_or(""),
            '「' | '『' => body.split(['」', '』']).next().unwrap_or(""),
            _ => SENTENCE_END.split(body).next().unwrap_or(""),
        };
        let body = body.trim();
        if body.chars().count() >= 2 && !BARE.is_match(body) {
            return true;
        }
    }
    let tail = &line[end..until.max(end)];
    let tail = SENTENCE_END.split(tail).next().unwrap_or("");
    tail.chars()
        .filter(|c| !c.is_whitespace() && *c != '*' && *c != '_')
        .count()
        >= EXPLANATION_CHARS
}

/// Whether the ID starts a cell of a table row and says what it is there:
/// text follows it in the cell (not a particle), or it fills the cell and
/// the next cell has text.
fn starts_table_cell(line: &str, start: usize, end: usize) -> bool {
    if !line.trim_start().starts_with('|') {
        return false;
    }
    let cell_start = line[..start].rfind('|');
    let cell_end = line[end..].find('|').map(|i| end + i);
    let (Some(a), Some(b)) = (cell_start, cell_end) else {
        return false;
    };
    let emphasis = |s: &str| s.trim().trim_matches(['*', '_']).trim().to_string();
    if !emphasis(&line[a + 1..start]).is_empty() {
        return false;
    }
    let inside = emphasis(&line[end..b]);
    if !inside.is_empty() {
        return !PARTICLE.is_match(&inside);
    }
    let next = line[b + 1..].split('|').next().unwrap_or("");
    next.trim().chars().any(|c| c.is_alphanumeric()) && line[b + 1..].contains('|')
}

/// The IDs from `first` to `last` (A1, A2, A3 for A1 and A3), when both
/// have the same letters and the numbers go up by at most 20.
fn range(first: &str, last: &str) -> Option<Vec<String>> {
    let split = |id: &str| {
        let at = id.find(|c: char| c.is_ascii_digit())?;
        Some((id[..at].to_string(), id[at..].parse::<u32>().ok()?))
    };
    let ((letters, from), (other, to)) = (split(first)?, split(last)?);
    (letters == other && from < to && to - from <= 20)
        .then(|| (from..=to).map(|n| format!("{letters}{n}")).collect())
}

/// The IDs `text` uses without saying what they are, in the order first
/// used.
pub fn undefined(text: &str, skip: &[String]) -> Vec<String> {
    // A definition counts wherever the reader sees it, in code and quotes
    // too (`- `A1`: ...`, a diff's comment); a use counts only in prose.
    let every: Vec<String> = lines(text).map(|l| l.replace('`', "")).collect();
    let (defined, _) = scan(every, skip);
    let (_, mut used) = scan(super::label::prose(text), skip);
    used.retain(|u| !defined.contains(u));
    used
}

/// The IDs `lines` say, and the IDs they use without saying, in the order
/// first seen.
fn scan(lines: Vec<String>, skip: &[String]) -> (Vec<String>, Vec<String>) {
    let mut defined: Vec<String> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for line in lines {
        let mut line = line;
        for re in LEFT_OUT.iter() {
            line = re.replace_all(&line, "").into_owned();
        }
        let found = ids(&line, skip);
        let mut i = 0;
        while i < found.len() {
            // IDs joined into a list or a range (A1・A2, A1〜A3) are said
            // or used as one.
            let mut j = i;
            while j + 1 < found.len() && JOIN.is_match(&line[found[j].2..found[j + 1].1]) {
                j += 1;
            }
            let until = found.get(j + 1).map_or(line.len(), |n| n.1);
            let says = defines(&line, found[i].1, found[j].2, until);
            let mut group: Vec<String> = Vec::new();
            for k in i..=j {
                group.push(found[k].0.to_string());
                if says
                    && k < j
                    && RANGE.is_match(&line[found[k].2..found[k + 1].1])
                    && let Some(between) = range(found[k].0, found[k + 1].0)
                {
                    group.extend(between);
                }
            }
            for id in group {
                let list = if says { &mut defined } else { &mut used };
                if !list.contains(&id) {
                    list.push(id);
                }
            }
            i = j + 1;
        }
    }
    (defined, used)
}

fn skip_words() -> Vec<String> {
    std::env::var("LABEL_UNDEFINED_GUARD_SKIP")
        .unwrap_or_default()
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "Stop" || input.text("stop_hook_active") == "true" {
        return Vec::new();
    }
    let text = input.text("last_assistant_message");
    if text.is_empty() {
        return Vec::new();
    }
    let missing = undefined(&text, &skip_words());
    if missing.is_empty() {
        return Vec::new();
    }
    let mut listed = missing
        .iter()
        .take(LISTED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if missing.len() > LISTED {
        listed.push_str(&format!(" ほか {} 個", missing.len() - LISTED));
    }
    super::keep_input(input, "label-undefined-guard-last-block.json");
    vec![Finding::new(
        ID,
        Kind::Block,
        format!(
            "直前の返答は、次の記号を何の案か説明せずに使っている: {listed}。前の返答で決めた記号だけでは、\
             読み手は何の案か分からない。記号ごとに、何をする案か・影響・推奨を同じ返答の中で書き直し、\
             書き直した返答だけを出す(記号をやめて中身で書いてもよい)。"
        ),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing(text: &str) -> Vec<String> {
        undefined(text, &[])
    }

    #[test]
    fn finds_ids_by_ascii_word() {
        assert_eq!(missing("案はDH1で進める。"), ["DH1"], "between Japanese");
        assert_eq!(missing("DH1案かCW2案。"), ["DH1", "CW2"]);
        assert_eq!(missing("A1/A2 のどちらか。"), ["A1", "A2"]);
        for word in [
            "v1", "0.15.0", "978906f", "3FA9C1E", "H264", "P2P", "E2E", "ABCD1", "A123", "a1",
        ] {
            assert!(missing(&format!("{word} を使う。")).is_empty(), "{word}");
        }
    }

    #[test]
    fn skips_words_of_other_meaning() {
        for word in [
            "UTF8", "MP3", "OS2", "F5", "H1", "Q3", "IPV6", "BO3", "DAY1", "LV50",
        ] {
            assert!(missing(&format!("{word} で進める。")).is_empty(), "{word}");
        }
        assert!(missing("A4 用紙で出す。").is_empty());
        assert!(missing("B5サイズ。").is_empty());
        assert!(missing("A4 で 2 ページの PDF。").is_empty());
        assert!(missing("A4判で出す。").is_empty());
        assert!(missing("(A4、1〜2 ページ)").is_empty());
        assert!(missing("マップの A4② の建物。").is_empty(), "a map's grid");
        assert!(missing("intro-A1 の版。").is_empty(), "part of a name");
        assert_eq!(missing("A4 で進める。"), ["A4"], "an option, not paper");
        for text in [
            "A1 を横の画面に出した。",
            "A4 と判断した。",
            "A2 のとおりページを直す。",
            "A4 のページを直す。",
            "B1 で 2 ページ目を直す。",
        ] {
            assert_eq!(missing(text).len(), 1, "{text}");
        }
        assert!(undefined("R2 で進める。", &["R2".to_string()]).is_empty());
    }

    #[test]
    fn skips_paths_anchors_and_versions() {
        for text in [
            "docs/x.md#A1 を見る。",
            "A1.md を開く。",
            "A1-2 の手順。",
            "B2.0 に上げる。",
            "@A1 に送る。",
        ] {
            assert!(missing(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn takes_list_items_headings_and_tables_as_definitions() {
        for text in [
            "- A1: 並列化\n- A2: 先行起動\n\nA1 で進める。",
            "- **A1**: 並列化\n推奨は A1。",
            "1. A1 並列化\nA1 で。",
            "### A1 並列化\nA1 で。",
            "| ID | 案 |\n|---|---|\n| A1 | 並列化 |\n\nA1 で。",
            "| **U1** | 書き直す | 速い |\nU1 で。",
            "| 1 | A1 | 並列化 |\nA1 で。",
            "| | A1. 試験用で試す | A2. 本番で試す |\nA2 で。",
            "**A1**\n並列化する。\n\nA1 で。",
            "A1. 並列化\nA1 で。",
            "A1 = 並列化\nA1 で。",
        ] {
            assert!(missing(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn takes_explanations_after_an_id_as_definitions() {
        assert!(missing("推奨は A1(並列化で速い)。").is_empty());
        assert!(missing("推奨は A1（並列化）。").is_empty());
        assert!(missing("推奨は A1: 並列化。").is_empty());
        assert!(missing("推奨は A1 で、全部の処理を並列にして待ち時間を減らす。").is_empty());
        assert!(
            missing("A1 で進める。\nA1: 並列化").is_empty(),
            "defined later"
        );
        assert!(
            missing("推奨は E1「画像だけ」。").is_empty(),
            "a name in brackets"
        );
        assert!(missing("- **A1** (並列化)\nA1 で。").is_empty());
        assert!(
            missing("この形は保存(C2)の設計で決める。").is_empty(),
            "names the words before"
        );
        assert!(missing("扱い(E1・E2・E3)を決める。").is_empty());
        assert!(
            missing("**V3 追加**:\n- 色を変えた").is_empty(),
            "a bold heading"
        );
    }

    #[test]
    fn takes_definitions_in_code_and_quotes() {
        assert!(missing("- `A1`: 並列化\n\nA1 で進める。").is_empty());
        assert!(missing("```diff\n# BB1 p3: 重なり\n- x\n+ y\n```\nBB1 を直す。").is_empty());
        assert!(missing("> A1: 並列化\n\nA1 で進める。").is_empty());
        assert_eq!(
            missing("```\nA1 で進める\n```\nA1 で。"),
            ["A1"],
            "a use in code"
        );
    }

    #[test]
    fn takes_a_range_said_as_a_whole() {
        assert!(missing("- E1〜E3: 打ち合わせの資料\n\nE2 で進める。").is_empty());
        assert!(missing("案 A1-A3: 並列の度合いを変える\nA2 で。").is_empty());
        assert_eq!(missing("A1〜A3 のどれか。"), ["A1", "A3"]);
        assert_eq!(missing("- A1〜A3: x\nB2 で。"), ["B2"], "other letters");
        assert!(
            missing("| A1・B1・C1 | 規則の文 |\nB1 で。").is_empty(),
            "a list"
        );
        assert_eq!(missing("A1・B2 で進める。"), ["A1", "B2"]);
    }

    #[test]
    fn finds_ids_used_bare() {
        assert_eq!(missing("A1 で進める。"), ["A1"]);
        assert_eq!(missing("推奨は A1(推奨)。"), ["A1"], "bare word");
        assert_eq!(
            missing("推奨は (A1)。"),
            ["A1"],
            "no words before the parentheses"
        );
        assert_eq!(missing("- A1 で進める"), ["A1"], "particle in a list item");
        assert_eq!(missing("推奨は DH1。CW2 は影響が大きい。"), ["DH1", "CW2"]);
        assert_eq!(
            missing("推奨は A1。A2 で全部の処理を並列にして待ち時間を減らす。"),
            ["A1"],
            "the next sentence explains A2, not A1"
        );
        assert_eq!(missing("| 推奨 | A1 |\n|---|---|"), ["A1"], "no next cell");
        assert_eq!(
            missing("| 推奨 | A1 で進める |"),
            ["A1"],
            "particle in a cell"
        );
    }

    #[test]
    fn leaves_out_code_quotes_and_links() {
        for text in [
            "```\nA1 で進める\n```",
            "1. 手順\n   ```\n   A1 で\n   ```",
            "`A1` で進める。",
            "> A1 で進めて",
            "https://example.com/A1 を見る。",
            "[A1 の説明](https://example.com) を見る。",
        ] {
            assert!(missing(text).is_empty(), "{text}");
        }
    }
}
