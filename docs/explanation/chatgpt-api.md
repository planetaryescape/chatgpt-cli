# The chatgpt.com web API

What chatgpt.com's private API does, as observed on 2026-09-27, 2026-09-28 and 2026-09-29, and how to re-observe it when something changes. OpenAI doesn't document or version this API, so treat every line here as a dated observation.

## Endpoints in use

| Purpose | Request | Notes |
|---|---|---|
| Access token | `GET /api/auth/session` | Needs the session cookies; returns `accessToken` |
| List chats | `GET /backend-api/conversations?offset&limit&order=updated&is_archived&hide_snorlax=false` | `limit=100` works. `hide_snorlax=false` includes project chats |
| One chat | `GET /backend-api/conversation/{id}` | Full message tree, including `default_model_slug` |
| Many chats | `POST /backend-api/conversations/batch` `{conversation_ids}` | At most 10 ids; full trees |
| Search | `POST /backend-api/global/search` | One hit per matching message; `payload.conversation_id`; `limit` is at most 40 |
| List saved memories | `GET /backend-api/memories?include_memory_entries=true` | Returns `memories[]` with ids, content, status, timestamps and optional source chat ids |
| Memory summary | `POST /backend-api/memories/about_you/summary?source=personalization-setting` `{}` | JSON fallback for the UI's streamed summary; returns titled sections |
| Delete saved memory | `DELETE /backend-api/memories/{memory_id}` | Returns `{success: true}` after the UI's confirmation |
| List projects | `GET /backend-api/gizmos/snorlax/sidebar?conversations_per_gizmo=0&limit=20&owned_only=false` | `items` contain project ids, names, write permission and archive state; `cursor` pages results |
| Create project | `POST /backend-api/projects` `{emoji: null, instructions: "", memory_scope: "unset", name, theme: null}` | Observed in the ChatGPT UI on 2026-09-29; returns `resource.gizmo` with the new project id and display name |
| Delete project | `DELETE /backend-api/gizmos/{id}` | Observed 2026-10-02 on an empty project; returns `{"deleted": true}`, and the project leaves the sidebar list. `project delete` sends it once and counts it only on `deleted: true`; a 404 means no such project |
| Move into project | `PATCH /backend-api/conversation/{id}` `{gizmo_id: "g-p-…"}` | Returns `{success: true}`; observed through the ChatGPT UI and replayed by the CLI |
| Remove from project | `PATCH /backend-api/conversation/{id}` `{gizmo_id: ""}` | Observed through the ChatGPT UI; the CLI confirms legacy 500 responses by reading the chat |
| Archive / unarchive | `PATCH /backend-api/conversation/{id}` `{is_archived}` | |
| Rename | `POST /backend-api/conversation/id/{id}/rename` `{title}` | |
| Delete | `DELETE /backend-api/conversation/id/{id}` | Later reads return `404 conversation_deleted` |

Every call needs `Authorization: Bearer <token>` and the session cookies.

## Observed behaviour

