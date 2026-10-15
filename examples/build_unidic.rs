//! Builds the dictionary the wago-verb-reminder check reads, from UniDic
//! (unidic-mecab 2.1.2, used under its BSD license):
//!
//! ```sh
//! cargo run --release --example build_unidic -- <unidic-mecab-2.1.2 dir> <out dir> [context_id_freq.txt]
//! ```
//!
//! Only the fields the check reads are kept, which takes the dictionary from
//! 191 MB to about 120 MB with the same segmentation: surface, the context
//! ids and cost, part of speech, its first subcategory, lexeme (語彙素),
//! base spelling (書字形基本形) and word type (語種). `context_id_freq.txt`
//! (lindera-unidic ships one, MIT) only orders the connection ids for
//! speed. The license files are copied next to the dictionary.

use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use lindera_dictionary::builder::DictionaryBuilder;
use lindera_dictionary::dictionary::metadata::Metadata;

/// The lex.csv columns kept, in order.
const KEEP: [usize; 9] = [0, 1, 2, 3, 4, 5, 11, 14, 16];

const METADATA: &str = r#"{
  "name": "unidic-wago",
  "encoding": "UTF-8",
  "default_word_cost": -10000,
  "default_left_context_id": 0,
  "default_right_context_id": 0,
  "default_field_value": "*",
  "flexible_csv": false,
  "skip_invalid_cost_or_id": false,
  "normalize_details": false,
  "dictionary_schema": {
    "fields": [
      "surface", "left_context_id", "right_context_id", "cost",
      "part_of_speech", "part_of_speech_subcategory_1",
      "lexeme", "orthographic_base_form", "word_type"
    ]
  },
  "user_dictionary_schema": { "fields": ["surface", "part_of_speech", "reading"] },
  "connection_id_mapping": true
}"#;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(src), Some(out)) = (args.first(), args.get(1)) else {
        return Err(
            "usage: build_unidic <unidic-mecab-2.1.2 dir> <out dir> [context_id_freq.txt]".into(),
        );
    };
    let (src, out) = (Path::new(src), Path::new(out));
    let input = out.with_extension("src");
    fs::create_dir_all(&input)?;

    let lex = BufReader::new(fs::File::open(src.join("lex.csv"))?);
    let mut trimmed = BufWriter::new(fs::File::create(input.join("lex.csv"))?);
    for line in lex.lines() {
        let line = line?;
        // No field of unidic-mecab 2.1.2's lex.csv is quoted.
        let fields: Vec<&str> = line.split(',').collect();
        let kept: Vec<&str> = KEEP.iter().map(|&i| fields[i]).collect();
        writeln!(trimmed, "{}", kept.join(","))?;
    }
    trimmed.flush()?;
    for name in ["char.def", "unk.def", "matrix.def"] {
        fs::copy(src.join(name), input.join(name))?;
    }

    let metadata: Metadata = serde_json::from_str(METADATA)?;
    let mut builder = DictionaryBuilder::new(metadata);
    if let Some(freq) = args.get(2) {
        builder = builder.with_context_id_freq(freq);
    }
    fs::create_dir_all(out)?;
    builder.build_dictionary(&input, out)?;
    for name in ["COPYING", "BSD"] {
        fs::copy(src.join(name), out.join(format!("UNIDIC-{name}")))?;
    }
    fs::remove_dir_all(&input)?;
    Ok(())
}
