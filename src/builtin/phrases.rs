//! Phrases that make Japanese read as machine-written or translated from
//! English, noted by wago-verb-reminder beside its verbs. A match is not an
//! error by itself, so each group of phrases comes with the cases where the
//! phrase may stay.
//!
//! The patterns, and the cases where a phrase may stay, come from yomiyasu
//! (https://github.com/nanaism/yomiyasu, skills/yomiyasu/scripts/
//! yomiyasu_lint.py at commit 84e1f5b; MIT License, Copyright (c) 2026
//! nanaism; see THIRD-PARTY-NOTICES.md). Their lookarounds are kept as
//! written, hence fancy-regex rather than regex. Of yomiyasu's metaphors,
//! only those translated from English are taken. Sentence forms such as
//! 〜に記載した or 〜してもよい are left out: whether they read well depends
//! on what the sentence says, which a pattern cannot tell.

use std::sync::LazyLock;

use fancy_regex::Regex;

/// Words that seldom say anything in plain prose (yomiyasu: SLOP_WORDS).
const EMPTY_WORDS: [&str; 32] = [
    // Words that pretend to touch or feel something.
    "手触り",
    "肌感",
    "肌感覚",
    "体温",
    "温度感",
    "熱量",
    "血の通った",
    "泥臭い",
    "泥臭さ",
    // Words that pretend to judge or understand something.
    "解像度",
    "腹落ち",
    "メンタルモデル",
    "本質的",
    "地に足のついた",
    "等身大",
    // Abstract nouns used as metaphors.
    "営み",
    "装置",
    "土台",
    "羅針盤",
    "起爆剤",
    "触媒",
    // Words that may make everyday things sound grand.
    "真理",
    "虚飾",
    "境地",
    "美学",
    "深淵",
    "冷徹",
    "禁欲的",
    "優美",
    "極致",
    "宿命",
    // Which role it names must be checked.
    "正本",
];

/// Words whose plain substring would catch other words, with the pattern
/// that leaves those out (yomiyasu: SLOP_WORD_PATTERNS).
const EMPTY_WORD_PATTERNS: [(&str, &str); 5] = [
    // Loanwords ending in -gate (ナビゲート, デリゲート) are left out by the
    // katakana before them; レビューゲート and the like are kept.
    (
        "ゲート",
        r"(?:(?<![ァ-ヴー])|(?<=レビュー)|(?<=リリース)|(?<=チェック)|(?<=デプロイ)|(?<=マージ)|(?<=テスト))(?<!搭乗)(?<!改札)ゲート(?!ウ[ェエ]イ|ボール|キーパー)",
    ),
    // Closures in mathematics (推移閉包, 閉包演算) are terms as defined.
    (
        "閉包",
        r"(?<!推移)(?<!反射)(?<!対称)(?<!凸)(?<!代数)(?<!代数的)閉包(?!演算)",
    ),
    // Ledgers in accounting and administration are terms as defined.
    (
        "台帳",
        r"(?<!資産)(?<!住民基本)(?<!会計)(?<!会計の)(?<!行政の)(?<!課税)(?<!土地)(?<!家屋)(?<!備品)(?<!登記)台帳",
    ),
    (
        "〜のOS",
        r"(思考|意思決定|仕事|人生|組織|チーム|学習|経営)の?OS(?![A-Za-z])",
    ),
    ("本質を突く", r"本質を突"),
];

/// Software terms put into dictionary Sino-Japanese words. Established
/// terms (日本語版, 回帰テスト) are left out by their form (yomiyasu:
/// TRANSLATED_TERM_PATTERNS).
const TRANSLATED: [(&str, &str); 3] = [
    (
        r"(この|その|あの|新しい|古い|最新の|前の|次の|今の|現在の|以前の)版(では|で|に|から|を|が|へ|の)",
        "「版」(version)",
    ),
    // 版 as part of a word (日本語版を更新, 第2版を更新) is left out.
    (
        r"(?<![一-龥ァ-ヴーA-Za-z0-9])版を(上げ|下げ|更新|戻)",
        "「版」(version)",
    ),
    (
        r"(?<!原点)(?<!への)(?<!先祖)(?<!平均)回帰(が|を)(出|発生|起こ|起き|生じ|防)",
        "「回帰」(regression)",
    ),
];

