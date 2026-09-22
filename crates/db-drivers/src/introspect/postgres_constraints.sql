-- IntrospectLevel::Full: one table's own constraints (primary key,
-- unique, foreign key, check) — a foreign key's referenced schema/table/
-- columns and its ON UPDATE/ON DELETE action codes come along so a
-- caller never needs a second round trip to resolve the reference.
-- $1 = schema name, $2 = table name.
SELECT con.conname AS constraint_name,
       con.contype AS constraint_type,
       array(
         SELECT attname FROM pg_catalog.pg_attribute
         WHERE attrelid = con.conrelid AND attnum = ANY(con.conkey)
         ORDER BY array_position(con.conkey, attnum)
       ) AS columns,
       fn.nspname AS ref_schema,
       fc.relname AS ref_table,
       array(
         SELECT attname FROM pg_catalog.pg_attribute
         WHERE attrelid = con.confrelid AND attnum = ANY(con.confkey)
         ORDER BY array_position(con.confkey, attnum)
       ) AS ref_columns,
       con.confupdtype AS on_update,
       con.confdeltype AS on_delete,
       pg_catalog.pg_get_constraintdef(con.oid) AS definition
FROM pg_catalog.pg_constraint con
JOIN pg_catalog.pg_class c ON c.oid = con.conrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_class fc ON fc.oid = con.confrelid
LEFT JOIN pg_catalog.pg_namespace fn ON fn.oid = fc.relnamespace
WHERE n.nspname = $1
  AND c.relname = $2
ORDER BY con.conname;
