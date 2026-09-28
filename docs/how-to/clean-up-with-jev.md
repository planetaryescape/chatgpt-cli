# Clean up classified chats from the command line

Delete or archive classified chats in bulk from the command line. `classify` uses Jev, its follow-up, and Luna for cases that remain unsure or have conflicting evidence. One-off help whose useful moment has passed can be classified delete when it leaves no lasting record. To review chats one at a time, use `chatgpt tui` instead ([tutorial](../tutorial.md)).

Start from a current index and current judgments:

```sh
chatgpt sync
chatgpt classify
```

## Preview, then delete

Always run the dry run first:

```sh
chatgpt delete --suggest delete --dry-run
```

It prints up to 25 matching chats, newest first, then the total. The output looks like this:

```text
00000000-0000-4000-8000-000000000004  2026-09-08       Old Weather Check
…
dry run: would delete 3 conversation(s).
```

When the list looks right, run it for real:

```sh
chatgpt delete --suggest delete
```

It asks you to type the number of chats before it deletes anything:

```text
Permanently delete 3 conversation(s)? This cannot be undone. Type 3 to confirm:
```

Deleted chats can't be recovered. If you're unsure about a group, archive it instead; that is reversible.

## Check a hand-picked set

`--check` reads each target's current classification (asking Jev about any not yet judged) and acts only on chats with a sure supporting suggestion:

```sh
chatgpt delete --title "^New chat$" --check --dry-run
```

```text
Classification backs delete for 3 of 30.
Held back (suggestion, then the title):
  keep        New chat  (nothing 0.02 · re-askable 0.10 · worth 2.8/3 · …)
  archive?    New chat  (…)
```

A delete needs a sure delete suggestion. An archive is allowed for a sure delete or archive suggestion.

## Narrow the target

Every filter combines with `--suggest`:

```sh
chatgpt delete --suggest delete --older-than 1y --dry-run      # only chats last touched over a year ago
chatgpt delete --suggest delete --topic coding_general --dry-run
chatgpt archive --suggest archive --dry-run
chatgpt list --suggest delete --count                          # just the number
```

To hand-edit the list first, pipe it:

```sh
chatgpt list --suggest delete > deletes.txt
# remove lines for chats you want to keep, then
chatgpt delete - < deletes.txt
```

The confirmation prompt still appears; it reads your answer from the terminal.

## Undo an archive

```sh
chatgpt unarchive --title "rental car" --dry-run
chatgpt unarchive --title "rental car"
```

`unarchive` targets archived chats by default.

## Leave some chats alone

- Pinned chats are skipped unless you pass `--pinned`.
- `delete --suggest delete` skips `delete?` chats, and `archive --suggest archive` skips `archive?` chats. Use `--check` when acting on other filters or explicit ids.
- Brainstorms are always suggested keep, so `--suggest delete` never includes them.