/// Metaphors translated from English (yomiyasu: METAPHOR_VERB_PATTERNS).
const METAPHORS: [(&str, &str); 3] = [
    (
        r"静かに(壊れ|落ち|失敗|沈黙)",
        "静かに壊れる (silently fail)",
    ),
    (r"黙って(無視|捨て|スキップ|破棄)", "黙って無視される"),
    (r"した瞬間に?", "〜した瞬間 (the moment ...)"),
];

const OVERSTATEMENT: &str = r"とも?(?:言|い)(?:い切|いき)?っても(?:過言|言い過ぎ|言いすぎ|いいすぎ)(?:では(?:ありません|ない)|じゃ(?:ありません|ない))(?:です)?(?:でしょう|だろう|かもしれ(?:ません|ない)|と(?:思|おも)(?:います|う)|ね|よ)?(?:[（(][^）)]*[）)])?[。！？!?]?$";

/// Set phrases, matched in each sentence (`^` and `$` are its ends), with
/// the case where each may stay, if any (yomiyasu: FILLER_PATTERNS,
/// FILLER_NOTES).
const FILLERS: [(&str, &str, &str); 15] = [
    (
        r"^(まず|ここで)?(重要|大切|大事)なのは、?",
        "重要なのは",
        "",
    ),
    (
        r"^結論から(言|い)(う|い|え)|^結論から申し上げ",
        "結論から言うと",
        "",
    ),
    (r"^正直に言うと、?", "正直に言うと", ""),
    (r"^本音を言うと、?", "本音を言うと", ""),
    (r"^避けたいのは、?", "避けたいのは", ""),
    (r"^ポイントは、", "ポイントは、", ""),
    (
        r"見落とされがち|面白いのはここ|まさにそこがポイント",
        "見落とされがち など",
        "",
    ),
    (
        r"いかがでした(でしょうか|か)?[？?。]?$",
        "いかがでしたでしょうか",
        "メールや手紙の結びの挨拶なら残してよい",
    ),
    (
        r"ぜひ(?:参考にして|試して|活用して)(?:みて)?ください[！!。]?",
        "ぜひ〜してみてください",
        "書き手が実際に勧めている内容なら残してよい",
    ),
    (
        r"参考に(なれ|な)ば幸い|お役に立て(れ)?ば幸い|ぜひご(活用|参考に)ください|まずは小さく(始め|はじめ)(?:ましょう|てみましょう|てください|てみてください|てみませんか|て(?:みて)?はいかが(?:ですか|でしょうか)|る(?:ことが(?:大切|大事|重要)|のがおすすめ)です)[。！？!?]?$",
        "参考になれば幸いです など",
        "メールや手紙の結びの挨拶や、書き手が実際に勧めている内容なら残してよい",
    ),
    (r"に(?:他|ほか)なりません", "〜に他なりません", ""),
    (
        r"^ご質問ありがとうございます",
        "ご質問ありがとうございます",
        "チャットやメールの返信そのものなら残してよい",
    ),
    (
        r"見ていきましょう[。！!]?$|深掘りしていきます",
        "見ていきましょう など",
        "",
    ),
    (
        r"^(必要なら|ご希望があれば|よろしければ)、?(次に|続けて|この後(?!の))[^。！？!?]*(?:ます|ましょう)(?:か|よ|ね|よね)?(?:（[^。！？!?（）]*）)?[。！？!?]?$",
        "必要なら次に〜します",
        "書き手が読み手に実際に申し出ている案内なら残してよい",
    ),
    (
        OVERSTATEMENT,
        "〜と言っても過言ではありません",
        "事実を淡々と書く。数値などの裏付けがあって誇張でない事実を強調する場合や、引用・会話・表現そのものへの言及なら残してよい",
    ),
];

fn compile<T: Copy>(table: &[(&str, T)]) -> Vec<(Regex, T)> {
    table
        .iter()
        .map(|&(pattern, label)| (Regex::new(pattern).unwrap(), label))
        .collect()
}

