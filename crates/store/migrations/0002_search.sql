-- The lexical search index, as the TS CLI's src/search/store.ts @ 1b8c950
-- builds it: transcript chunks, which chats they're current for, and an
-- external-content FTS5 table over them kept in step by triggers. The
-- TS CLI's search_vectors table and its trigger clause belong to semantic
-- search and are left out until it's ported.
create table search_chunks (
    id integer primary key,
    conversation_id text not null,
    update_time text not null,
    render_version integer not null,
    chunk_version integer not null,
    chunk_index integer not null,
    title text not null,
    body text not null,
    unique (conversation_id, chunk_index)
);

create index search_chunks_conversation on search_chunks(conversation_id);

create table search_indexed (
    conversation_id text primary key,
    update_time text not null,
    render_version integer not null,
    chunk_version integer not null
);

create virtual table search_fts using fts5(
    title, body,
    content = 'search_chunks', content_rowid = 'id',
    tokenize = 'porter unicode61 remove_diacritics 2'
);

create trigger search_chunks_insert after insert on search_chunks begin
    insert into search_fts(rowid, title, body) values (new.id, new.title, new.body);
end;

create trigger search_chunks_delete after delete on search_chunks begin
    insert into search_fts(search_fts, rowid, title, body) values ('delete', old.id, old.title, old.body);
end;
