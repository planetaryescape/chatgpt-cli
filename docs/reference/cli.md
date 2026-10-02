<!-- Generated from the binary's --help output by crates/cli/tests/cli_reference.rs. Edit the command definitions in crates/cli/src/args.rs, then run `CHATGPT_UPDATE_CLI_REFERENCE=1 cargo nextest run -p chatgpt-cli --test cli_reference`. -->

# CLI reference

Every command and flag, taken from `chatgpt <command> --help`.

Shared behaviour:

- **Filters.** `--older-than`, `--newer-than`, `--before`, `--after`, `--title`, `--archived`, `--all`, `--limit`, `--suggest`, `--topic` and `--brainstorm` work the same on every command that lists them. They read the local index, which the daemon keeps fresh.
- **Targets.** Commands that take `[ids...]` accept full ids, unique id prefixes, or `-` to read ids from stdin (the first column of `list` output works). A repeated id counts once.
- **Pinned chats.** Bulk commands skip pinned chats unless you pass `--pinned`.
- **Recommendations.** Preview with `archive --suggest archive --dry-run` or `delete --suggest delete --dry-run`, then repeat without `--dry-run` to apply. Both commands skip unsure `?` chats. Deletion sends up to three requests at a time.
- **Output.** Results go to stdout; progress, prompts and errors go to stderr, so piping `list` into another command stays clean.
- **Exit codes.** `0` on success. `2` for invalid arguments or input (an unknown command or flag included), `4` when no browser session can be read, `6` when ChatGPT's rate limit holds, `7` when this build or platform can't do it, and `1` for any other failure, including any item in a bulk action failing.

## `chatgpt`

```text
Manage your ChatGPT conversations from the terminal (uses your browser login).

Usage: chatgpt [OPTIONS] <COMMAND>

Commands:
  configure     Store Jev, OpenAI, or Anthropic API keys in the user config; omit provider to show status
  sync          Update the local index: new and changed chats, plus archive state
  list          List conversations from the local index
  stats         Chat suggestions, brainstorms and topics, plus saved-memory suggestions
  export        Export a conversation as markdown: chat link, id, or id prefix [alias: show]
  search        Search the local transcript index; use --semantic or --hybrid for meaning-based matches
  search-index  Build or refresh the local text and semantic search index
  daemon        Start, stop and inspect the background daemon
  archive       Archive conversations by id/prefix, `-` for ids on stdin, or by filter
  unarchive     Unarchive conversations by id/prefix, `-` for ids on stdin, or by filter
  delete        Delete conversations by id/prefix, `-` for ids on stdin, or by filter
  rename        Rename a conversation in ChatGPT (changes its order in the app)
  title         Set a local display title without changing ChatGPT
  titles        Generate local display titles and topic themes with gpt-6-luna
  classify      Classify with Jev and Luna, then generate missing local titles and topic themes
  project       Create, list and manage chat projects
  memory        List and delete saved memories, or read the memory summary
  review        Triage conversations one by one; changes apply after a final confirmation
  tui           Browse, filter and triage conversations in a terminal UI
  help          Print this message or the help of the given subcommand(s)

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
  -V, --version         Print version
```

## `chatgpt configure`

```text
Store Jev, OpenAI, or Anthropic API keys in the user config; omit provider to show status

Usage: chatgpt configure [OPTIONS] [provider]

Arguments:
  [provider]  jev, openai, or anthropic

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --remove          Remove the selected provider's stored key
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt sync`

```text
Update the local index: new and changed chats, plus archive state

Usage: chatgpt sync [OPTIONS]

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --full            Rebuild from scratch (also drops chats deleted in the browser)
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt list`

```text
List conversations from the local index

Usage: chatgpt list [OPTIONS]

Options:
      --browser <name>       Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --older-than <age>     Last updated more than <age> ago (30d, 12w, 6m, 2y)
      --newer-than <age>     Last updated within <age>
      --profile <name>       Use a browser profile directory (with --browser)
      --before <date>        Last updated before YYYY-MM-DD
      --after <date>         Last updated on or after YYYY-MM-DD
      --title <regex>        Title matches regex (case-insensitive)
      --archived             Archived conversations instead of active ones
      --all                  Both active and archived
      --limit <n>            At most n conversations (newest first)
      --suggest <action>     Only chats classified as delete, archive, or keep (needs `classify`)
      --topic <topic>        Only chats classified in this topic (`chatgpt stats` lists them)
      --brainstorm [<kind>]  Only chats where you were brainstorming; optionally writing, sermon, product, or other
      --json                 Output JSON
      --format <format>      Output format: ids (default: text)
      --count                Print only the number of matches
  -h, --help                 Print help
```