static EMPTY_RES: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
    EMPTY_WORD_PATTERNS
        .iter()
        .map(|&(word, pattern)| (Regex::new(pattern).unwrap(), word))
        .collect()
});
static TRANSLATED_RES: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| compile(&TRANSLATED));
static METAPHOR_RES: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| compile(&METAPHORS));
static FILLER_RES: LazyLock<Vec<(Regex, (&str, &str))>> = LazyLock::new(|| {
    FILLERS
        .iter()
        .map(|&(pattern, label, keep)| (Regex::new(pattern).unwrap(), (label, keep)))
        .collect()
});

/// Quoted text names a phrase rather than uses it.
static QUOTED: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"「[^「」]*」|『[^『』]*』").unwrap());
/// What quoted text is replaced with: not a space, so that a sentence
/// going on after a quote does not seem to start there.
const BLANK: &str = "＿";
static HEADING: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s{0,3}#{1,6}\s").unwrap());
/// Table rows and code indented as a block, which are not prose.
static NOT_PROSE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(\s*\||( {4}|\t)\s*([^-*+\d\s]|\d+[^.)\d]))").unwrap());
/// Markdown that comes before the words of a line.
static LINE_MARK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\s*(#{1,6}\s+|([-*+]|\d+[.)])\s+(\[[ xX]\]\s+)?)?").unwrap()
});

fn hit(re: &Regex, text: &str) -> bool {
    re.is_match(text).unwrap_or(false)
}

fn push(found: &mut Vec<&'static str>, label: &'static str) {
    if !found.contains(&label) {
        found.push(label);
    }
}

/// A line of prose to look at.
struct Line {
    text: String,
    /// A heading is not a sentence, so only words are looked for there.
    heading: bool,
}

/// The lines of prose to look at: quotes in Markdown (`>`), table rows and
/// indented code left out, quoted phrases blanked, and marks before the
/// words and for emphasis removed.
fn prose_lines(text: &str) -> Vec<Line> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('>') && !NOT_PROSE.is_match(line))
        .map(|line| {
            let heading = HEADING.is_match(line);
            let mut line = line.to_string();
            // Nested quotes are blanked from the inside out.
            while QUOTED.is_match(&line) {
                line = QUOTED
                    .replace_all(&line, |c: &regex::Captures| {
                        BLANK.repeat(c[0].chars().count())
                    })
                    .into_owned();
            }
            let text = LINE_MARK
                .replace(&line, "")
                .replace("**", "")
                .replace("__", "")
                .replace('*', "");
            Line { text, heading }
        })
        .filter(|line| !line.text.trim().is_empty())
        .collect()
}

