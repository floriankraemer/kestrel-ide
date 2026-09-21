-- IntrospectLevel::Columns/Full: one table's columns, in declaration
-- order. $1 = schema name, $2 = table name.
SELECT a.attname AS column_name,
       pg_catalog.format_type(a.atttypid, a.atttypmod) AS type_name,
       NOT a.attnotnull AS nullable
FROM pg_catalog.pg_attribute a
JOIN pg_catalog.pg_class c ON c.oid = a.attrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = $1
  AND c.relname = $2
  AND a.attnum > 0
  AND NOT a.attisdropped
ORDER BY a.attnum;