## `chatgpt stats`

```text
Chat suggestions, brainstorms and topics, plus saved-memory suggestions

Usage: chatgpt stats [OPTIONS]

Options:
      --browser <name>       Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --older-than <age>     Last updated more than <age> ago (30d, 12w, 6m, 2y)
      --newer-than <age>     Last updated within <age>
      --profile <name>       Use a browser profile directory (with --browser)
      --before <date>        Last updated before YYYY-MM-DD
      --after <date>         Last updated on or after YYYY-MM-DD
      --title <regex>        Title matches regex (case-insensitive)
      --archived             Archived conversations instead of active ones
      --all                  Both active and archived
      --limit <n>            At most n conversations (newest first)
      --suggest <action>     Only chats classified as delete, archive, or keep (needs `classify`)
      --topic <topic>        Only chats classified in this topic (`chatgpt stats` lists them)
      --brainstorm [<kind>]  Only chats where you were brainstorming; optionally writing, sermon, product, or other
  -h, --help                 Print help
```

## `chatgpt search`

```text
Search the local transcript index; use --semantic or --hybrid for meaning-based matches

Usage: chatgpt search [OPTIONS] <QUERY>

Arguments:
  <QUERY>  

Options:
      --browser <name>   Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --semantic         Rank by local embedding similarity
      --hybrid           Combine full-text and semantic ranking
      --profile <name>   Use a browser profile directory (with --browser)
      --remote           Use ChatGPT's server-side search instead
      --archived         Search archived conversations instead of active ones
      --all              Search both active and archived conversations
      --format <format>  Output format: json, csv, table, or ids (default: two-line text)
      --limit <n>        Maximum conversations [default: 20]
  -h, --help             Print help
```

## `chatgpt search-index`

```text
Build or refresh the local text and semantic search index

Usage: chatgpt search-index [OPTIONS]

Options:
      --archived        Index archived conversations instead of active ones
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --all             Index both active and archived conversations
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt memory`

```text
List and delete saved memories, or read the memory summary

Usage: chatgpt memory [OPTIONS] <COMMAND>

Commands:
  list      List saved memories from ChatGPT
  classify  Classify saved memories for keep, delete, or review with Jev and Luna; never deletes
  summary   Read ChatGPT's generated memory summary
  delete    Delete saved memories by id/prefix or `-` for ids on stdin; does not delete source chats
  help      Print this message or the help of the given subcommand(s)

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt memory list`

```text
List saved memories from ChatGPT

Usage: chatgpt memory list [OPTIONS]

Options:
      --browser <name>   Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --search <text>    Only memories whose content contains text
      --limit <n>        At most n saved memories
      --profile <name>   Use a browser profile directory (with --browser)
      --format <format>  Output format: json, csv, table, or ids [default: table]
  -h, --help             Print help
```

## `chatgpt memory classify`

```text
Classify saved memories for keep, delete, or review with Jev and Luna; never deletes

Usage: chatgpt memory classify [OPTIONS]

Options:
      --browser <name>    Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --suggest <action>  Show only keep, delete, or review
      --limit <n>         At most n results after filtering
      --profile <name>    Use a browser profile directory (with --browser)
      --format <format>   Output format: json, csv, table, or ids [default: table]
      --redo              Reclassify all saved memories
  -h, --help              Print help
```

## `chatgpt memory summary`

```text
Read ChatGPT's generated memory summary

Usage: chatgpt memory summary [OPTIONS]

Options:
      --browser <name>   Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --format <format>  Output format: json or table [default: table]
      --profile <name>   Use a browser profile directory (with --browser)
  -h, --help             Print help
```

## `chatgpt memory delete`

```text
Delete saved memories by id/prefix or `-` for ids on stdin; does not delete source chats

Usage: chatgpt memory delete [OPTIONS] <ids>...

Arguments:
  <ids>...  Saved memory ids or id prefixes, or `-` to read them from stdin

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
  -n, --dry-run         Preview without deleting memories
      --profile <name>  Use a browser profile directory (with --browser)
  -y, --yes             Skip the confirmation prompt
  -h, --help            Print help
```

