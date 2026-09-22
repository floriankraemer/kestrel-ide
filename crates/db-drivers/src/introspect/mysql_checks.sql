-- IntrospectLevel::Full: one table's own CHECK constraints (MySQL 8.0.16+
-- and MariaDB both expose these; older MySQL simply returns no rows,
-- which this driver treats the same as "no check constraints" rather
-- than an error). ? = schema name, ? = table name.
SELECT cc.constraint_name,
       cc.check_clause
FROM information_schema.check_constraints cc
JOIN information_schema.table_constraints tc
  ON tc.constraint_schema = cc.constraint_schema
 AND tc.constraint_name = cc.constraint_name
WHERE tc.table_schema = ?
  AND tc.table_name = ?
  AND tc.constraint_type = 'CHECK';
