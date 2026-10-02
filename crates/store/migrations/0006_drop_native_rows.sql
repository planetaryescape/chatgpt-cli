-- The import from the TS CLI's index is gone with the TS CLI, and with it
-- the only reader of `native_rows` (0005): the rows the daemon wrote
-- itself, which that import had to leave alone.
drop table if exists native_rows;
