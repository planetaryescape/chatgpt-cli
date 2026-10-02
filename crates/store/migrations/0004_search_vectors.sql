-- Semantic search's vectors, as the TS CLI's src/search/store.ts @ 1b8c950
-- keeps them: one per chunk, a 384-float little-endian blob, tagged with
-- what made it. A chunk's vector goes with it: chunk ids are rowids that
-- SQLite can hand out again, so a vector left behind would attach itself to
-- an unrelated chunk.
create table search_vectors (
    chunk_id integer primary key,
    model_version text not null,
    embedding blob not null
);

create trigger search_chunks_delete_vectors after delete on search_chunks begin
    delete from search_vectors where chunk_id = old.id;
end;
