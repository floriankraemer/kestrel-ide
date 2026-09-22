-- IntrospectLevel::Full: one table's own primary-key/unique/foreign-key
-- constraints, plus a foreign key's referenced schema/table/columns and
-- its ON UPDATE/ON DELETE action — a caller never needs a second round
-- trip to resolve the reference. `CHECK` constraints are fetched
-- separately (`mysql_checks.sql`): `information_schema.check_constraints`
-- carries no column list of its own to join against here. ? = schema
-- name, ? = table name.
SELECT tc.constraint_name,
       tc.constraint_type,
       GROUP_CONCAT(kcu.column_name ORDER BY kcu.ordinal_position SEPARATOR ',') AS columns,
       MAX(kcu.referenced_table_schema) AS ref_schema,
       MAX(kcu.referenced_table_name) AS ref_table,
       GROUP_CONCAT(kcu.referenced_column_name ORDER BY kcu.ordinal_position SEPARATOR ',') AS ref_columns,
       MAX(rc.update_rule) AS on_update,
       MAX(rc.delete_rule) AS on_delete
FROM information_schema.table_constraints tc
JOIN information_schema.key_column_usage kcu
  ON kcu.constraint_schema = tc.constraint_schema
 AND kcu.constraint_name = tc.constraint_name
 AND kcu.table_name = tc.table_name
LEFT JOIN information_schema.referential_constraints rc
  ON rc.constraint_schema = tc.constraint_schema
 AND rc.constraint_name = tc.constraint_name
WHERE tc.table_schema = ?
  AND tc.table_name = ?
  AND tc.constraint_type IN ('PRIMARY KEY', 'UNIQUE', 'FOREIGN KEY')
GROUP BY tc.constraint_name, tc.constraint_type
ORDER BY tc.constraint_name;