## `chatgpt project`

```text
Create, list and manage chat projects

Usage: chatgpt project [OPTIONS] <COMMAND>

Commands:
  create  Create a project with ChatGPT's default settings
  list    List projects available to your account
  add     Move chats into an existing project by name or id; use `-` for ids on stdin
  remove  Remove chats from an existing project; use `-` for ids on stdin
  delete  Delete a project in ChatGPT; asks you to type its name to confirm
  help    Print this message or the help of the given subcommand(s)

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt project create`

```text
Create a project with ChatGPT's default settings

Usage: chatgpt project create [OPTIONS] <name>

Arguments:
  <name>  

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --json            Output JSON
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt project list`

```text
List projects available to your account

Usage: chatgpt project list [OPTIONS]

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --json            Output JSON
      --limit <n>       At most n projects
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt project add`

```text
Move chats into an existing project by name or id; use `-` for ids on stdin

Usage: chatgpt project add [OPTIONS] <project> <ids>...

Arguments:
  <project>  Project name, id or id prefix
  <ids>...   Conversation ids or id prefixes, or `-` to read them from stdin

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
  -n, --dry-run         Preview without changing anything
      --profile <name>  Use a browser profile directory (with --browser)
  -y, --yes             Skip the confirmation prompt
      --archived        Select archived conversations instead of active ones
      --all             Select both active and archived conversations
  -h, --help            Print help
```

## `chatgpt project remove`

```text
Remove chats from an existing project; use `-` for ids on stdin

Usage: chatgpt project remove [OPTIONS] <project> <ids>...

Arguments:
  <project>  Project name, id or id prefix
  <ids>...   Conversation ids or id prefixes, or `-` to read them from stdin

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
  -n, --dry-run         Preview without changing anything
      --profile <name>  Use a browser profile directory (with --browser)
  -y, --yes             Skip the confirmation prompt
      --archived        Select archived conversations instead of active ones
      --all             Select both active and archived conversations
  -h, --help            Print help
```

## `chatgpt project delete`

```text
Delete a project in ChatGPT; asks you to type its name to confirm

Usage: chatgpt project delete [OPTIONS] <project>

Arguments:
  <project>  Project name, id or id prefix

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
  -n, --dry-run         Preview without changing anything
      --profile <name>  Use a browser profile directory (with --browser)
  -y, --yes             Skip the confirmation prompt
  -h, --help            Print help
```

## `chatgpt export`

```text
Export a conversation as markdown: chat link, id, or id prefix

Usage: chatgpt export [OPTIONS] <LINK>

Arguments:
  <LINK>  Chat link, id, or id prefix

Options:
      --browser <name>   Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
  -o, --output [<file>]  Write to a file (default name: from the title)
  -c, --copy             Copy to the clipboard
      --profile <name>   Use a browser profile directory (with --browser)
      --archived         Allow an archived conversation
      --all              Allow either active or archived
  -h, --help             Print help
```

## `chatgpt classify`

```text
Classify with Jev and Luna, then generate missing local titles and topic themes

Usage: chatgpt classify [OPTIONS] [ids]...

Arguments:
  [ids]...  Conversation ids or id prefixes, or `-` to read them from stdin

Options:
      --browser <name>       Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --older-than <age>     Last updated more than <age> ago (30d, 12w, 6m, 2y)
      --newer-than <age>     Last updated within <age>
      --profile <name>       Use a browser profile directory (with --browser)
      --before <date>        Last updated before YYYY-MM-DD
      --after <date>         Last updated on or after YYYY-MM-DD
      --title <regex>        Title matches regex (case-insensitive)
      --archived             Archived conversations instead of active ones
      --all                  Both active and archived
      --limit <n>            At most n conversations (newest first)
      --suggest <action>     Only chats classified as delete, archive, or keep (needs `classify`)
      --topic <topic>        Only chats classified in this topic (`chatgpt stats` lists them)
      --brainstorm [<kind>]  Only chats where you were brainstorming; optionally writing, sermon, product, or other
      --pinned               Include pinned conversations (skipped by default)
      --redo                 Re-judge every matching chat, not just new or changed ones (reuses cached transcripts and summaries)
  -y, --yes                  Don't ask before summarising a large batch
  -h, --help                 Print help
```

## `chatgpt titles`

