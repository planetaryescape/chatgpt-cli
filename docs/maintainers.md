# Maintainer guide

How to change `chatgpt` safely: where things live, how to verify a change, and which decisions are settled.

## Layout

| Path | Job |
|---|---|
| `src/cli.ts` | Command definitions (commander) and wiring |
| `src/auth/` | Browser session cookies, model keys from env or config |
| `src/api/` | HTTP client (impit, retries) and one function per chatgpt.com endpoint |
| `src/index/` | SQLite: chat index, cached transcripts, summaries, judgments |
| `src/search/` | Transcript chunks, SQLite FTS5 and vectors, local embedding model, search ranking |
| `src/render/` | Conversation tree → markdown |
| `src/classify/` | Jev questions and policy, summarisers, the three-step pipeline, unsure follow-up, Luna review and local titles |
| `src/commands/` | Command behaviour: selection, bulk actions, export, review, stats |
| `src/tui/` | OpenTUI React app |
| `src/progress.ts` | Progress bars and timings on stderr |
| `scripts/gen-cli-reference.ts` | Generates `docs/reference/cli.md` |

## Verify a change

```sh
bunx tsc --noEmit
bun test
```

Tests use temporary SQLite fixtures and never call chatgpt.com, TypeSafe or a summariser. For anything touching the API, rendering or classification, also run the real command:

```sh
bun src/cli.ts sync
bun src/cli.ts list --limit 5
bun src/cli.ts export <link>
bun src/cli.ts classify --title "<a few known chats>"
```

Before a change to the TUI, run `bun test src/tui` (which drives it with simulated keys). Then open `bun src/cli.ts tui` yourself.

## Regenerate docs

```sh
bun run docs:cli
```

This rewrites `docs/reference/cli.md` from the CLI's `--help`. `bun test` fails when that file is out of date, so run this after changing any command, flag or description in `src/cli.ts`.

Search index changes that alter passage text should bump `CHUNK_VERSION` in `src/search/chunks.ts`. Changes to the model, revision, pooling or quantization should change `MODEL_VERSION` in `src/search/embeddings.ts`. Run `bun src/cli.ts search-index`, then check lexical, semantic and hybrid results on the real account; fixture tests do not measure retrieval quality or runtime.

## Change classification

| You changed | Then |
|---|---|
| A threshold in `src/classify/policy.ts` | Nothing to re-run; suggestions are recalculated on read |
| A question, its criteria, or the set of questions | Bump `QUESTIONS_VERSION`; next `classify` re-judges everything |
| A follow-up question or its criteria | Bump `DEEP_QUESTIONS_VERSION`; next `classify` re-asks unsure chats |
| Luna's final-review prompt or result meaning | Bump `LUNA_JUDGMENT_VERSION`; next `classify` re-reviews chats still unsure, every product label, borderline product ideas and conflicting time-expired cases |
| Luna's local-title prompt or result meaning | Bump `LOCAL_TITLE_VERSION`; next `titles` regenerates titles and themes |
| The summary prompt | Bump `SUMMARY_PROMPT_VERSION`; long chats are re-summarised on your subscriptions |
| Transcript rendering | Bump `RENDER_VERSION`; transcripts are re-downloaded |
| A provider's prices | Update `PRICES` in `src/classify/costs.ts` with the new source and date |

Then:

1. Check the change against a labelled handful of real chats. Print titles and scores, not transcripts:

   ```sh
   bun src/cli.ts classify --title '^(Chat A|Chat B|Chat C)$' --pinned -y
   bun src/cli.ts list --title '^(Chat A|Chat B|Chat C)$' --json | jq -r '.[] | "\(.jev.suggestion)\t\(.jev.reason)\t\(.title)"'
   ```

2. Update [Classification reference](reference/classification.md).

## Handle personal data

The database holds private conversations that mention other people.

- When debugging, print ids, titles, counts, scores and token counts. Read transcript text only when the bug is in the text.
- Test destructive actions on a throwaway chat you create for the purpose, or on already-archived chats you restore afterwards.
- Keep cookies, HAR files and access tokens out of the repo. `.gitignore` covers `*.har`.

## Locked decisions

Settle these with the owner before changing them.

- **Auth comes from a local browser session.** Use the macOS default browser unless `--browser` and `--profile` select another source. Do not silently fall back to another browser, which may be logged into a different account. No separate login flow or stored ChatGPT tokens.
- **HTTP goes through `impit`.** Plain fetch and curl are blocked by Cloudflare. Headless Chrome was challenged too; a visible browser works but can't run for every command.
- **Model credentials.** Environment variables take precedence over `~/.config/chatgpt-cli/config.json`. A configured OpenAI or Anthropic key replaces that provider's subscription CLI for model work. Without those keys, summaries try Codex `gpt-6-luna`, then `claude -p --model haiku`.
- **Memory classification.** The saved-memory rubric and version are separate from chat classification; see `docs/reference/memory-classification.md`. Quick decisions can only keep or route to Luna. Every delete suggestion needs Luna review. Do not infer memory staleness from an archived source chat.
- **Brainstorms are always keep.** That includes drafting talks and articles.
- **Faith influences do not make a sermon brainstorm.** Use `sermon` when religious teaching or reflection is the piece's main purpose; broader writing stays `writing` even when faith informs it.
- **Product brainstorms are the user's own concepts or substantial new directions.** Generic coding, settled implementation and work on someone else's product do not qualify by themselves.
- **Re-askable quick help is delete.** The test is whether anything would be lost, not the number of turns.
- **Age alone doesn't affect suggestions.** One-off help may become delete when its useful moment has passed and it leaves no lasting record or artifact. Keep meaningful decisions, personal history, claims, drafts and work to resume even after a deadline or event.
- **A few misses are acceptable.** Don't tune question wording to flip a single chat. Borderline chats surface as `keep?`.

## Out of scope

- Other operating systems for browser-session auth. macOS supports Safari, Chrome, Firefox, Dia, Arc, Brave and Edge.
- Sending messages or starting chats. Verified 2026-09-28: a send without sentinel tokens gets `403 Unusual activity has been detected from your device`, and the tokens need Turnstile and fingerprinting programs that only a real browser runs. See [The chatgpt.com web API](explanation/chatgpt-api.md#sending-messages).
- Shared links (`/share/…`).
- A web UI.

## Commits

`type: description`, with an optional body and no generated attribution footers. Use your own Git identity.
