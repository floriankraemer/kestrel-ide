-- IntrospectLevel::Columns/Full: one table's columns, in declaration
-- order. ? = schema (database) name, ? = table name.
SELECT column_name,
       column_type,
       is_nullable,
       column_key,
       extra,
       column_default,
       column_comment
FROM information_schema.columns
WHERE table_schema = ?
  AND table_name = ?
ORDER BY ordinal_position;