```text
Generate local display titles and topic themes with gpt-6-luna

Usage: chatgpt titles [OPTIONS] [ids]...

Arguments:
  [ids]...  Conversation ids or id prefixes, or `-` to read them from stdin

Options:
      --browser <name>       Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --older-than <age>     Last updated more than <age> ago (30d, 12w, 6m, 2y)
      --newer-than <age>     Last updated within <age>
      --profile <name>       Use a browser profile directory (with --browser)
      --before <date>        Last updated before YYYY-MM-DD
      --after <date>         Last updated on or after YYYY-MM-DD
      --title <regex>        Title matches regex (case-insensitive)
      --archived             Archived conversations instead of active ones
      --all                  Both active and archived
      --limit <n>            At most n conversations (newest first)
      --suggest <action>     Only chats classified as delete, archive, or keep (needs `classify`)
      --topic <topic>        Only chats classified in this topic (`chatgpt stats` lists them)
      --brainstorm [<kind>]  Only chats where you were brainstorming; optionally writing, sermon, product, or other
      --pinned               Include pinned conversations (skipped by default)
      --redo                 Regenerate Luna titles; manual titles are preserved
  -h, --help                 Print help
```

## `chatgpt title`

```text
Set a local display title without changing ChatGPT

Usage: chatgpt title [OPTIONS] <id> <title>

Arguments:
  <id>     Conversation id or id prefix
  <title>  The new title

Options:
      --archived        Allow an archived conversation
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --all             Allow either active or archived
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt review`

```text
Triage conversations one by one; changes apply after a final confirmation

Usage: chatgpt review [OPTIONS] [ids]...

Arguments:
  [ids]...  Conversation ids or id prefixes, or `-` to read them from stdin

Options:
      --browser <name>       Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --older-than <age>     Last updated more than <age> ago (30d, 12w, 6m, 2y)
      --newer-than <age>     Last updated within <age>
      --profile <name>       Use a browser profile directory (with --browser)
      --before <date>        Last updated before YYYY-MM-DD
      --after <date>         Last updated on or after YYYY-MM-DD
      --title <regex>        Title matches regex (case-insensitive)
      --archived             Archived conversations instead of active ones
      --all                  Both active and archived
      --limit <n>            At most n conversations (newest first)
      --suggest <action>     Only chats classified as delete, archive, or keep (needs `classify`)
      --topic <topic>        Only chats classified in this topic (`chatgpt stats` lists them)
      --brainstorm [<kind>]  Only chats where you were brainstorming; optionally writing, sermon, product, or other
      --pinned               Include pinned conversations (skipped by default)
      --oldest-first         Start from the oldest match
  -h, --help                 Print help
```

## `chatgpt tui`

```text
Browse, filter and triage conversations in a terminal UI

Usage: chatgpt tui [OPTIONS]

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt archive`

```text
Archive conversations by id/prefix, `-` for ids on stdin, or by filter

Usage: chatgpt archive [OPTIONS] [ids]...

Arguments:
  [ids]...  Conversation ids or id prefixes, or `-` to read them from stdin

Options:
      --browser <name>       Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --older-than <age>     Last updated more than <age> ago (30d, 12w, 6m, 2y)
      --newer-than <age>     Last updated within <age>
      --profile <name>       Use a browser profile directory (with --browser)
      --before <date>        Last updated before YYYY-MM-DD
      --after <date>         Last updated on or after YYYY-MM-DD
      --title <regex>        Title matches regex (case-insensitive)
      --archived             Archived conversations instead of active ones
      --all                  Both active and archived
      --limit <n>            At most n conversations (newest first)
      --suggest <action>     Only chats classified as delete, archive, or keep (needs `classify`)
      --topic <topic>        Only chats classified in this topic (`chatgpt stats` lists them)
      --brainstorm [<kind>]  Only chats where you were brainstorming; optionally writing, sermon, product, or other
      --pinned               Include pinned conversations (skipped by default)
  -n, --dry-run              Show what would change
  -y, --yes                  Skip the confirmation prompt
      --check                Ask Jev to read each chat and only act on those it agrees with (archive/delete)
  -h, --help                 Print help
```

## `chatgpt unarchive`

