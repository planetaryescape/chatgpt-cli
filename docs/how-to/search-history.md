# Search your history

Search cached conversations without asking ChatGPT's search API:

```sh
chatgpt sync
chatgpt search "garden lighting"
```

No search needs an indexing step: after every sync the background daemon fetches missing transcripts, splits them into short passages, indexes them for full-text search, then embeds them with a local model for semantic search. The first time, indexing takes a few minutes and embedding about half an hour; until they're done, `search` answers from what's ready and says `N of M chats indexed` (and, for `--semantic` and `--hybrid`, `N of M chunks embedded`) on stderr. Later syncs only fetch and embed new or changed chats. `chatgpt daemon status` shows the progress, and `chatgpt search-index` waits until both are done, showing their progress. Active, archived and pinned chats are all indexed.

Search shows active chats by default in every mode, including `--remote`. Pass `--archived` for archived chats only, or `--all` for both. These filters apply before the result limit in local search.
ChatGPT's remote search returns at most 40 message hits per request, so an archived filter or repeated hits from one chat can leave fewer than `--limit` conversations.

## Search by meaning

```sh
chatgpt search --semantic "choosing a power meter for home wiring"
chatgpt search --hybrid "power meter home wiring"
```

`--semantic` ranks passages by local embedding similarity, so the query need not share words with a result. `--hybrid` combines that ranking with full-text matches. Plain `search` uses SQLite full-text ranking and starts without loading the model. Every mode prints one result per conversation with a passage from the best matching part.

The daemon downloads the model (23 MB) from Hugging Face the first time it needs it, checks it against a pinned checksum and caches it under `~/.cache/chatgpt-cli/models` (or `$XDG_CACHE_HOME/chatgpt-cli/models`). Transcript and query embeddings run on your machine, in a low-priority background process that uses at most one CPU core; their text is not sent to Hugging Face. Offline, full-text search works as usual and semantic search says why it isn't ready. The model is English-focused, and semantic ranking can still miss a relevant passage. Use `--hybrid` when exact terms matter too. [Embeddings in the Rust daemon](../explanation/embeddings.md) explains the model and runtime.

## Pipe search results

```sh
chatgpt search --semantic "cache" --format json | jq -r '.[] | [.id, .title] | @tsv'
chatgpt search "Garden Planner" --format ids | chatgpt project add "Garden Planner" - --dry-run
chatgpt search "garden lighting" --format csv > matches.csv
chatgpt search "garden lighting" --format table
```

JSON is one array on stdout, including `[]` when nothing matches. JSON and CSV use the same fields: `id`, `title`, `updated` (ISO timestamp), `archived` (boolean), `score` (null for `--remote`), and `snippet`. Higher scores rank better within one search mode; scores from different modes are not comparable. CSV quotes fields containing commas, quotes or newlines. `ids` prints one full conversation id per line without a header; it prints nothing when there are no results. `table` shows shortened ids and snippets for terminal scanning; use JSON, CSV, ids or the default output when you need full values. Progress and errors go to stderr, so they do not break a pipe.

## Keep results current

```sh
chatgpt sync
chatgpt search-index   # optional: wait until every chat is indexed and embedded
```

Both indexes follow every sync on their own. Changed and deleted chats stop appearing as soon as the local conversation index changes, before their new passages are indexed.

The previous ChatGPT search remains available when you want it:

```sh
chatgpt search --remote "garden lighting"
```

## See also

- [CLI reference](../reference/cli.md) for all search flags.
- [Data and model storage](../reference/data.md) for the SQLite and cache paths.
- [How local search works](../explanation/how-it-works.md#searching-locally) for the index design.
