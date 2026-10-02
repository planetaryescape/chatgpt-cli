# Find your brainstorms

List the chats where you were shaping an article, sermon, devotional, talk, book or app, and pull one out to keep working on it in Claude Code.

```sh
chatgpt list --brainstorm writing
```

```text
00000000-0000-4000-8000-000000000003  2026-09-25    J  keep     writing_creativity    idea:writing  Reading List Ideas
```

The `idea:` column shows what Jev thinks the brainstorm was for.

## Filter by kind

```sh
chatgpt list --brainstorm              # any kind
chatgpt list --brainstorm writing      # general writing, even when faith informs part of it
chatgpt list --brainstorm sermon       # religious teaching or reflection as the main purpose
chatgpt list --brainstorm product      # apps, products, tools, businesses
chatgpt list --brainstorm other        # other creative or personal projects
chatgpt stats                          # counts per kind
```

Combine with any other filter:

```sh
chatgpt list --brainstorm product --newer-than 6m
chatgpt list --brainstorm sermon --topic faith
```

In the TUI, press `b` to cycle through `any`, `writing`, `sermon`, `product` and `other`. The list shows the kind in the `idea:` column.

## Hand one to Claude Code

```sh
chatgpt export <chat-id> --copy
```

Or select it in the TUI and press `c`. Canvas documents come out at the end in their final state, which is usually the most refined version of the idea. See [Export a conversation](export-a-conversation.md).

## What counts as a brainstorm

Jev counts a chat as a brainstorm when you develop your own idea for something you want to create. That includes drafting or outlining a talk, article, sermon, devotional, Bible study, abstract or essay. It also includes chats that started as something else, such as life advice that turned into an article idea. `product` requires a separate product-idea score: it covers your own app, tool, product or business concept, including a substantial new feature direction. Routine coding, settled implementation, a tutorial, an interview exercise or work on someone else's product does not qualify by itself. Luna reviews every proposed product label and removes it when the chat only implements, documents or explains an already established product. Use `sermon` when religious teaching or reflection is the piece's main purpose; use `writing` for broader work even when faith informs part of it. Faith questions without a piece being developed are not brainstorms. Brainstorms are always suggested keep. Counts are conversations, not distinct products or pieces.

A brainstorm needs a score of at least 0.6 on the brainstorming question. Chats close to that line show as `keep?`. The full rules are in the [classification reference](../reference/classification.md).

## If a brainstorm is missing

- Run `chatgpt classify`. Only judged chats have a brainstorm verdict, and `chatgpt stats` shows how many aren't judged yet.
- Pinned chats are skipped by `classify` unless you pass `--pinned`.
- Jev can miss one. Search by title (`chatgpt list --title "outline"`) or by content (`chatgpt search "newsletter"`). The daemon indexes every transcript in the background; `chatgpt search-index` waits until it has.