/// The sentences of a line, each ending after 。！？!?.
fn sentences(line: &str) -> Vec<&str> {
    line.split_inclusive(['。', '！', '？', '!', '?'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// A line of note for each group of phrases found in `text`.
pub fn notes(text: &str) -> Vec<String> {
    let (mut empty, mut translated, mut metaphors) = (Vec::new(), Vec::new(), Vec::new());
    let mut fillers: Vec<(&str, &str)> = Vec::new();
    for Line {
        text: line,
        heading,
    } in prose_lines(text)
    {
        for word in EMPTY_WORDS {
            if line.contains(word) {
                push(&mut empty, word);
            }
        }
        for (re, word) in EMPTY_RES.iter() {
            if hit(re, &line) {
                push(&mut empty, word);
            }
        }
        for (re, label) in TRANSLATED_RES.iter() {
            if hit(re, &line) {
                push(&mut translated, label);
            }
        }
        for (re, label) in METAPHOR_RES.iter() {
            if hit(re, &line) {
                push(&mut metaphors, label);
            }
        }
        if heading {
            continue;
        }
        for sentence in sentences(&line) {
            for (re, filler) in FILLER_RES.iter() {
                if hit(re, sentence) && !fillers.contains(filler) {
                    fillers.push(*filler);
                }
            }
        }
    }
    let mut notes = Vec::new();
    if !translated.is_empty() {
        notes.push(format!(
            "英語の直訳の語: {}。ソフトウェアの文脈なら、定着した言い方(バージョン・リグレッション・デグレ)か具体的な説明にする。\
             出版物や製品の種類を表す「版」、回帰テスト・回帰分析など定着した用語なら残してよい。",
            translated.join("・")
        ));
    }
    if !metaphors.is_empty() {
        notes.push(format!(
            "英語の直訳の比喩: {}。ふだん使う動詞か、起きることをそのまま書く。文字どおりの動作や状態の変化なら残してよい。",
            metaphors.join("・")
        ));
    }
    if !empty.is_empty() {
        notes.push(format!(
            "中身の無いことの多い語: {}。比喩や飾りなら、ふだん使う言い方にする。文字どおりの意味や、文脈上必要な意味を担うなら残してよい。",
            empty.join("・")
        ));
    }
    if !fillers.is_empty() {
        let labels: Vec<&str> = fillers.iter().map(|(label, _)| *label).collect();
        let keeps: String = fillers
            .iter()
            .filter(|(_, keep)| !keep.is_empty())
            .map(|(label, keep)| format!("{label}: {keep}。"))
            .collect();
        notes.push(format!(
            "決まり文句: {}。前置きや飾りなら削り、本題から書く。評価や主張そのものを担うなら、述語に移すなどして意味を残す。{keeps}",
            labels.join("・")
        ));
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> String {
        notes(text).join("\n")
    }

    #[test]
    fn compiles_every_pattern() {
        assert_eq!(EMPTY_RES.len(), EMPTY_WORD_PATTERNS.len());
        assert_eq!(TRANSLATED_RES.len(), TRANSLATED.len());
        assert_eq!(METAPHOR_RES.len(), METAPHORS.len());
        assert_eq!(FILLER_RES.len(), FILLERS.len());
    }

    #[test]
    fn finds_translated_terms_but_not_established_ones() {
        assert!(found("この版では直した。").contains("「版」(version)"));
        assert!(found("版を上げる。").contains("「版」(version)"));
        assert!(found("回帰が出た。").contains("「回帰」(regression)"));
        for text in [
            "日本語版を更新した。",
            "第2版を更新した。",
            "回帰テストを足した。",
            "原点回帰を起こす。",
        ] {
            assert_eq!(found(text), "", "{text}");
        }
    }

    #[test]
    fn finds_metaphors_from_english() {
        assert!(found("設定が無いと静かに失敗する。").contains("静かに壊れる"));
        assert!(found("引数は黙って無視される。").contains("黙って無視される"));
        assert!(found("押した瞬間に開く。").contains("〜した瞬間"));
    }

    #[test]
    fn finds_empty_words_by_their_form() {
        let text = found("手触りのある解像度。レビューゲートを通す。");
        assert!(text.contains("手触り・解像度・ゲート"), "{text}");
        for text in [
            "ナビゲートする。",
            "搭乗ゲートに行く。",
            "推移閉包を取る。",
            "資産台帳に載せる。",
            "macOS で動く。",
        ] {
            assert_eq!(found(text), "", "{text}");
        }
        assert!(found("思考のOSを変える。").contains("〜のOS"));
    }

    #[test]
    fn finds_set_phrases_in_sentences() {
        let text = found("## 使い方\n\n重要なのは、設定です。いかがでしたでしょうか。");
        assert!(
            text.contains("重要なのは・いかがでしたでしょうか"),
            "{text}"
        );
        assert!(
            text.contains("いかがでしたでしょうか: メールや手紙"),
            "{text}"
        );
        assert!(found("- 結論から言うと、動く。").contains("結論から言うと"));
        assert!(found("速いと言っても過言ではありません。").contains("過言"));
        assert_eq!(found("最も重要なのは速さだ。"), "", "not at the start");
    }

    #[test]
    fn looks_at_prose_only() {
        let text = found("## 重要なのは\n\n## 手触り");
        assert!(text.contains("手触り"), "words in headings: {text}");
        assert!(
            !text.contains("決まり文句"),
            "no sentences in headings: {text}"
        );
        assert_eq!(found("| 列 | 解像度 |"), "", "table row");
        assert_eq!(found("    重要なのは、これ。"), "", "indented code");
        assert!(found("- a\n    - 重要なのは、これ。").contains("重要なのは"));
        assert!(found("見ていきましょう。").contains("見ていきましょう など"));
    }

    #[test]
    fn leaves_quotes_alone() {
        assert_eq!(
            found("「はい」重要なのは、これ。"),
            "",
            "not a sentence start"
        );
        assert_eq!(found("「重要なのは」と「手触り」を消した。"), "");
        assert_eq!(found("> この版では直した。"), "");
        assert_eq!(found("「『解像度』の話」をした。"), "");
    }
}
