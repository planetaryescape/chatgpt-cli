# Follow-ups from native classification

Recorded on 2026-10-02 for stage 5 of the Rust port (`configure`, `classify`, `titles`, `memory classify`, summaries in the Jev guard, and Jev in the background). Each is left as is for now; "verified" means observed or read, not reproduced in a user journey.

## Deliberate differences from the TS CLI

- A failing `codex` or `claude -p` is reported by how it exited (`codex exited 1`, `claude -p exited 1 without a JSON answer`, `gpt-6-luna exited 1`); the TS CLI adds the last 300 characters of its stderr, which can quote the chat. An OpenAI answer that isn't `completed` says `OpenAI response <status>: no completed output` without the API's own message, and an HTTP error says `<host> returned <status>` without the body. No bodies, by rule.
- `codex` and `claude` run with only `HOME`, `USER`, `LOGNAME`, `TMPDIR`, the locale, `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `XDG_CONFIG_HOME` and the client's `PATH`; the TS CLI passes its whole environment, API keys included. They're killed (their whole process group) after 10 minutes; the TS CLI waits forever. The model APIs get 10 minutes too, and one attempt, as in the TS CLI.
- `memory classify --limit` is checked before any paid call; the TS CLI checks it after classifying.
- At a terminal, a step opens with its label (`Deep-classifying…`) until its first count, where the TS CLI draws its bar at zero, and `failed:` lines aren't red. Plain (piped) output is identical. The parity harness compares the terminal transcript without step lines.
- Luna's reason is cut at 250 UTF-16 units without splitting a character; the TS CLI can leave half a surrogate pair.
- A saved memory's Luna review re-stamps `classified_at` on a row that came from the cache; the TS CLI keeps the quick pass's stamp. Nothing reads it.
- The follow-up (`DeepClassifier`) asks before summarising more than 500k tokens, and says so in a note; the TS CLI's follow-up summarises without asking.
- A first pass saved again for the same chat and question version (`classify --redo`, a time-bound refresh, a race) keeps the existing follow-up and Luna review; the TS CLI's `saveJudgment` drops them. A redo still re-asks both (`--redo` forces them).
- The Jev guard holds back a chat whose `update_time` changed while it was judged, even when the cache reconcile moved its judgment forward (unchanged content). Safe; may hold back a chat that a second `--check` would approve.

## Residual risks

- ~~Every save takes the sync pass lock briefly. While the bridge exists a pass can include the TS CLI's sync (up to 10 minutes), so a save, and the step waiting on it, can stall that long. Disappears with the TS sync at stage 6.~~ Gone with the TS sync: a pass is the daemon's own sync now.
- ~~The background Jev's "new since enabled" baseline compares `update_time` as text, as sync does; the TS index's mixed millisecond and microsecond formats can order two times in the same millisecond wrongly. At worst a chat is judged one pass late or one more chat is judged.~~ Fixed after 0.2.0: the query compares the times, not the text (`chats_new_since_the_background_jev_baseline_compare_by_time_not_text`).
- ~~`codex`'s process group is killed after the run is reaped; if the group is already empty and its id reused in that instant, another group gets the signal. macOS hands out PIDs in sequence, so reuse needs a wrap-around within microseconds.~~ Fixed after 0.2.0: the group is killed once the leader has exited but before it's reaped, while its PID still reserves the group's ID.
- ~~The bridged `review` and `tui` read the TS index's own verdicts, made at the installed TS CLI's versions, which `classify` no longer refreshes. Stage 6 ports both.~~ Resolved in 0.1.5: both are native and read the daemon's verdicts.
- ~~Judgments imported earlier from the TS index (at the private versions) stay in the daemon's index, stale. The background Jev doesn't judge the history it found when first enabled: a full `chatgpt classify` does (about $0.0002 of Jev per chat, plus summaries and Luna on the subscription).~~ Decided after 0.2.0: left as they are, no cleanup migration. Every read filters by this build's versions, so they're never shown or counted; the full `classify` of 2026-10-02 re-judged the active set; and deleting them would tie a migration to one build's version constants for a few kilobytes.
