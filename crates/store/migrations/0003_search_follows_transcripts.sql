-- Search chunks count as current only while they were built from the
-- transcript the cache holds now. Any write that brings new transcript
-- content (the TS import, the indexer) drops the chat's chunks in the same
-- transaction, so the reconcile, which moves chunks forward with a
-- transcript it verified, can never vouch for chunks of other content.
-- Moving `update_time` alone (the reconcile, the import's time-only fix)
-- keeps them.
create trigger transcripts_insert_unindexes after insert on transcripts begin
    delete from search_chunks where conversation_id = new.id;
    delete from search_indexed where conversation_id = new.id;
end;

create trigger transcripts_update_unindexes after update of markdown on transcripts
when old.markdown is not new.markdown begin
    delete from search_chunks where conversation_id = new.id;
    delete from search_indexed where conversation_id = new.id;
end;
