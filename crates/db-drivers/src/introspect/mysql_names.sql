-- IntrospectLevel::Names: every database (MySQL's own "schema") and its
-- tables/views in one round trip, so this stays a cheap query even on a
-- 5 000-table catalog (database-tools.md §6's NFR) — the built-in
-- `mysql`/`information_schema`/`performance_schema`/`sys` databases are
-- never a user's own schema, so they are excluded the same way Postgres's
-- own fixture excludes `pg_catalog`/`information_schema`.
SELECT t.table_schema AS schema_name,
       t.table_name AS object_name,
       t.table_type AS object_kind
FROM information_schema.tables t
WHERE t.table_schema NOT IN ('mysql', 'information_schema', 'performance_schema', 'sys')
ORDER BY t.table_schema, t.table_name;
