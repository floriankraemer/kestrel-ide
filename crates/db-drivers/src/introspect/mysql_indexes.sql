-- IntrospectLevel::Full: one table's own indexes — name, uniqueness,
-- access method and columns in declaration order. `GROUP_CONCAT` rather
-- than `JSON_ARRAYAGG` (MariaDB 11 and MySQL 8.4 both support the latter,
-- but the former needs no JSON parsing back in Rust for the common case
-- of plain column names, which never contain a comma) — ? = schema name,
-- ? = table name.
SELECT index_name,
       MAX(non_unique) = 0 AS is_unique,
       MAX(index_type) AS method,
       GROUP_CONCAT(column_name ORDER BY seq_in_index SEPARATOR ',') AS columns
FROM information_schema.statistics
WHERE table_schema = ?
  AND table_name = ?
GROUP BY index_name
ORDER BY index_name;
