# Semantic search follow-ups

Found while porting semantic, hybrid and remote search and `search-index` (stage 3). None blocks a user journey; each is small.

## Should fix

- ~~**Release binary size.** tract takes the binary from 21.2 MB to 46.3 MB. `strip = true` under `[profile.release]` brings it to 35.0 MB (measured with `strip`). It touches the release build, so it's left for a release-focused change.~~ Fixed in stage 7: `strip = true` takes the arm64 release build from 51.3 MB to 38.9 MB; `--version` (the release smoke test) and `codesign -v` pass.
- ~~**`search-index --browser`.** The fetch uses the daemon's session choice (the last sync's), as the background indexer does, not the `--browser`/`--profile` given to `search-index`. The TS CLI's uses the flags. Fetching with another browser's session could mix accounts, so the fix is to refuse the flags with a clear message rather than honour them.~~ Fixed in stage 7 by honouring them: the request carries the choice, a choice other than the daemon's is checked against the index's account first (another account's is refused), and it stays the daemon's choice, as a `sync`'s does (`semantic_cli::search_index_fetches_with_the_chosen_browser`).
- ~~**`search-index` failure lines.** A chat whose batch ChatGPT answered with an error is reported as `<id> <title>: not returned by ChatGPT; run sync.`, where the TS CLI prints the error. The indexer keeps only the ids it set aside; keeping the reason with each would let the message say why.~~ Fixed in stage 7: each set-aside chat keeps its reason (an error's status and path, or not returned), never a body (`semantic_cli::search_index_failures_say_why_without_the_body`).

## Nits

- ~~**A remote hit without `payload.is_archived`** counts as not archived. The TS CLI's JSON would leave the `archived` key out for it. ChatGPT always sent the field in testing.~~ Fixed in stage 7: such a hit takes its chat's archive state from the index, or counts as active for a chat the index doesn't have (`remote::tests::a_hit_without_archive_state_takes_the_indexs`).
- **Remote errors** say `403 from /backend-api/global/search` without the body the TS CLI appends. That's the daemon's no-bodies rule, kept on purpose.
- ~~**Chunks the model fails on** are skipped until the daemon restarts (an in-memory set). None failed on BK's 43,755 chunks.~~ Fixed in stage 7: failures live in `search_vector_failures` (migration 8) and are retried after a day, across restarts (`vector_tests::a_failed_chunk_waits_until_its_retry_time`).
- ~~**Equal semantic scores** keep SQLite's row order, as the TS CLI's do. Two indexes with different chunk ids could order exact ties differently; the parity harness gives both the same ids. Real vectors don't tie.~~ Fixed in stage 7: ties go to the smaller chat id, and within a chat to the earlier chunk (`semantic::tests::equal_scores_rank_by_chat_id_and_chunk_order`). The semantic and hybrid goldens were regenerated for it.
- ~~**`search --format table` with a cut emoji:** the cell width of a lone surrogate (`Bun.stringWidth`) wasn't compared; the cut shows as U+FFFD in both.~~ Gone after 0.1.5: snippets never cut an emoji.
- **The embedder recounts** chunks and vectors at every page of 64 (about 30 ms on 43,755 chunks, roughly 1% of embedding time), to keep `daemon status` right while the indexer adds chunks. Kept in stage 7: at about 1% of embedding time it isn't worth a cache that must track the indexer's writes.