```text
Unarchive conversations by id/prefix, `-` for ids on stdin, or by filter

Usage: chatgpt unarchive [OPTIONS] [ids]...

Arguments:
  [ids]...  Conversation ids or id prefixes, or `-` to read them from stdin

Options:
      --browser <name>       Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --older-than <age>     Last updated more than <age> ago (30d, 12w, 6m, 2y)
      --newer-than <age>     Last updated within <age>
      --profile <name>       Use a browser profile directory (with --browser)
      --before <date>        Last updated before YYYY-MM-DD
      --after <date>         Last updated on or after YYYY-MM-DD
      --title <regex>        Title matches regex (case-insensitive)
      --archived             Archived conversations instead of active ones
      --all                  Both active and archived
      --limit <n>            At most n conversations (newest first)
      --suggest <action>     Only chats classified as delete, archive, or keep (needs `classify`)
      --topic <topic>        Only chats classified in this topic (`chatgpt stats` lists them)
      --brainstorm [<kind>]  Only chats where you were brainstorming; optionally writing, sermon, product, or other
      --pinned               Include pinned conversations (skipped by default)
  -n, --dry-run              Show what would change
  -y, --yes                  Skip the confirmation prompt
      --check                Ask Jev to read each chat and only act on those it agrees with (archive/delete)
  -h, --help                 Print help
```

## `chatgpt delete`

```text
Delete conversations by id/prefix, `-` for ids on stdin, or by filter

Usage: chatgpt delete [OPTIONS] [ids]...

Arguments:
  [ids]...  Conversation ids or id prefixes, or `-` to read them from stdin

Options:
      --browser <name>       Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --older-than <age>     Last updated more than <age> ago (30d, 12w, 6m, 2y)
      --newer-than <age>     Last updated within <age>
      --profile <name>       Use a browser profile directory (with --browser)
      --before <date>        Last updated before YYYY-MM-DD
      --after <date>         Last updated on or after YYYY-MM-DD
      --title <regex>        Title matches regex (case-insensitive)
      --archived             Archived conversations instead of active ones
      --all                  Both active and archived
      --limit <n>            At most n conversations (newest first)
      --suggest <action>     Only chats classified as delete, archive, or keep (needs `classify`)
      --topic <topic>        Only chats classified in this topic (`chatgpt stats` lists them)
      --brainstorm [<kind>]  Only chats where you were brainstorming; optionally writing, sermon, product, or other
      --pinned               Include pinned conversations (skipped by default)
  -n, --dry-run              Show what would change
  -y, --yes                  Skip the confirmation prompt
      --check                Ask Jev to read each chat and only act on those it agrees with (archive/delete)
  -h, --help                 Print help
```

## `chatgpt rename`

```text
Rename a conversation in ChatGPT (changes its order in the app)

Usage: chatgpt rename [OPTIONS] <id> <title>

Arguments:
  <id>     Conversation id or id prefix
  <title>  The new title

Options:
      --archived        Allow an archived conversation
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --all             Allow either active or archived
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt daemon`

```text
Start, stop and inspect the background daemon

Usage: chatgpt daemon [OPTIONS] <COMMAND>

Commands:
  run        Run the daemon in the foreground
  status     Show the daemon's state, starting it if it isn't running
  stop       Stop the daemon
  logs       Print the daemon's log
  install    Start the daemon at login (writes a macOS LaunchAgent; doesn't load it)
  uninstall  Stop starting the daemon at login (removes the LaunchAgent)
  help       Print this message or the help of the given subcommand(s)

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt daemon status`

```text
Show the daemon's state, starting it if it isn't running

Usage: chatgpt daemon status [OPTIONS]

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --json            Output JSON
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt daemon stop`

```text
Stop the daemon

Usage: chatgpt daemon stop [OPTIONS]

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt daemon logs`

```text
Print the daemon's log

Usage: chatgpt daemon logs [OPTIONS]

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
  -f, --follow          Keep printing new lines as they're written
  -n, --lines <LINES>   How many of the last lines to print first [default: 50]
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt daemon install`

```text
Start the daemon at login (writes a macOS LaunchAgent; doesn't load it)

Usage: chatgpt daemon install [OPTIONS]

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt daemon uninstall`

```text
Stop starting the daemon at login (removes the LaunchAgent)

Usage: chatgpt daemon uninstall [OPTIONS]

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```

## `chatgpt daemon run`

```text
Run the daemon in the foreground

Usage: chatgpt daemon run [OPTIONS]

Options:
      --browser <name>  Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
      --profile <name>  Use a browser profile directory (with --browser)
  -h, --help            Print help
```
