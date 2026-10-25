# yhooky

Runs Claude Code hooks from a single executable (Rust).

Hooks written as shell scripts start several scripts on every tool call, and each script starts jq or grep in turn. Starting a process is slow on Windows Git Bash, 0.5 to 1.5 seconds each, and some processes can keep using CPU after their parent has exited.

yhooky reads the input JSON once, checks every rule that applies to the event inside one process, and returns a single answer. It can be started without a shell (exec form).

- Simple rules ("if this input field matches this regular expression, add a note / stop") are written in TOML files. Adding or changing one needs no rebuild.
- Checks a pattern cannot express (parsing quotes, looking at files, reading the transcript) are built in.
- It never gets in the way when it fails. On unreadable input, an unreadable rule or a panic, it prints nothing, exits with status 0, and writes the reason to `~/.claude/logs/yhooky.log`.

## Install

```sh
git clone https://github.com/ycookiey/yhooky
cd yhooky
cargo install --path .
```

The checks that need to know whether a repository is public (`public-repo-writing-reminder`, `push-check-reminder`, `wago-verb-reminder`) use git and an authenticated GitHub CLI (`gh`).

The built-in check `wago-verb-reminder` uses a dictionary (a folder of about 120 MB built from UniDic). Put it in a `unidic` folder next to the executable, or in the folder named by the environment variable `YHOOKY_UNIDIC_DIR`. Without it, this check lists no verbs, but still notes the phrases. To build it:

```sh
# From an unpacked unidic-mecab-2.1.2. context_id_freq.txt is in the lindera-unidic crate (optional).
cargo run --release --example build_unidic -- <unidic-mecab-2.1.2> <output>/unidic [context_id_freq.txt]
```

The dictionary uses UniDic (© The UniDic Consortium) under the BSD license; the license text is placed in the dictionary folder as `UNIDIC-BSD`.

## Setup

Register yhooky once per event under `hooks` in `settings.json`. With `args`, it starts without a shell.

```json
{
  "hooks": {
    "PreToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "yhooky", "args": ["hook"], "timeout": 10 }] }],
    "PostToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "yhooky", "args": ["hook"], "timeout": 10 }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "yhooky", "args": ["hook"], "timeout": 15 }] }]
  }
}
```

### Pointers to your documents

Some built-in checks name a document for details in their messages (`bash.md`, `git.md`, `public-repo.md`, `external-japanese.md`). These documents do not ship with yhooky: they are the user's own notes, and a message may point into them down to a section title or a rule number. The author's notes are at <https://github.com/ycookiey/dotfiles/tree/main/claude/docs>.

Set `YHOOKY_DOCS_DIR` to the folder that holds them, and the messages name `<dir>/<file>`; without it, the pointer is left out. A hook inherits Claude Code's environment, so the `env` block of `settings.json` can set it:

```json
{
  "env": { "YHOOKY_DOCS_DIR": "~/.claude/docs" }
}
```

The value is only written into messages, not opened, so `~` can stay as it is.

`stage-own-edit-reminder` and `git-add-own-check` ask to run `git stage-own-edit`, which does not ship with yhooky either: it is a separate script, at <https://github.com/ycookiey/dotfiles/blob/main/bin/git-stage-own-edit>.

Messages from the built-in checks are in Japanese.

## Rules

Rules are read from the `*.toml` files in these folders, in this order (files within a folder by name):

1. `~/.claude/hooks/rules/` (`HOME`, else `USERPROFILE`)
2. `$CLAUDE_PROJECT_DIR/.claude/hooks/rules/` (project rules; skipped when it is the same folder as 1)

```toml
# Built-in checks to run
builtins = ["backslash-shrink-guard", "language-guard"]

[[rule]]
id = "push-check-reminder"
event = "PreToolUse"                 # PreToolUse, PostToolUse, Stop, ...
tools = '^Bash$'                     # regular expression the tool name must match
fields = ["tool_input.command"]      # fields to read (dot-separated); the first that is not empty
all = ['(^|\s)git\s+push(\s|$)']     # each must match some line
none = ['--dry-run']                 # none may match any line
action = "context"                   # context (add a note), deny (stop), deny-once (stop once per session and matched text; sent again, it only adds the note), block (stop at Stop)
message = '''Did you run the same checks as CI before pushing?'''
```

Other fields:

