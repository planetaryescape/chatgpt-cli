# Export a conversation

Copy a ChatGPT conversation as markdown, to hand to Claude Code or save as a file.

```sh
chatgpt export https://chatgpt.com/c/00000000-0000-4000-8000-000000000001 --copy
```

```text
copied "Garden Lighting Notes" to the clipboard (<size> KB)
```

Paste it wherever you need it. Exporting doesn't need `chatgpt sync`.

## Choose where it goes

```sh
chatgpt export <link>                  # print to stdout
chatgpt export <link> --copy           # clipboard (macOS pbcopy)
chatgpt export <link> -o               # file named after the title, e.g. garden-lighting-notes.md
chatgpt export <link> -o idea.md       # file with the name you choose
chatgpt export <link> --copy -o        # both
```

`<link>` accepts:

- a chat link: `https://chatgpt.com/c/<id>`
- a project chat link: `https://chatgpt.com/g/g-p-<project>/c/<id>`
- a full id, such as `00000000-0000-4000-8000-000000000001`
- an id prefix from the local index, such as `00000000-0000`. This one needs `chatgpt sync` first.

Shared links (`/share/…`) aren't supported. Open the chat and use its own `/c/…` link.

## What's in the export

The export follows the thread you see in the ChatGPT UI, which is the branch you last used if you edited or regenerated a message. It contains:

- A header: title, link, creation date, model.
- Each turn under `## Me` or `## ChatGPT`, separated by `---`.
- Voice-mode chats as their transcripts.
- Web citations as markdown links: `([Visual Studio Code](https://…))`.
- Images as `[image]` and ChatGPT-generated images as `[generated image]`; files as `[attached file: name.pdf]`.
- Canvas documents at the end, in their final edited state, under `## Canvas (final): <name>`.

It leaves out ChatGPT's hidden reasoning, tool calls and search results.

## From the TUI

Select the chat in `chatgpt tui` and press `c`. That copies the same markdown.

## If it fails

- `No conversation matching "<prefix>"`: the prefix isn't in the index. Pass the full link, or run `chatgpt sync`.
- `rate limited by ChatGPT; waiting 5s`: single-chat fetches are rate-limited. The CLI waits and retries for up to about 75 seconds. See [Troubleshooting](troubleshoot.md#rate-limited-by-chatgpt).
