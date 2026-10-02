-- Rows the daemon wrote itself (a `title`, a judgment from the Jev guard
-- behind `archive`/`delete --check`), so the import from the TS index
-- (src/legacy.rs) doesn't drop or overwrite them: the TS index never has
-- them. `written_at` is the row's own stamp (`updated_at` or
-- `classified_at`, ISO 8601); a newer row the TS CLI writes later wins.
-- Bridge-only, like the import itself.
create table native_rows (
    tbl text not null,
    id text not null,
    written_at text not null,
    primary key (tbl, id)
);