| Field | Meaning |
|---|---|
| `equals` | The field's value equals this string (compared in the form `jq -r` prints). Example: `{ "tool_input.run_in_background" = "true" }` |
| `transform` | Changes the text before matching: `slashes` (`\` to `/`), `squeeze-slashes` (runs of `/` to one) |
| `repo_dir_in` | The folder the command works in (the last `git -C <dir>`, else the last `cd <dir>`, else the hook's cwd) is one of these |
| `repo_dir_in_env` | Environment variable used instead of `repo_dir_in` when set (space-separated) |
| `repo_remote` | Regular expression the `origin` URL of that folder must match |
| `group` | Of the rules in one group, only the first that matches counts |

In `message`, `{repo_dir}` is replaced by that folder, and `{repo_name}` by the last part of the `origin` URL (without `.git`).

Regular expressions are Rust regex. As with `grep -E` in shell hooks, they are matched one line at a time (`^` and `$` are the start and end of each line).

When several rules match, the call is stopped if any of them stops it, with the reasons joined; notes to add are joined as well.

### Built-in checks

| Name | Event | What it does |
|---|---|---|
| `backslash-shrink-guard` | PreToolUse (Bash) | The Bash tool shrinks `\\` to `\`. Stops `\\` in a heredoc body or inside single quotes. For a command that passes an inline script (node -e, python -c, sed, awk, ...), stops `\\` once per session and command (sent again, it only notes it) |
| `rm-junction-guard` | PreToolUse (Bash) | Stops `rm -r` on a folder that holds node_modules (it can delete through a junction) |
| `askuser-label-guard` | PreToolUse (AskUserQuestion) | Stops when an option label does not start with an ID A, B, C, ..., or when an option of a question with a preview has a description (it is not shown). Asks for long explanations to go to a ysidepaney note |
| `sendfile-to-ysidepaney` | PreToolUse (SendUserFile) | Puts images in ysidepaney. For videos (mp4, mov, webm, mkv, avi, m4v, wmv), opens their folder in Explorer with the file selected, keeps other windows from taking input while it opens, and minimizes it right after (Windows only; the window shows for a moment). Then stops SendUserFile. For files it could not place or open, and for other files, asks to send them again |
| `stage-own-edit-reminder` | PreToolUse (Edit, Write) | Stops the first edit of a tracked file that differs from HEAD, and asks to take a copy with `git stage-own-edit begin` before sending it again (once per session and file; the edit sent again goes through; a copy counts as begun only while it matches the current file) |
| `git-add-own-check` | PreToolUse (Bash) | Stops `git add <file>` of a tracked file that differs from HEAD unless this session made its first change with Edit or Write (known from the mark `stage-own-edit-reminder` leaves, so enable both): once per session and file to check the diff first (the command sent again goes through), and every time for a file with a `git stage-own-edit begin` copy, asking to stage the session's own lines with `git stage-own-edit stage` |
| `push-check-reminder` | PreToolUse (Bash) | Stops `git push` the first time for each session, repository and HEAD, with the checks to run before a push. The same push sent again goes through. When the command runs other commands too (`git add … && git push`; `cd` and commands that only filter the output aside), the message starts by saying that none of them ran either, and asks to check with `git status` and `git log -1` before sending it again. A push chained after a `git commit` in the same command is stopped every time, asking to send the commit first: a hook sees the command before it runs, so the commit made there could not be checked (by this check or by `private-word-guard`) before it is pushed. For a push to a public repository whose commits to push (since the upstream, else since origin's default branch) change text for readers (README, CHANGELOG, CONTRIBUTING or LICENSE files, Markdown (`.md`/`.mdx`), `package.json`, or added comment lines; counted as changed when the base to compare with is unknown), it also asks for a subagent to review that text, reading it as an intended user who does not know the author and checking that it does not assume the author's setup, the history of the work, or terms used only in the author's conversations with the agent |
| `public-repo-writing-reminder` | PreToolUse (Edit, Write, Bash, PowerShell) | In a public repository, notes that it is a product for other people, and asks to decide first who the intended users are and what they are promised (what it does, supported environments, install and removal, limits), to write for them without assuming the author's setup, the history of the work or terms used only in the author's conversations with the agent, to use the language of the README and existing commits, and to compare with the usual form of public repositories of its kind. Once per session and repository at each of two points: the first file written, and the first `gh pr create` or `gh release create` (the repository named by `-R`/`--repo`, else where the command runs). At the second point it adds that the description cannot be made private afterwards. Whether a repository is public is found as in `wago-verb-reminder` |
| `private-word-guard` | PreToolUse (Bash) | Before `git push`, looks for words that could identify a person in the added lines and commit messages of commits no remote has yet (`HEAD --not --remotes`). On a match, stops once to think it over (once per session, repository and set of words and places found; the same push sent again goes through), and, like `push-check-reminder`, says when other commands chained with the push did not run either. It is not a deny list, so the list can be broad. Words and repositories to skip go in `~/.claude/private-words.toml` (`PRIVATE_WORDS_FILE`) as `words = [...]` and `skip_repos = ["owner/repo"]`. Words of ASCII letters and digits only match at word boundaries, and the name of the repository pushed to is not matched. Without the file it does nothing |
| `heredoc-size-guard` | PreToolUse (Bash) | Stops once per session and command when a heredoc body is longer than 100 lines and holds single quotes, since the Bash tool's wrapping of the command can fail with `unexpected EOF` and run nothing. Asks to write the file with the Write tool and run it (sent again, it goes through) |
| `post-push-reminder` | PostToolUse (Bash) | After a successful push, notes the repository's post-push script |
| `head-truncation-reminder` | PostToolUse (Bash) | When the output reaches the limit of the last `\| head`, asks to count the results |
| `label-guard` | Stop | Stops headings labeled with Greek letters or Roman numerals |
| `language-guard` | Stop | Stops when the last reply is mostly English |
| `turn-state` | Stop, PostToolUse (Monitor) | At the end of a turn, writes the background work still running to `<session>.stop.json` (see "turn-state" below). Returns nothing |
| `wago-verb-reminder` | PreToolUse (input to a browser page, sending a message, Write, Edit, Bash, PowerShell) | In Japanese text that goes out, lists verbs that are not Sino-Japanese compounds (native Japanese verbs). Auxiliary words (為る, 有る, 居る, 置く, 因る) are left out. It also notes phrases that make the text read as machine-written or translated from English (translated terms and metaphors, words that seldom say anything, set phrases), with the cases where each may stay; quoted phrases (「…」, `>`) are not noted. The phrase patterns come from [yomiyasu](https://github.com/nanaism/yomiyasu) (see `THIRD-PARTY-NOTICES.md`). Actions that may send or publish something (sending a message, computer on a page, `gh pr create`/`edit` and `gh release create`/`edit`) cannot be taken back once seen, so it stops once per session and text (sent again, it goes through). Scripts run on a page (javascript_tool) are code and are not read. For Write and Edit, only the README and HTML of a public repository (found with `gh api` and remembered for a day; counted as public when it cannot be found). For gh, only a public repository (the one named by `-R`/`--repo`, else where the command runs); it reads the title, body and notes given in the command (inline or in a heredoc) and the files named by `--body-file`, `--notes-file` or `-F`. The dictionary is described under "Install" |

### turn-state

At the end of a turn, writes to a file whether background work that lets Claude continue on its own (shells and Monitors awaiting completion, subagents, scheduled wakeups) is still running. Tools that read it (plugins, for example) can tell whether Claude is waiting for the user's input or for background work to finish.

- Location: `$TURN_STATE_DIR`, else `~/.claude/state/turn-state`
- `<session>.stop.json` (rewritten at every Stop): `{ "at": <ms>, "tasks": [...], "dropped": [...], "crons": <count> }`
  - `tasks`: the `background_tasks` of the Stop input that can finish on their own and wake Claude (`id`, `type`, `description`, `command`)
  - `dropped`: commands that run until stopped (development servers, watch, ...) and work matching `ignore_task` (such as watching a published page for comments, which only a person's input wakes). They do not wake Claude by finishing, so they are left out
  - `crons`: the number of `session_crons` (ScheduleWakeup, CronCreate, /loop)
- `<session>.monitors`: the task IDs of Monitors. The Stop input lists a Monitor as `type: "shell"`, so they are remembered from `tool_response.taskId` at PostToolUse and kept out of `dropped`

Commands that run until stopped, and work not to count, go in `~/.claude/hooks/turn-state.toml` (outside the rules folders, since rule files accept only rules).

```toml
[[resident]]
pattern = '(pnpm|npm|yarn|bun)(\s+run)?\s+(dev|start|serve|preview)(\s|$)'  # matched from the start of the command
except = ['\bbuild\b']                                                     # none may match

[[ignore_task]]
type = 'monitor'                                  # the same string as the type in background_tasks
description = '^live updates for artifact '       # regular expression matching somewhere in the description
```

A command is matched part by part, split at `&&`, `||`, `;`, `|` and newlines outside quotes. From the start of each part, `VAR=value`, `timeout <n>`, `nohup`, `exec`, `time`, `(` and `{` are removed, and parts that only prepare (`cd`, `sleep`, `rm`, `echo`, ...) are skipped. Matching is done against the string as written, quotes included.

## Commands

```sh
yhooky hook                   # run as a hook: input on stdin, answer on stdout
yhooky explain                # for the input on stdin, print what each rule returns, as JSON lines
yhooky check                  # read the rule files and report the ones that cannot be used
yhooky test <cases.toml>...   # check rules against example inputs (format in src/cases.rs)
yhooky language-check <f>     # list the turns of past transcripts that language-guard would stop
yhooky show-file <path>...    # open each file's folder in Explorer with the file selected, then minimize it (started by sendfile-to-ysidepaney)
```

## License

MIT OR Apache-2.0

Parts taken from other projects keep their own licenses: see `THIRD-PARTY-NOTICES.md`.
