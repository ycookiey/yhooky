# yhooky

Claude Code の hook を、1 つの実行ファイルでまとめて動かす (Rust)。

hook を shell の script で書くと、ツールを使うたびに script が何本も起動し、その中でも jq や grep が起動する。Windows の Git Bash ではプロセスの起動が重く、1 本で 0.5〜1.5 秒かかる。親が終わったあとも CPU を使い続けるプロセスが残ることもある。

yhooky は入力の JSON を 1 回だけ読み、その出来事に当てはまる規則を全部、1 つのプロセスの中で判定して、答えを 1 つ返す。shell を通さずに起動できる(exec form)。

- 単純な規則(「入力のこの項目がこの正規表現に当たれば、文を差し込む / 止める」)は TOML のファイルに書く。足すのも直すのも、build し直さなくてよい
- パターンでは書けない判定(引用の解析、ファイルの確認、transcript の読み取り)は、組み込みの判定(built-in)として持つ
- 失敗しても邪魔をしない。読めない入力、読めない規則、panic のどれでも、何も出力せず終了コード 0 で終わり、理由を `~/.claude/logs/yhooky.log` に書く

## Install

```sh
git clone https://github.com/ycookiey/yhooky
cd yhooky
cargo install --path .
```

## 登録

`settings.json` の hooks に、出来事ごとに 1 つ登録する。`args` を付けると shell を通さずに起動する。

```json
{
  "hooks": {
    "PreToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "yhooky", "args": ["hook"], "timeout": 10 }] }],
    "PostToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "yhooky", "args": ["hook"], "timeout": 10 }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "yhooky", "args": ["hook"], "timeout": 15 }] }]
  }
}
```

## 規則

規則は次のフォルダの `*.toml` から、この順(フォルダの中はファイル名の順)に読む。

1. `~/.claude/hooks/rules/`(`HOME`、なければ `USERPROFILE`)
2. `$CLAUDE_PROJECT_DIR/.claude/hooks/rules/`(project の規則。1 と同じフォルダなら読まない)

```toml
# 組み込みの判定のうち、動かすもの
builtins = ["backslash-shrink-guard", "language-guard"]

[[rule]]
id = "push-check-reminder"
event = "PreToolUse"                 # PreToolUse・PostToolUse・Stop など
tools = '^Bash$'                     # ツール名が当たるべき正規表現
fields = ["tool_input.command"]      # 読む項目(. 区切り)。最初の空でないもの
all = ['(^|\s)git\s+push(\s|$)']     # どれも、どこかの行に当たる
none = ['--dry-run']                 # どれも、どの行にも当たらない
action = "context"                   # context(文を差し込む)・deny(止める)・block(Stop で止める)
message = '''push の前に CI と同じ確認を回したか。'''
```

ほかの項目:

| 項目 | 意味 |
|---|---|
| `equals` | 項目の値が、この文字列と同じ(`jq -r` が出す形で比べる)。例: `{ "tool_input.run_in_background" = "true" }` |
| `transform` | 照合の前に変える。`slashes`(`\` を `/` に)、`squeeze-slashes`(続く `/` を 1 つに) |
| `repo_dir_in` | コマンドが作業するフォルダ(最後の `git -C <dir>`、なければ最後の `cd <dir>`、なければ hook の cwd)が、このどれか |
| `repo_dir_in_env` | 設定されていれば `repo_dir_in` の代わりにする環境変数(空白区切り) |
| `repo_remote` | そのフォルダの `origin` の URL が当たるべき正規表現 |
| `group` | 同じ group の規則は、最初に当たった 1 つだけを数える |

`message` の中の `{repo_dir}` はそのフォルダに、`{repo_name}` は `origin` の URL の最後の部分(`.git` を除く)に置き換わる。

正規表現は Rust の regex。shell の hook の `grep -E` と同じく、1 行ずつ照合する(`^` と `$` は各行の頭と終わり)。

複数の規則が当たったとき、止める規則が 1 つでもあれば止め、理由はつなげて返す。差し込む文もつなげる。

### 組み込みの判定

| 名前 | 出来事 | 内容 |
|---|---|---|
| `backslash-shrink-guard` | PreToolUse (Bash) | Bash ツールは `\\` を `\` に縮める。heredoc の本文と単一引用の中の `\\` は止める |
| `rm-junction-guard` | PreToolUse (Bash) | node_modules を含むフォルダへの `rm -r` を止める(junction の先まで消しうる) |
| `askuser-label-guard` | PreToolUse (AskUserQuestion) | 選択肢の label が A, B, C … の ID で始まらなければ止める |
| `sendfile-to-ysidepaney` | PreToolUse (SendUserFile) | 画像は ysidepaney に置き、SendUserFile を止める |
| `post-push-reminder` | PostToolUse (Bash) | 成功した push の後、repo の push 後の script を知らせる |
| `head-truncation-reminder` | PostToolUse (Bash) | 出力が最後の `\| head` の上限に届いたら、件数を数えるよう知らせる |
| `label-guard` | Stop | ギリシャ文字・ローマ数字の見出しを止める |
| `language-guard` | Stop | 最後の返答が英語主体なら止める |

## コマンド

```sh
yhooky hook                   # hook として動く: stdin に入力、stdout に答え
yhooky explain                # stdin の入力に、各規則が何を返すかを JSON の行で出す
yhooky check                  # 規則のファイルを読み、使えないものを報告する
yhooky test <cases.toml>...   # 規則を例の入力で確かめる(書式は src/cases.rs)
yhooky language-check <f>     # 過去の transcript で language-guard が止める回を出す
```

## License

MIT OR Apache-2.0
