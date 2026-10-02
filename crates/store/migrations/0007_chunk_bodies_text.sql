-- Chunk bodies are read as UTF-8 text from here on. Only chunks from
-- before chunk version 2 can hold anything else (half an emoji, in the
-- bytes Bun gave SQLite for it), and those are stale: the indexer rebuilds
-- them from the cached transcripts. Dropping them leaves every body valid
-- UTF-8. The triggers take their FTS rows and vectors with them.
delete from search_chunks where chunk_version < 2;
delete from search_indexed where chunk_version < 2;
