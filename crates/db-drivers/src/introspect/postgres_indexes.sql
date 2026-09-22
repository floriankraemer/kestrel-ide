-- IntrospectLevel::Full: one table's own indexes — name, columns in
-- declaration order, uniqueness and access method (`btree`, `hash`, …).
-- $1 = schema name, $2 = table name.
SELECT ic.relname AS index_name,
       i.indisunique AS is_unique,
       am.amname AS method,
       array(
         SELECT a.attname FROM pg_catalog.pg_attribute a
         WHERE a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
         ORDER BY array_position(i.indkey, a.attnum)
       ) AS columns
FROM pg_catalog.pg_index i
JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
JOIN pg_catalog.pg_class c ON c.oid = i.indrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
JOIN pg_catalog.pg_am am ON am.oid = ic.relam
WHERE n.nspname = $1
  AND c.relname = $2
ORDER BY ic.relname;
