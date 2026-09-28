# Manage memories

List saved memories directly from your ChatGPT account:

```sh
chatgpt memory list
chatgpt memory list --search 'keyboard'
chatgpt memory list --limit 10
chatgpt memory list --format json | jq '.[] | {id, content, status}'
```

`memory list` reads the saved-memory entries shown under Personalization. It does not use the conversation index, so you do not need to run `sync`. Use `--format csv` for a spreadsheet or `--format ids` to pipe ids into another command. The default table shortens long memory text; JSON and CSV keep the full content.

## Read the memory summary

```sh
chatgpt memory summary
chatgpt memory summary --format json | jq '.sections[] | {title, description}'
```

The generated memory summary is separate from the list of saved-memory entries. Its sections can change as ChatGPT updates what it remembers. The command reads the current summary from ChatGPT and does not save it locally.
The CLI currently reads the summary; corrections to it use a separate ChatGPT web flow.

## Classify saved memories

```sh
chatgpt memory classify --suggest delete
chatgpt memory classify --suggest review --format json
chatgpt memory classify --suggest delete --format ids
chatgpt memory classify --limit 10
```

Classification reads every current saved memory. Jev quickly keeps clear, lasting context. Luna reviews everything else, including every potential deletion. Results are cached in the local SQLite database and rechecked when the memory or a related memory changes, when the rubric version changes, or in a new calendar month. `--redo` repeats the whole run. `--suggest` and `--limit` filter the displayed results after classification; `--limit` does not reduce the first run's model work.

The outcomes are **keep**, **delete**, and **review**. A delete is a recommendation only: this command never deletes. Review means the evidence is insufficient, often because a project's current status cannot be inferred from a dated saved memory. Use `--format json` or `csv` for full content and reasons; `ids` produces one id per line. [The rubric](../reference/memory-classification.md) explains the decisions.

Run `chatgpt stats` to see the current saved-memory total and the cached counts for keep, delete, review, and not classified. Stats reads the live memory list but does not classify; stale or new entries appear as not classified. Chat filters such as `--limit` only affect the chat part of stats.

## Delete a saved memory

```sh
chatgpt memory delete <memory-id> --dry-run
chatgpt memory delete <memory-id>
```

The dry run shows the memory id and a short preview. The second command asks you to type the number of memories to confirm deletion. You can pass multiple full ids or unique prefixes, or pipe ids using `-`:

```sh
chatgpt memory list --search 'obsolete' --format ids | chatgpt memory delete - --dry-run
```

Deleting a saved memory does not delete the chat where it came from. Deleting a chat does not delete its saved memory. [OpenAI's memory guide](https://help.openai.com/en/articles/8590148-memory-in-chatgpt) explains how other sources can continue to supply remembered information. This command does not act on source chats, including archived chats.

## See also

- [CLI reference](../reference/cli.md) for every `memory` option.
- [The chatgpt.com web API](../explanation/chatgpt-api.md) for the observed endpoints.
