# Semantic search follow-ups

Found while porting semantic, hybrid and remote search and `search-index` (stage 3). None blocks a user journey; each is small.

## Should fix

- **Release binary size.** tract takes the binary from 21.2 MB to 46.3 MB. `strip = true` under `[profile.release]` brings it to 35.0 MB (measured with `strip`). It touches the release build, so it's left for a release-focused change.
- **`search-index --browser`.** The fetch uses the daemon's session choice (the last sync's), as the background indexer does, not the `--browser`/`--profile` given to `search-index`. The TS CLI's uses the flags. Fetching with another browser's session could mix accounts, so the fix is to refuse the flags with a clear message rather than honour them.
- **`search-index` failure lines.** A chat whose batch ChatGPT answered with an error is reported as `<id> <title>: not returned by ChatGPT; run sync.`, where the TS CLI prints the error. The indexer keeps only the ids it set aside; keeping the reason with each would let the message say why.

## Nits

- **A remote hit without `payload.is_archived`** counts as not archived. The TS CLI's JSON would leave the `archived` key out for it. ChatGPT always sent the field in testing.
- **Remote errors** say `403 from /backend-api/global/search` without the body the TS CLI appends. That's the daemon's no-bodies rule, kept on purpose.
- **Chunks the model fails on** are skipped until the daemon restarts (an in-memory set). None failed on BK's 43,755 chunks.
- **Equal semantic scores** keep SQLite's row order, as the TS CLI's do. Two indexes with different chunk ids could order exact ties differently; the parity harness gives both the same ids. Real vectors don't tie.
- ~~**`search --format table` with a cut emoji:** the cell width of a lone surrogate (`Bun.stringWidth`) wasn't compared; the cut shows as U+FFFD in both.~~ Gone after 0.1.5: snippets never cut an emoji.
- **The embedder recounts** chunks and vectors at every page of 64 (about 30 ms on 43,755 chunks, roughly 1% of embedding time), to keep `daemon status` right while the indexer adds chunks.
