# chatgpt-cli

A CLI and TUI for managing ChatGPT history through chatgpt.com's private web API. Start with `docs/maintainers.md`: layout, verification, the classification change procedure, locked decisions, and out-of-scope work.

## Every change

1. `bunx tsc --noEmit` and `bun test` pass. `bun test` fails when `docs/reference/cli.md` is stale; `bun run docs:cli` regenerates it.
2. For anything touching the API, rendering, or classification, run the real command against your own account (`bun src/cli.ts <command>`). Tests only use fixtures; the private API is verified live.
3. Update the page in `docs/` that describes what you changed.
4. Commit as `type: description`, using your own Git identity without agent attribution.

## Private data

Transcripts can contain private conversations about other people. When debugging, print ids, synthetic titles, counts, scores, and turn/token numbers; read transcript text only when the bug is in the text itself. Test destructive actions on a throwaway chat or on already-archived chats you restore. Cookies, HARs, tokens, and real conversation details stay out of git and public issues.

## Before you change

- An API call, or something chatgpt.com returns: read `docs/explanation/chatgpt-api.md`, then re-observe the live site rather than guessing.
- A Jev question, the policy, or the summariser: read `docs/reference/classification.md` and the classification section of `docs/maintainers.md`, and bump the right version constant.
- The TUI: its key handler reads state from the `live` ref, because key repeat outruns React commits. Tests drive it with `testRender`, a temp SQLite fixture, and `settle()`.
- Commands, flags or output formats: update the relevant page in `docs/` and regenerate `docs/reference/cli.md`.
- Terminal output: send status through `note()` in `src/progress.ts`. Reserve `console.error` for real errors, since Bun prints it in red.