- **Cloudflare.** Plain `fetch` and `curl` get `403` with `cf-mitigated: challenge`, because the check is on the TLS and HTTP/2 fingerprint. `impit`, impersonating Chrome, passes. About 1 in 7 fresh connections is still challenged, and a new connection usually passes. On 2026-10-01, Bun and Node `fetch` and macOS `curl` were still challenged, but no fresh connection was: 50 of 50 through the Rust impit client and 20 of 20 through npm `impit` passed, and so did plain Rust `reqwest` (rustls) with or without the `cf_clearance` cookie. The challenge rate drifts, so keep retrying challenges on a fresh connection.
- **Rate limits.** Repeated single-chat `GET`s hit `429` within a few hundred calls, with no `retry-after`. It clears after about a minute. The batch endpoint handled 100 chats in 43 seconds with no 429s.
- **List paging.** `total` is only `offset + items + 1`, a "there's more" hint, so page until a page comes back short. `order=created` returns 500. Ordering is by `update_time`, so a chat updated while you page moves to the top, shifts later pages and can be skipped. Dedupe by id, then re-read the top.
- **update_time.** New messages and renames change it; archiving and unarchiving don't. A delta sync therefore can't see archive changes from `update_time` alone.
- **Batch quirks.** Unknown ids are silently left out, but some deleted chats still appear in batch results after the single-chat endpoint returns `404`. A batch result cannot prove that a chat still exists. Items use `id` rather than `conversation_id`, ISO timestamps rather than epoch seconds, and have no `default_model_slug`. For some legacy chats, the batch `update_time` is old or rounded even when the conversation list has a newer timestamp. Cache reconciliation compares rendered content and title, then uses the list timestamp.
- **Incomplete archived lists.** On 2026-09-28, the archived list omitted chats that individual reads confirmed still existed. A full sync now checks every previously indexed chat missing from both lists through the single-chat endpoint before dropping it. The delta sync checks archived drop-outs with the batch endpoint; a deleted tombstone may remain in the local index until a full sync.
- **Rename on older chats.** Chats created before 2025 return `500` to rename, but the sidebar title changes anyway. The detail and batch endpoints keep showing the old title.
- **Project moves.** The ChatGPT UI sends `gizmo_id: ""` to remove a chat from a project. Moving a chat updates its `gizmo_id` in the detail response and can change its `update_time` in conversation listings. Some older chats returned `500` after the move still applied; the CLI reads the chat to confirm those cases. The CLI updates local `project_id` after a confirmed move; `sync` refreshes the timestamp and checks cached content before preserving derived data.
- **Memory experiences.** On 2026-09-28, Personalization opened a generated Memory summary through `POST /memories/about_you/summary/stream` and the Saved memories view used `include_memory_entries=true`. The CLI uses the JSON summary fallback observed live. Deleting a disposable saved memory through the UI returned `{success: true}`; deleting its source chat was a separate request. The Saved memories view offered per-item deletion. Its page bundle references a separate streamed summary-correction endpoint, which was not exercised.

## Message shapes

A conversation is `mapping` (node id → `{message, parent, children}`) plus `current_node`, the leaf of the thread shown in the UI.

| Shape | Where | Rendered as |
|---|---|---|
| Plain text | `content.parts[]` strings, `content_type: text` | the text |
| Voice | parts with `content_type: audio_transcription`, text in `.text` | the text |
| Image | parts with `content_type: image_asset_pointer` | `[image]` |
| Generated image | a `tool`-role `multimodal_text` message holding an image | `[generated image]` |
| File | `metadata.attachments[]` (`mime_type` can be null) | `[attached file: name]` |
| Web citation | private-use characters in text (`citeturn0search1`), listed in `metadata.content_references` with display text in `alt` | the `alt` link |
| Canvas | assistant messages with `recipient: canmore.create_textdoc` (JSON with `name`, `content`) and `canmore.update_textdoc` (regex `updates`) | final document |
| Reasoning, tool calls, search results | `thoughts`, `reasoning_recap`, `code`, tool-role messages | omitted |

One trap: some `content_references` have `matched_text` of a single space. Replacing those globally deletes every space in the message, so references are applied in order, one at a time, and whitespace-only ones are skipped.

## Sending messages

Not supported, and observed on 2026-09-28 to be deliberately hard:

1. `POST /backend-api/sentinel/chat-requirements/prepare`. The body is `{p}`, a value the page generates. The response demands three proofs, all `required: true`:
   - `proofofwork`: a `seed` and `difficulty` (hashing)
   - `turnstile`: `dx`, a ~29 KB program for Cloudflare Turnstile's VM
   - `so`: `collector_dx` and `snapshot_dx`, ~33 KB and ~26 KB programs that fingerprint the browser
2. `POST …/chat-requirements/finalize` with the answers returns a token valid for 540 seconds.
3. `POST /backend-api/f/conversation/prepare` returns a `conduit_token`.
4. `POST /backend-api/f/conversation` streams the reply as server-sent events. It carries `OpenAI-Sentinel-Chat-Requirements-Token`, `OpenAI-Sentinel-Proof-Token`, `OpenAI-Sentinel-Turnstile-Token` and `x-conduit-token`.

Posting straight to `f/conversation` without them returns `403 {"detail":"Unusual activity has been detected from your device. Try again later."}`. The only workable route is driving a real, visible browser. `codex exec` is the supported way to talk to the model from a terminal, but its threads don't appear in chatgpt.com.

## When it breaks

Re-observe rather than guess:

1. Open chatgpt.com in a real browser with network recording. The agent-browser skill's `derive-client` flow records a HAR while you use the site.
2. Do the action that broke (list, open a chat, archive…) and find the request in the HAR.
3. Compare its URL, body and response with the table above and with `crates/daemon/src/api.rs` (writes in `api/writes.rs`).
4. Delete the HAR afterwards: it contains your session cookies and token.

A headed browser passes Cloudflare where a headless one gets challenged.
