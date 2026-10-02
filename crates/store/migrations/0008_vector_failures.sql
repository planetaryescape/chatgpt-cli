-- Chunks the embedding model failed on, so the embedder skips them until
-- `retry_at` (unix seconds) instead of until the daemon restarts. A
-- failure goes with its chunk, as a vector does: chunk ids are rowids that
-- SQLite can hand out again.
create table search_vector_failures (
    chunk_id integer primary key,
    model_version text not null,
    retry_at integer not null
);

create trigger search_chunks_delete_vector_failures after delete on search_chunks begin
    delete from search_vector_failures where chunk_id = old.id;
end;
