-- IntrospectLevel::Names: schemas + tables/views/materialized views in one
-- round trip, so this stays a cheap query even on a 5 000-table catalog
-- (database-tools.md §6's NFR).
SELECT n.nspname AS schema_name,
       c.relname AS object_name,
       c.relkind AS object_kind
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r', 'v', 'm')
  AND n.nspname NOT IN ('pg_catalog', 'information_schema')
ORDER BY n.nspname, c.relname;
