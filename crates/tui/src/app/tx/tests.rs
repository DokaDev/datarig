use super::{DdlLock, ddl_lock};

#[test]
fn ddl_locks_are_said_only_where_the_documentation_gives_them() {
    use DdlLock::*;
    let ex = |n: &str| Some(Exclusive(n.to_string()));
    let kept = |n: &str| Some(Kept(n.to_string()));
    for (sql, want) in [
        ("ALTER TABLE shop.users ADD COLUMN zz int", ex("shop.users")),
        ("ALTER TABLE shop.users ADD zz int", ex("shop.users")),
        ("ALTER TABLE IF EXISTS ONLY shop.users DROP COLUMN zz", ex("shop.users")),
        ("ALTER TABLE t DROP CONSTRAINT c", ex("t")),
        ("ALTER TABLE t ALTER COLUMN a TYPE bigint", ex("t")),
        ("alter table t alter a set data type bigint", ex("t")),
        ("ALTER TABLE t ALTER COLUMN a SET NOT NULL", ex("t")),
        ("ALTER TABLE t ALTER COLUMN a DROP NOT NULL", ex("t")),
        ("ALTER TABLE t RENAME TO u", ex("t")),
        ("ALTER TABLE t RENAME COLUMN a TO b", ex("t")),
        ("ALTER TABLE t OWNER TO me", ex("t")),
        ("ALTER TABLE t SET TABLESPACE fast", ex("t")),
        ("ALTER TABLE t SET UNLOGGED", ex("t")),
        ("ALTER TABLE p DETACH PARTITION p1", ex("p")),
        ("ALTER TABLE \"Mixed\".\"T\" ADD c int", ex("\"Mixed\".\"T\"")),
        ("DROP TABLE IF EXISTS shop.reviews", ex("shop.reviews")),
        ("DROP VIEW v", ex("v")),
        ("DROP MATERIALIZED VIEW mv", ex("mv")),
        ("DROP SEQUENCE s", ex("s")),
        ("TRUNCATE shop.orders", ex("shop.orders")),
        ("TRUNCATE TABLE ONLY shop.orders", ex("shop.orders")),
        ("DROP INDEX shop.users_name_idx", Some(DropIndex("shop.users_name_idx".into()))),
        ("CREATE INDEX i ON shop.users (name)", Some(Share("shop.users".into()))),
        ("CREATE UNIQUE INDEX i ON ONLY t (a)", Some(Share("t".into()))),
        // Other modes, or not one action: a lock is kept, which one is not said.
        ("ALTER TABLE t ADD CONSTRAINT fk FOREIGN KEY (a) REFERENCES u", kept("t")),
        ("ALTER TABLE t ADD PRIMARY KEY (a)", kept("t")),
        ("ALTER TABLE t VALIDATE CONSTRAINT c", kept("t")),
        ("ALTER TABLE t SET (fillfactor = 70)", kept("t")),
        ("ALTER TABLE t ALTER COLUMN a SET STATISTICS 100", kept("t")),
        ("ALTER TABLE t ALTER COLUMN a SET DEFAULT 1", kept("t")),
        ("ALTER TABLE t ENABLE TRIGGER x", kept("t")),
        ("ALTER TABLE p ATTACH PARTITION p1 FOR VALUES IN (1)", kept("p")),
        ("ALTER TABLE p DETACH PARTITION p1 FINALIZE", kept("p")),
        ("ALTER TABLE t ADD COLUMN a int, ADD COLUMN b int", kept("t")),
        ("ALTER INDEX i RENAME TO j", kept("i")),
        ("ALTER VIEW v RENAME TO w", kept("v")),
        ("ALTER MATERIALIZED VIEW mv RENAME TO m2", kept("mv")),
        // No note.
        ("ALTER TABLE ALL IN TABLESPACE a SET TABLESPACE b", None),
        ("CREATE TABLE zz (id int)", None),
        ("CREATE INDEX CONCURRENTLY i ON t (a)", None),
        ("REINDEX TABLE t", None),
        ("CLUSTER t", None),
        ("COMMENT ON TABLE t IS 'x'", None),
    ] {
        assert_eq!(ddl_lock(sql), want, "{sql}");
    }
}
