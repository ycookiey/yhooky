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

組み込みの判定 `wago-verb-reminder` は辞書(UniDic から作る約 120 MB のフォルダ)を使う。実行ファイルの隣の `unidic` フォルダか、環境変数 `YHOOKY_UNIDIC_DIR` の指すフォルダに置く。無ければこの判定は何もしない。作り方:

```sh
# unidic-mecab-2.1.2 を展開したフォルダから作る。context_id_freq.txt は lindera-unidic の crate にある(省略可)
cargo run --release --example build_unidic -- <unidic-mecab-2.1.2> <出力先>/unidic [context_id_freq.txt]
```

辞書は UniDic(© The UniDic Consortium)を BSD ライセンスで使い、許諾文を辞書のフォルダの `UNIDIC-BSD` に置く。

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
action = "context"                   # context(文を差し込む)・deny(止める)・deny-once(session と照合した文ごとに一度だけ止め、出し直せば文を差し込むだけ)・block(Stop で止める)
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
| `backslash-shrink-guard` | PreToolUse (Bash) | Bash ツールは `\\` を `\` に縮める。heredoc の本文と単一引用の中の `\\` は止める。インラインのスクリプト(node -e・python -c・sed・awk 等)を渡すコマンドの `\\` は、session とコマンドごとに一度止める(出し直せば知らせるだけ) |
| `rm-junction-guard` | PreToolUse (Bash) | node_modules を含むフォルダへの `rm -r` を止める(junction の先まで消しうる) |
| `askuser-label-guard` | PreToolUse (AskUserQuestion) | 選択肢の label が A, B, C … の ID で始まらないとき、preview のある質問の選択肢に description があるとき(表示されない)に止める。長い説明は ysidepaney の note に出すよう返す |
| `sendfile-to-ysidepaney` | PreToolUse (SendUserFile) | 画像は ysidepaney に置き、動画(mp4・mov・webm・mkv・avi・m4v・wmv)はそのフォルダを Explorer で開いてファイルを選んだ状態にし、開く間はほかの窓が入力を取れないようにして、出たらすぐ最小化し(Windows のみ。開いた窓は一瞬見える)、SendUserFile を止める。置けない・開けないものと、それ以外のファイルは、改めて送るよう返す |
| `stage-own-edit-reminder` | PreToolUse (Edit・Write) | HEAD から変更のある追跡済みのファイルの初回の編集を止め、`git stage-own-edit begin` で写しを取ってから出し直すよう返す(session とファイルごとに 1 回。出し直した編集は通す。写しは今のファイルと一致するときだけ begin 済みとみなす) |
| `push-check-reminder` | PreToolUse (Bash) | `git push` を session・repo・HEAD の組ごとに初回だけ止め、push 前の確認の項目を返す。同じ push の 2 回目は通す |
| `private-word-guard` | PreToolUse (Bash) | `git push` の前に、どの remote にもまだ無い commit(`HEAD --not --remotes`)の追加行と commit メッセージから、個人を推測させうる語の候補を探す。当たれば、考え直すために一度止める(session・repo・当たった語と場所の組ごとに 1 回。同じ push を出し直せば通す)。禁止の一覧ではないので、候補は広めに入れてよい。語と除外する repo は `~/.claude/private-words.toml`(`PRIVATE_WORDS_FILE`)に `words = [...]`・`skip_repos = ["owner/repo"]` で書く。英数字だけの語は単語の境目で照合し、push 先の repo 自身の名前は照合しない。ファイルが無ければ何もしない |
| `heredoc-size-guard` | PreToolUse (Bash) | heredoc の本文が 100 行を超え、単一引用符を含むとき、session とコマンドごとに一度止める。Bash ツールがコマンドを包む処理で `unexpected EOF` になり、何も実行されないことがあるため。Write ツールでファイルに書いて実行するよう返す(出し直せば通す) |
| `post-push-reminder` | PostToolUse (Bash) | 成功した push の後、repo の push 後の script を知らせる |
| `head-truncation-reminder` | PostToolUse (Bash) | 出力が最後の `\| head` の上限に届いたら、件数を数えるよう知らせる |
| `label-guard` | Stop | ギリシャ文字・ローマ数字の見出しを止める |
| `language-guard` | Stop | 最後の返答が英語主体なら止める |
| `turn-state` | Stop・PostToolUse (Monitor) | ターンの終わりに、まだ動いている裏の処理を `<session>.stop.json` に書く(下の「turn-state」)。何も返さない |
| `wago-verb-reminder` | PreToolUse (ブラウザへの入力・メッセージの送信、Write・Edit) | 外に出る日本語の文の、熟語でない動詞(語種が和語の動詞)を並べて知らせる。補助的な語(為る・有る・居る・置く・因る)は除く。送りうる操作(メッセージの送信、ページでの computer)は送った後では直せないので、session と文ごとに一度止める(出し直せば通す)。ページで動かすスクリプト(javascript_tool)はコードなので見ない。Write・Edit は公開 repo の README と HTML だけ(公開かは `gh api` で調べ、1 日覚える。調べられなければ公開とみなす)。辞書は上の「Install」 |

### turn-state

ターンの終わりに、Claude が自分で続けるための裏の処理(完了を待つ shell・Monitor・subagent・予約した起動)が残っているかを、ファイルに書く。これを読むツール(plugin など)が、Claude が次にユーザーの入力を待っているのか、裏の処理の完了を待っているのかを見分ける。

- 置き場所: `$TURN_STATE_DIR`、なければ `~/.claude/state/turn-state`
- `<session>.stop.json`(Stop のたびに書き直す): `{ "at": <ms>, "tasks": [...], "dropped": [...], "crons": <件数> }`
  - `tasks`: Stop の入力の `background_tasks` のうち、自分で終わって Claude を起こしうるもの(`id`・`type`・`description`・`command`)
  - `dropped`: 止めるまで動き続けるコマンド(開発用のサーバー、watch など)と、`ignore_task` に当たる処理(公開したページへのコメントの見張りなど、人の入力でしか Claude を起こさないもの)。終わることで Claude を起こさないので外す
  - `crons`: `session_crons`(ScheduleWakeup・CronCreate・/loop)の件数
- `<session>.monitors`: Monitor の task の ID。Stop の入力では Monitor も `type: "shell"` で載るので、PostToolUse の `tool_response.taskId` で覚えておき、`dropped` には入れない

止めるまで動き続けるコマンドと、数えない処理は、`~/.claude/hooks/turn-state.toml` に書く(規則のフォルダの外。規則のファイルは規則しか受け付けないため)。

```toml
[[resident]]
pattern = '(pnpm|npm|yarn|bun)(\s+run)?\s+(dev|start|serve|preview)(\s|$)'  # コマンドの頭から当てる
except = ['\bbuild\b']                                                     # どれにも当たらないこと

[[ignore_task]]
type = 'monitor'                                  # background_tasks の type と同じ文字列
description = '^live updates for artifact '       # description のどこかに当たる正規表現
```

コマンドは、引用の外の `&&`・`||`・`;`・`|`・改行で区切った部分ごとに照合する。各部分の先頭の `VAR=value`・`timeout <n>`・`nohup`・`exec`・`time`・`(`・`{` は外し、準備だけの部分(`cd`・`sleep`・`rm`・`echo` など)は見ない。照合は引用を含む書いたままの文字列に対して行う。

## コマンド

```sh
yhooky hook                   # hook として動く: stdin に入力、stdout に答え
yhooky explain                # stdin の入力に、各規則が何を返すかを JSON の行で出す
yhooky check                  # 規則のファイルを読み、使えないものを報告する
yhooky test <cases.toml>...   # 規則を例の入力で確かめる(書式は src/cases.rs)
yhooky language-check <f>     # 過去の transcript で language-guard が止める回を出す
yhooky show-file <path>...    # 各ファイルのフォルダを Explorer で開いてファイルを選び、最小化する(sendfile-to-ysidepaney が裏で起動する)
```

## License

MIT OR Apache-2.0
