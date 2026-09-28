# Data, files and credentials

Where `chatgpt` keeps data, which credentials it reads, and what it sends to which service.

## Files

| Path | Contents |
|---|---|
| `~/.local/share/chatgpt-cli/index.db` | SQLite database: the chat index, cached transcripts, search index and embeddings, summaries and Jev judgments. Set `XDG_DATA_HOME` to move it to `$XDG_DATA_HOME/chatgpt-cli/index.db` |
| `~/.cache/chatgpt-cli/models` | Downloaded local embedding model. Set `XDG_CACHE_HOME` to move it to `$XDG_CACHE_HOME/chatgpt-cli/models` |
| `~/.config/chatgpt-cli/config.json` | Optional Jev, OpenAI, and Anthropic API keys saved by `chatgpt configure`, owner-readable only. Set `XDG_CONFIG_HOME` to move it |

Delete the database to start over; `chatgpt sync`, `chatgpt search-index` and `chatgpt classify` rebuild their respective data.

`chatgpt memory list` and `memory summary` read ChatGPT live. `memory classify` also reads live entries, then stores model judgments and reasons in `index.db`; it does not cache saved-memory text there.

### Tables

| Table | One row per | Key columns |
|---|---|---|
| `conversations` | chat | `id`, `title`, `create_time`, `update_time`, `is_archived`, `pinned`, `project_id` |
| `transcripts` | chat | `update_time` and `render_version` at the time it was rendered; `markdown`, `turns`, `approx_tokens` |
| `summaries` | long chat | `update_time`, `prompt_version`, `summary`, `model` (`gpt-6-luna`, `claude-haiku`, …) |
| `judgments` | chat | `update_time`, `version` (the questions version), `content_kind` (`full` or `summary`), `answers` (Jev's raw JSON), generated `topic` |
| `deep_judgments` | unsure chat | `update_time`, regular `questions_version`, follow-up `version`, `answers` (Jev's raw follow-up JSON) |
| `luna_judgments` | chat still unsure after Jev, with a product label or borderline product idea, or with conflicting evidence of an expired purpose | `update_time`, regular and deep versions, prompt version, final suggestion, brainstorm kind and reason |
| `memory_judgments` | saved memory classified | id, input hash, rubric version, Jev answers, optional Luna decision and reason, classification time |
| `local_titles` | chat | locally generated or manual `title`, open-ended `theme`, source, update time and prompt version; ChatGPT's title stays in `conversations` |
| `search_chunks` | transcript passage | chat id, update time, render and chunk versions, title and body |
| `search_fts` | transcript passage | SQLite FTS5 index of title and body, maintained by triggers |
| `search_indexed` | chat | update time and versions of the completed text index |
| `search_vectors` | passage | embedding model version and 384-dimensional float vector |
| `meta` | setting | `synced_at` |

A cached row counts only while its `update_time` matches the chat's and its version matches the code's constant. Stale rows are ignored, then overwritten.

Inspect it directly:

```sh
sqlite3 ~/.local/share/chatgpt-cli/index.db "select model, count(*) from summaries group by model"
```

## Environment variables

| Variable | Used for | Default |
|---|---|---|
| `TYPESAFE_API_KEY` | Jev, during `classify` | configured `jev` key; otherwise required |
| `OPENAI_API_KEY` | Luna review, local titles, and first summary provider | configured key, then Codex CLI if absent |
| `ANTHROPIC_API_KEY` | summary fallback | configured key, then Claude CLI if absent |
| `XDG_CONFIG_HOME` | where `config.json` lives | `~/.config` |
| `XDG_DATA_HOME` | where `index.db` lives | `~/.local/share` |
| `XDG_CACHE_HOME` | where the embedding model is cached | `~/.cache` |
| `PAGER` | `v` (view) in `chatgpt review` | `less` |

## Credentials

| Credential | Source | Needed by |
|---|---|---|
| ChatGPT session | Dia's Personal profile cookie store (`~/Library/Application Support/Dia/User Data/Default/Cookies`), decrypted with the Keychain entry "Dia Safe Storage" | every command that talks to ChatGPT |
| ChatGPT access token | exchanged from the session at `/api/auth/session` on each run, kept in memory only | same |
| TypeSafe key | `TYPESAFE_API_KEY` or configured `jev` key | chat and memory classification, `--check` |
| OpenAI key or Codex login | `OPENAI_API_KEY`, configured `openai` key, then existing `codex` login | summarising long chats, Luna review and local titles |
| Anthropic key or Claude login | `ANTHROPIC_API_KEY`, configured `anthropic` key, then existing `claude` login | summary fallback |

`chatgpt configure` writes only the API keys you enter to `config.json` with mode `0600`. The ChatGPT session is read fresh from Dia on each run, so logging out of ChatGPT in Dia also logs `chatgpt` out.

## What goes where

| Destination | What's sent |
|---|---|
| chatgpt.com | API requests made with your session: listing, reading, searching, archiving, deleting and renaming chats; listing, reading and deleting saved memories |
| Hugging Face | The embedding model files are downloaded on first use; transcript and query text stay on the local machine |
| TypeSafe (Jev) | Chat classification gets title, dates, turn count, project and pinned flags, and transcript or summary. Memory classification gets one saved-memory entry and up to three related entries |
| Codex or OpenAI API (`gpt-6-luna`) | Long-chat summaries, final chat review, local titles and themes; memory entries needing deeper review |
| Claude CLI or Anthropic API (Haiku) | Long-chat summaries when the first provider fails |

Review the providers and their data terms before classifying private conversations or saved memories.
