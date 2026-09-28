# Search your history

Build the local search index once, then search cached conversations without asking ChatGPT's search API:

```sh
chatgpt sync
chatgpt search-index
chatgpt search "garden lighting"
```

`search-index` downloads transcripts missing from the cache, splits them into short passages, builds a SQLite full-text index, and embeds the passages with a local model. The first run can take several minutes. Later runs only fetch and embed new or changed chats, and an interrupted run resumes from saved work. It includes pinned chats. Use `chatgpt search-index --archived` to index archived chats, or `--all` to index both states.

Search shows active chats by default in every mode, including `--remote`. Pass `--archived` for archived chats only, or `--all` for both. These filters apply before the result limit in local search. If an archived search is missing chats, run `chatgpt search-index --archived` to index them.
ChatGPT's remote search returns at most 40 message hits per request, so an archived filter or repeated hits from one chat can leave fewer than `--limit` conversations.

## Search by meaning

```sh
chatgpt search --semantic "choosing a power meter for home wiring"
chatgpt search --hybrid "power meter home wiring"
```

`--semantic` ranks passages by local embedding similarity, so the query need not share words with a result. `--hybrid` combines that ranking with full-text matches. Plain `search` uses SQLite full-text ranking and starts without loading the model. Every mode prints one result per conversation with a passage from the best matching part.

The model is downloaded from Hugging Face on the first index build and cached under `~/.cache/chatgpt-cli/models` (or `$XDG_CACHE_HOME/chatgpt-cli/models`). Transcript and query embeddings run on your machine; their text is not sent to Hugging Face. The model is English-focused, and semantic ranking can still miss a relevant passage. Use `--hybrid` when exact terms matter too.

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
chatgpt search-index
```

`search` automatically adds any current transcripts already in the local cache to the text index. Run `search-index` after `sync` to fetch missing transcripts and refresh embeddings. Changed and deleted chats stop appearing as soon as the local conversation index changes, even before rebuilding search.

The previous ChatGPT search remains available when you want it:

```sh
chatgpt search --remote "garden lighting"
```

## See also

- [CLI reference](../reference/cli.md) for all search flags.
- [Data and model storage](../reference/data.md) for the SQLite and cache paths.
- [How local search works](../explanation/how-it-works.md#searching-locally) for the index design.
