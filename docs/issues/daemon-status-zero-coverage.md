# The daemon's status says "0 of 0" for an index that is full

**Resolved.** `daemon status` counts the search index and the embeddings from the index itself, when it answers, and keeps that count until the index changes. The first answer is the real coverage, and a pass with nothing to do is no longer reported as indexing.

Reported on 2026-10-21, with the released v0.2.1 binary against BK's own index, whose database held 859 chats, 49,806 chunks and 49,806 vectors. Reproduced the same day with that binary against a dev instance holding 3 chats, 4 chunks and 4 vectors.

The daemon's indexer and embedder keep their counters in memory, and set them only when a background pass finishes. A daemon that has just started has counted nothing, so the status answered `search index: 0 of 0 chats indexed, indexing now, 0 transcript(s) fetched` and `embeddings: 0 of 0 chunks embedded, embedding now, checking the embedding model`, where the index held every chat, chunk and vector. The same command reported the truth again once the first pass had counted.

On a dev instance holding 3 chats, 4 chunks and 4 vectors, the released v0.2.1 binary answered `embeddings: 0 of 0 chunks embedded, embedding now, checking the embedding model` for the first status of a fresh daemon, where every chunk had a vector.

Impact: the status lies from the daemon's start until its first pass has counted, because the counters the passes set were the only ones it read before that.

Fix: count the coverage from the index when the status is answered, instead of reading the counters the passes set. The count walks every chunk and every vector, so it is kept until the index changes (`PRAGMA data_version`). `in_progress` now means "a pass is running or about to run *and* there is work to do", so a daemon that has just started doesn't claim to be indexing what is already indexed; the model being fetched again counts as work, which the coverage doesn't show.

Pinned by `the_first_status_reports_the_index_as_it_is` in `crates/cli/tests/daemon_cli.rs`.
