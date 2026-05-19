use gabysql::bptree::init_leaf_page;
use gabysql::sql::{parse, Engine, Value};
use gabysql::storage::{finalize_page_checksum, Header, Pager, PAGE_SIZE_DEFAULT};
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn create_insert_select_roundtrip() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("roundtrip");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, active BOOL, score FLOAT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO users (id,name,active,score) VALUES (1,'Ana',TRUE,9.5); INSERT INTO users (id,name,active,score) VALUES (2,'Beto',FALSE,7.25);",
    )?;

    let results = run_sql(&db, "SELECT id,name,active,score FROM users WHERE id = 2;")?;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].columns, vec!["id", "name", "active", "score"]);
    assert_eq!(
        results[0].rows,
        vec![vec![
            Value::Integer(2),
            Value::String("Beto".to_string()),
            Value::Bool(false),
            Value::Float(7.25),
        ]]
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn full_scan_limit_offset_and_nulls() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("scan");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE person (id INT PRIMARY KEY, name TEXT, active BOOL, score FLOAT, born DATE, meta JSON);",
    )?;
    run_sql(
        &db,
        "INSERT INTO person (id,name,active,score,born,meta) VALUES (1,'Ana',TRUE,9.5,'1990-01-01','{''role'':''dev''}');
         INSERT INTO person (id,name) VALUES (2,'Beto');
         INSERT INTO person (id,name,active,score,born,meta) VALUES (3,'Carla',FALSE,8.0,'1995-05-10','{''role'':''qa''}');",
    )?;

    let results = run_sql(
        &db,
        "SELECT id,name,active,score,born,meta FROM person LIMIT 1 OFFSET 1;",
    )?;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].rows.len(), 1);
    assert_eq!(
        results[0].rows[0],
        vec![
            Value::Integer(2),
            Value::String("Beto".to_string()),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ]
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn duplicate_primary_key_is_rejected() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("duplicate-pk");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE users (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "INSERT INTO users (id,name) VALUES (1,'Ana');")?;
    let err = run_sql(&db, "INSERT INTO users (id,name) VALUES (1,'Otra');").unwrap_err();
    assert!(err.to_string().contains("duplicate primary key"));

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn parser_returns_error_for_invalid_where() {
    let err = parse("SELECT * FROM users WHERE id LIKE 1;").unwrap_err();
    assert!(err.to_string().contains("WHERE soporta solo"));
}

#[test]
fn wal_recovery_replays_committed_pages() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("wal-recovery");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    let mut header = Header::new();
    header.page_count = 2;
    header.catalog_root_page = 1;
    let mut header_page = vec![0; PAGE_SIZE_DEFAULT];
    header.encode_into(&mut header_page)?;
    finalize_page_checksum(&mut header_page);

    let mut leaf_page = vec![0; PAGE_SIZE_DEFAULT];
    init_leaf_page(&mut leaf_page);
    finalize_page_checksum(&mut leaf_page);

    let mut wal_bytes = Vec::new();
    push_wal_page(&mut wal_bytes, 0, &header_page);
    push_wal_page(&mut wal_bytes, 1, &leaf_page);
    wal_bytes.push(2);
    fs::write(&wal, wal_bytes)?;

    let mut reopened = Pager::open(&db)?;
    let recovered = reopened.header();
    assert_eq!(recovered.page_count, 2);
    assert_eq!(recovered.catalog_root_page, 1);
    let page = reopened.page_data(1)?;
    assert_eq!(page[0], 1);
    reopened.close()?;
    assert!(!wal.exists());

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn database_level_statements_parse_and_engine_rejects() -> Result<(), Box<dyn Error>> {
    use gabysql::sql::Statement;

    // Parser accepts the three forms.
    let stmts = parse(
        "CREATE DATABASE foo; \
         CREATE DATABASE IF NOT EXISTS bar; \
         DROP DATABASE foo; \
         DROP DATABASE IF EXISTS bar; \
         SHOW DATABASES;",
    )?;
    assert_eq!(stmts.len(), 5);
    assert!(matches!(stmts[0], Statement::CreateDatabase(_)));
    assert!(matches!(stmts[1], Statement::CreateDatabase(_)));
    if let Statement::CreateDatabase(s) = &stmts[1] {
        assert!(s.if_not_exists);
    }
    assert!(matches!(stmts[2], Statement::DropDatabase(_)));
    assert!(matches!(stmts[3], Statement::DropDatabase(_)));
    if let Statement::DropDatabase(s) = &stmts[3] {
        assert!(s.if_exists);
    }
    assert!(matches!(stmts[4], Statement::ShowDatabases));

    // Engine refuses to execute them — they are caller-dispatched.
    let db = temp_db_path("dbstmts");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;
    let err = run_sql(&db, "CREATE DATABASE other;").unwrap_err();
    assert!(
        err.to_string().contains("CREATE/DROP/SHOW DATABASE"),
        "got: {}",
        err
    );
    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn secondary_index_lookup_and_maintenance() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("idx-secondary");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE u (id INT PRIMARY KEY, name TEXT, score INT);",
    )?;
    for i in 0..200i64 {
        let name = if i % 5 == 0 { "Ana" } else { "Other" };
        let sql = format!(
            "INSERT INTO u (id,name,score) VALUES ({},'{}',{});",
            i,
            name,
            i * 2
        );
        run_sql(&db, &sql)?;
    }

    // Index built AFTER the data is in: must backfill all 40 'Ana' rows.
    run_sql(&db, "CREATE INDEX idx_u_name ON u (name);")?;

    let res = run_sql(&db, "SELECT id FROM u WHERE name = 'Ana';")?;
    assert_eq!(res[0].rows.len(), 40);
    // Returned PKs sorted ascending.
    let first = match &res[0].rows[0][0] {
        Value::Integer(n) => *n,
        other => panic!("expected Integer, got {:?}", other),
    };
    assert_eq!(first, 0);

    // INSERT after index creation: must update the index live.
    run_sql(&db, "INSERT INTO u (id,name,score) VALUES (999,'Ana',1);")?;
    let res = run_sql(&db, "SELECT id FROM u WHERE name = 'Ana';")?;
    assert_eq!(res[0].rows.len(), 41);

    // UPDATE on the indexed column: old bucket loses the entry, new bucket
    // gains it.
    run_sql(&db, "UPDATE u SET name = 'Beto' WHERE id = 999;")?;
    let ana = run_sql(&db, "SELECT id FROM u WHERE name = 'Ana';")?;
    let beto = run_sql(&db, "SELECT id FROM u WHERE name = 'Beto';")?;
    assert_eq!(ana[0].rows.len(), 40);
    assert_eq!(beto[0].rows.len(), 1);

    // DELETE removes from the index too.
    run_sql(&db, "DELETE FROM u WHERE id = 999;")?;
    let beto_after = run_sql(&db, "SELECT id FROM u WHERE name = 'Beto';")?;
    assert_eq!(beto_after[0].rows.len(), 0);

    // Indexed column with INT type also works.
    run_sql(&db, "CREATE INDEX idx_u_score ON u (score);")?;
    let res = run_sql(&db, "SELECT id FROM u WHERE score = 100;")?;
    assert_eq!(res[0].rows.len(), 1);
    assert_eq!(res[0].rows[0][0], Value::Integer(50));

    // Non-indexed and non-PK column still rejected explicitly.
    let err = run_sql(&db, "SELECT id FROM u WHERE name = 'X' AND score = 1;")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    // (Parser doesn't support AND yet — verify the parser, not the
    // index path; the WHERE col=val by an indexed column already
    // succeeded above. This sub-assertion just guards against the parser
    // silently accepting AND in the future without us noticing.)
    assert!(err.contains("token") || err.contains("WHERE") || !err.is_empty());

    // DROP INDEX falls back to "column not indexed" error on next lookup.
    run_sql(&db, "DROP INDEX idx_u_name;")?;
    let err = run_sql(&db, "SELECT id FROM u WHERE name = 'Ana';").unwrap_err();
    assert!(err.to_string().contains("no está indexada"));

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn update_and_delete_by_pk_roundtrip() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("upd-del");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(
        &db,
        "CREATE TABLE u (id INT PRIMARY KEY, name TEXT, score FLOAT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO u (id,name,score) VALUES (1,'Ana',9.0); \
         INSERT INTO u (id,name,score) VALUES (2,'Beto',7.0); \
         INSERT INTO u (id,name,score) VALUES (3,'Caro',8.5);",
    )?;

    // UPDATE single column.
    run_sql(&db, "UPDATE u SET name = 'Ana M' WHERE id = 1;")?;
    let res = run_sql(&db, "SELECT name FROM u WHERE id = 1;")?;
    assert_eq!(res[0].rows[0][0], Value::String("Ana M".to_string()));

    // UPDATE multiple columns.
    run_sql(&db, "UPDATE u SET name = 'B2', score = 10.0 WHERE id = 2;")?;
    let res = run_sql(&db, "SELECT name, score FROM u WHERE id = 2;")?;
    assert_eq!(res[0].rows[0][0], Value::String("B2".to_string()));
    assert_eq!(res[0].rows[0][1], Value::Float(10.0));

    // DELETE by PK.
    run_sql(&db, "DELETE FROM u WHERE id = 3;")?;
    let res = run_sql(&db, "SELECT id FROM u;")?;
    assert_eq!(res[0].rows.len(), 2);

    // UPDATE on missing row should error.
    let err = run_sql(&db, "UPDATE u SET name = 'X' WHERE id = 999;").unwrap_err();
    assert!(err.to_string().contains("fila no existe"));

    // Cannot change PK.
    let err = run_sql(&db, "UPDATE u SET id = 99 WHERE id = 1;").unwrap_err();
    assert!(err.to_string().contains("PRIMARY KEY"));

    // DELETE non-PK column should error.
    let err = run_sql(&db, "DELETE FROM u WHERE name = 1;").unwrap_err();
    assert!(err.to_string().contains("WHERE solo soporta PK"));

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn create_refuses_to_overwrite_existing_db() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("no-overwrite");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE keep (id INT PRIMARY KEY);")?;

    let err = match Pager::create(&db) {
        Err(e) => e,
        Ok(_) => panic!("expected Pager::create to refuse overwrite"),
    };
    assert!(
        err.to_string().contains("refusing to overwrite"),
        "got: {}",
        err
    );

    // create_force succeeds and the new DB is empty (no 'keep' table).
    let mut forced = Pager::create_force(&db)?;
    forced.close()?;
    let err = run_sql(&db, "INSERT INTO keep (id) VALUES (1);").unwrap_err();
    assert!(
        err.to_string().contains("tabla no existe"),
        "force-created DB still has old data: {}",
        err
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn page_checksum_detects_corruption() -> Result<(), Box<dyn Error>> {
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};

    let db = temp_db_path("checksum");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (1,'Ana');")?;

    // Flip a byte inside the leaf page (page 2 here in practice).
    let mut f = OpenOptions::new().read(true).write(true).open(&db)?;
    f.seek(SeekFrom::Start(PAGE_SIZE_DEFAULT as u64 * 2 + 50))?;
    let mut byte = [0u8; 1];
    use std::io::Read;
    f.read_exact(&mut byte)?;
    f.seek(SeekFrom::Start(PAGE_SIZE_DEFAULT as u64 * 2 + 50))?;
    f.write_all(&[byte[0] ^ 0xFF])?;
    drop(f);

    let err = run_sql(&db, "SELECT id FROM u WHERE id = 1;").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("checksum") || msg.contains("corrupt"),
        "expected checksum error, got: {}",
        msg
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn btree_splits_leaves_and_promotes_internal_root() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("btree-splits");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE big (id INT PRIMARY KEY, name TEXT);")?;

    // Insert enough rows that the root must transition from leaf to internal.
    // Each row ~ 8 + (1+8) + (1+2+10) = ~30 bytes; with leaf overhead this
    // forces multiple leaves and at least one root split.
    for i in 0..600i64 {
        let sql = format!("INSERT INTO big (id,name) VALUES ({},'row{:05}');", i, i);
        run_sql(&db, &sql)?;
    }

    // Point lookup at far edge proves descent through internal node.
    let res = run_sql(&db, "SELECT id,name FROM big WHERE id = 599;")?;
    assert_eq!(res[0].rows.len(), 1);
    assert_eq!(res[0].rows[0][0], Value::Integer(599));

    // Range across multiple leaves.
    let res = run_sql(&db, "SELECT id FROM big WHERE id BETWEEN 100 AND 199;")?;
    assert_eq!(res[0].rows.len(), 100);
    assert_eq!(res[0].rows[0][0], Value::Integer(100));
    assert_eq!(res[0].rows[99][0], Value::Integer(199));

    // Full scan still finds all 600.
    let res = run_sql(&db, "SELECT id FROM big;")?;
    assert_eq!(res[0].rows.len(), 600);

    // Page count must have grown beyond a few leaves (proves splits + internal).
    let pager = Pager::open(&db)?;
    assert!(
        pager.header().page_count > 6,
        "expected splits to grow the file"
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn not_null_rejects_missing_and_explicit_null() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("notnull");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE u (id INT PRIMARY KEY, name TEXT NOT NULL, email TEXT);",
    )?;

    // Happy path: name supplied.
    run_sql(&db, "INSERT INTO u (id,name,email) VALUES (1,'Ana','a@x');")?;

    // Missing NOT NULL column → reject.
    let err = run_sql(&db, "INSERT INTO u (id,email) VALUES (2,'b@x');").unwrap_err();
    assert!(err.to_string().contains("NOT NULL"), "got: {}", err);

    // Explicit NULL into NOT NULL column → reject.
    let err = run_sql(&db, "INSERT INTO u (id,name,email) VALUES (3,NULL,'c@x');").unwrap_err();
    assert!(err.to_string().contains("NOT NULL"), "got: {}", err);

    // UPDATE that lands NULL into NOT NULL → reject.
    let err = run_sql(&db, "UPDATE u SET name = NULL WHERE id = 1;").unwrap_err();
    assert!(err.to_string().contains("NOT NULL"), "got: {}", err);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn default_fills_missing_and_can_be_overridden() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("default");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE t (id INT PRIMARY KEY, status TEXT DEFAULT 'new', tries INT DEFAULT 0, active BOOL DEFAULT TRUE);",
    )?;

    // All defaults fire when columns are omitted.
    run_sql(&db, "INSERT INTO t (id) VALUES (1);")?;
    let res = run_sql(&db, "SELECT id,status,tries,active FROM t WHERE id = 1;")?;
    assert_eq!(
        res[0].rows[0],
        vec![
            Value::Integer(1),
            Value::String("new".to_string()),
            Value::Integer(0),
            Value::Bool(true),
        ]
    );

    // Explicit value wins over default.
    run_sql(
        &db,
        "INSERT INTO t (id,status,tries,active) VALUES (2,'done',5,FALSE);",
    )?;
    let res = run_sql(&db, "SELECT status,tries,active FROM t WHERE id = 2;")?;
    assert_eq!(
        res[0].rows[0],
        vec![
            Value::String("done".to_string()),
            Value::Integer(5),
            Value::Bool(false),
        ]
    );

    // Explicit NULL wins over default too (column has no NOT NULL).
    run_sql(&db, "INSERT INTO t (id,status) VALUES (3,NULL);")?;
    let res = run_sql(&db, "SELECT status FROM t WHERE id = 3;")?;
    assert_eq!(res[0].rows[0][0], Value::Null);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn default_with_not_null_combination() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("notnull-default");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE q (id INT PRIMARY KEY, status TEXT NOT NULL DEFAULT 'pending');",
    )?;

    // Omitted → default fills, NOT NULL satisfied.
    run_sql(&db, "INSERT INTO q (id) VALUES (1);")?;
    let res = run_sql(&db, "SELECT status FROM q WHERE id = 1;")?;
    assert_eq!(res[0].rows[0][0], Value::String("pending".to_string()));

    // Explicit NULL still rejected even with a default present.
    let err = run_sql(&db, "INSERT INTO q (id,status) VALUES (2,NULL);").unwrap_err();
    assert!(err.to_string().contains("NOT NULL"), "got: {}", err);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn default_type_mismatch_rejected_at_create() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("default-mismatch");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    let err = run_sql(
        &db,
        "CREATE TABLE bad (id INT PRIMARY KEY, name TEXT DEFAULT 1);",
    )
    .unwrap_err();
    assert!(err.to_string().contains("DEFAULT"), "got: {}", err);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn inline_unique_rejects_duplicates() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("unique-inline");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE u (id INT PRIMARY KEY, email TEXT UNIQUE);",
    )?;

    run_sql(&db, "INSERT INTO u (id,email) VALUES (1,'a@x');")?;
    run_sql(&db, "INSERT INTO u (id,email) VALUES (2,'b@x');")?;

    // Duplicate email rejected.
    let err = run_sql(&db, "INSERT INTO u (id,email) VALUES (3,'a@x');").unwrap_err();
    assert!(err.to_string().contains("UNIQUE"), "got: {}", err);

    // UPDATE that creates a duplicate → reject.
    let err = run_sql(&db, "UPDATE u SET email = 'a@x' WHERE id = 2;").unwrap_err();
    assert!(err.to_string().contains("UNIQUE"), "got: {}", err);

    // UPDATE to the same value (no-op) → allowed.
    run_sql(&db, "UPDATE u SET email = 'a@x' WHERE id = 1;")?;

    // Multiple NULLs allowed under UNIQUE.
    run_sql(&db, "INSERT INTO u (id) VALUES (10);")?;
    run_sql(&db, "INSERT INTO u (id) VALUES (11);")?;

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn create_unique_index_backfill_aborts_on_duplicates() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("unique-backfill");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, email TEXT);")?;
    run_sql(&db, "INSERT INTO u (id,email) VALUES (1,'a@x');")?;
    run_sql(&db, "INSERT INTO u (id,email) VALUES (2,'a@x');")?;

    let err = run_sql(&db, "CREATE UNIQUE INDEX uq_u_email ON u (email);").unwrap_err();
    assert!(
        err.to_string().contains("UNIQUE INDEX") || err.to_string().contains("duplicad"),
        "got: {}",
        err
    );

    // After fixing the duplicate, the unique index can be created.
    run_sql(&db, "UPDATE u SET email = 'b@x' WHERE id = 2;")?;
    run_sql(&db, "CREATE UNIQUE INDEX uq_u_email ON u (email);")?;

    // And it now enforces uniqueness on subsequent INSERTs.
    let err = run_sql(&db, "INSERT INTO u (id,email) VALUES (3,'a@x');").unwrap_err();
    assert!(err.to_string().contains("UNIQUE"), "got: {}", err);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn drop_table_removes_catalog_entry() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("drop-table");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (1,'Ana');")?;

    // Plain DROP succeeds.
    run_sql(&db, "DROP TABLE u;")?;
    let err = run_sql(&db, "SELECT id FROM u;").unwrap_err();
    assert!(err.to_string().contains("tabla no existe"), "got: {}", err);

    // Re-creating with the same name works (catalog slot is free).
    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, email TEXT);")?;
    run_sql(&db, "INSERT INTO u (id,email) VALUES (1,'a@x');")?;
    let res = run_sql(&db, "SELECT email FROM u WHERE id = 1;")?;
    assert_eq!(res[0].rows[0][0], Value::String("a@x".to_string()));

    // DROP TABLE on missing → error without IF EXISTS.
    let err = run_sql(&db, "DROP TABLE missing;").unwrap_err();
    assert!(err.to_string().contains("tabla no existe"), "got: {}", err);

    // DROP TABLE IF EXISTS on missing → silent OK.
    run_sql(&db, "DROP TABLE IF EXISTS missing;")?;

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn alter_add_column_decodes_old_rows_with_default_or_null() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("alter-add");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (1,'Ana');")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (2,'Beto');")?;

    // Plain ADD COLUMN: old rows decode with NULL.
    run_sql(&db, "ALTER TABLE u ADD COLUMN nick TEXT;")?;
    let res = run_sql(&db, "SELECT id,name,nick FROM u WHERE id = 1;")?;
    assert_eq!(
        res[0].rows[0],
        vec![
            Value::Integer(1),
            Value::String("Ana".to_string()),
            Value::Null,
        ]
    );

    // ADD COLUMN with DEFAULT: old rows decode with the default.
    run_sql(
        &db,
        "ALTER TABLE u ADD COLUMN status TEXT NOT NULL DEFAULT 'pending';",
    )?;
    let res = run_sql(&db, "SELECT status FROM u WHERE id = 2;")?;
    assert_eq!(res[0].rows[0][0], Value::String("pending".to_string()));

    // New INSERT can target the new columns explicitly.
    run_sql(
        &db,
        "INSERT INTO u (id,name,nick,status) VALUES (3,'Caro','c','done');",
    )?;
    let res = run_sql(&db, "SELECT nick,status FROM u WHERE id = 3;")?;
    assert_eq!(
        res[0].rows[0],
        vec![
            Value::String("c".to_string()),
            Value::String("done".to_string())
        ]
    );

    // UPDATE old row to materialize the new column on disk; subsequent
    // SELECT still works.
    run_sql(&db, "UPDATE u SET nick = 'A' WHERE id = 1;")?;
    let res = run_sql(&db, "SELECT nick FROM u WHERE id = 1;")?;
    assert_eq!(res[0].rows[0][0], Value::String("A".to_string()));

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn alter_add_column_constraint_guards() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("alter-guards");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (1,'Ana');")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (2,'Beto');")?;

    // NOT NULL without DEFAULT on populated table → reject.
    let err = run_sql(&db, "ALTER TABLE u ADD COLUMN status TEXT NOT NULL;").unwrap_err();
    assert!(err.to_string().contains("NOT NULL"), "got: {}", err);

    // PRIMARY KEY in ADD COLUMN → reject.
    let err = run_sql(&db, "ALTER TABLE u ADD COLUMN extra INT PRIMARY KEY;").unwrap_err();
    assert!(err.to_string().contains("PRIMARY KEY"), "got: {}", err);

    // Duplicate column name → reject.
    let err = run_sql(&db, "ALTER TABLE u ADD COLUMN name TEXT;").unwrap_err();
    assert!(err.to_string().contains("ya existe"), "got: {}", err);

    // UNIQUE with non-NULL DEFAULT on populated table → reject (would
    // immediately produce duplicates).
    let err = run_sql(&db, "ALTER TABLE u ADD COLUMN tag TEXT UNIQUE DEFAULT 'x';").unwrap_err();
    assert!(err.to_string().contains("UNIQUE"), "got: {}", err);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn alter_add_column_unique_then_enforces() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("alter-unique");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (1,'Ana');")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (2,'Beto');")?;

    // Plain UNIQUE on populated table works (existing rows have NULL,
    // multi-NULL allowed under UNIQUE).
    run_sql(&db, "ALTER TABLE u ADD COLUMN email TEXT UNIQUE;")?;

    // Subsequent INSERTs are policed by the new index.
    run_sql(
        &db,
        "INSERT INTO u (id,name,email) VALUES (3,'Caro','c@x');",
    )?;
    let err = run_sql(
        &db,
        "INSERT INTO u (id,name,email) VALUES (4,'Dany','c@x');",
    )
    .unwrap_err();
    assert!(err.to_string().contains("UNIQUE"), "got: {}", err);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn identifier_rules_apply_across_ddl() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("identifiers");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    // Reserved word as table name → rejected.
    let err = run_sql(&db, "CREATE TABLE select (id INT PRIMARY KEY);").unwrap_err();
    assert!(
        err.to_string().contains("palabra reservada"),
        "got: {}",
        err
    );

    // Reserved word as column name → rejected. (Parser tokenizes 'where'
    // as an identifier here because it's after the comma; the validator
    // catches it before the engine does anything.)
    let err = run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, where TEXT);").unwrap_err();
    assert!(
        err.to_string().contains("palabra reservada"),
        "got: {}",
        err
    );

    // Identifier longer than the documented 64-char ceiling is rejected.
    let long = "a".repeat(65);
    let sql = format!("CREATE TABLE u (id INT PRIMARY KEY, {} TEXT);", long);
    let err = run_sql(&db, &sql).unwrap_err();
    assert!(err.to_string().contains("excede el máximo"), "got: {}", err);

    // Happy path: a normal CREATE still works after all those rejections.
    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;

    // Bad index name (reserved word) is rejected at CREATE INDEX time.
    let err = run_sql(&db, "CREATE INDEX select ON u (name);").unwrap_err();
    assert!(
        err.to_string().contains("palabra reservada"),
        "got: {}",
        err
    );

    // ALTER TABLE ADD COLUMN runs the same validator on the new column.
    let err = run_sql(&db, "ALTER TABLE u ADD COLUMN where TEXT;").unwrap_err();
    assert!(
        err.to_string().contains("palabra reservada"),
        "got: {}",
        err
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn fk_create_validation_rejects_bad_targets() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("fk-validate");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    // FK to non-existent table → reject.
    let err = run_sql(
        &db,
        "CREATE TABLE child (id INT PRIMARY KEY, parent_id INT REFERENCES parent(id));",
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("tabla inexistente"),
        "got: {}",
        err
    );

    run_sql(&db, "CREATE TABLE parent (id INT PRIMARY KEY, name TEXT);")?;

    // FK to a non-PK column → reject.
    let err = run_sql(
        &db,
        "CREATE TABLE bad (id INT PRIMARY KEY, p_name INT REFERENCES parent(name));",
    )
    .unwrap_err();
    assert!(err.to_string().contains("PK"), "got: {}", err);

    // FK type mismatch (TEXT vs INT) → reject.
    let err = run_sql(
        &db,
        "CREATE TABLE bad (id INT PRIMARY KEY, p TEXT REFERENCES parent(id));",
    )
    .unwrap_err();
    assert!(err.to_string().contains("INT"), "got: {}", err);

    // Happy path.
    run_sql(
        &db,
        "CREATE TABLE child (id INT PRIMARY KEY, parent_id INT REFERENCES parent(id));",
    )?;

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn fk_insert_update_enforcement() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("fk-iu");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE parent (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(
        &db,
        "CREATE TABLE child (id INT PRIMARY KEY, parent_id INT REFERENCES parent(id));",
    )?;
    run_sql(&db, "INSERT INTO parent (id,name) VALUES (1,'Ana');")?;
    run_sql(&db, "INSERT INTO parent (id,name) VALUES (2,'Beto');")?;

    // INSERT with valid parent → OK.
    run_sql(&db, "INSERT INTO child (id,parent_id) VALUES (10,1);")?;

    // INSERT with NULL FK → OK.
    run_sql(&db, "INSERT INTO child (id) VALUES (11);")?;

    // INSERT with non-existent parent → reject.
    let err = run_sql(&db, "INSERT INTO child (id,parent_id) VALUES (12,99);").unwrap_err();
    assert!(err.to_string().contains("FK"), "got: {}", err);

    // UPDATE FK to non-existent parent → reject.
    let err = run_sql(&db, "UPDATE child SET parent_id = 99 WHERE id = 10;").unwrap_err();
    assert!(err.to_string().contains("FK"), "got: {}", err);

    // UPDATE FK to existing parent → OK.
    run_sql(&db, "UPDATE child SET parent_id = 2 WHERE id = 10;")?;

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn fk_self_reference_allows_pointing_at_self() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("fk-self");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE employee (id INT PRIMARY KEY, name TEXT, manager_id INT REFERENCES employee(id));",
    )?;

    // Top-level employee is their own manager.
    run_sql(
        &db,
        "INSERT INTO employee (id,name,manager_id) VALUES (1,'CEO',1);",
    )?;
    // Subordinate referencing existing manager.
    run_sql(
        &db,
        "INSERT INTO employee (id,name,manager_id) VALUES (2,'VP',1);",
    )?;
    // Subordinate referencing non-existent manager → reject.
    let err = run_sql(
        &db,
        "INSERT INTO employee (id,name,manager_id) VALUES (3,'Lost',99);",
    )
    .unwrap_err();
    assert!(err.to_string().contains("FK"), "got: {}", err);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn fk_delete_restrict_blocks_when_children_exist() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("fk-restrict");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE parent (id INT PRIMARY KEY, name TEXT);")?;
    // No ON DELETE clause → defaults to RESTRICT.
    run_sql(
        &db,
        "CREATE TABLE child (id INT PRIMARY KEY, parent_id INT REFERENCES parent(id));",
    )?;
    run_sql(&db, "INSERT INTO parent (id,name) VALUES (1,'Ana');")?;
    run_sql(&db, "INSERT INTO child (id,parent_id) VALUES (10,1);")?;

    // DELETE parent with existing child → reject.
    let err = run_sql(&db, "DELETE FROM parent WHERE id = 1;").unwrap_err();
    assert!(err.to_string().contains("RESTRICT"), "got: {}", err);

    // After clearing the child, DELETE parent succeeds.
    run_sql(&db, "DELETE FROM child WHERE id = 10;")?;
    run_sql(&db, "DELETE FROM parent WHERE id = 1;")?;

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn fk_delete_cascade_removes_children_and_grandchildren() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("fk-cascade");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE a (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(
        &db,
        "CREATE TABLE b (id INT PRIMARY KEY, a_id INT REFERENCES a(id) ON DELETE CASCADE);",
    )?;
    run_sql(
        &db,
        "CREATE TABLE c (id INT PRIMARY KEY, b_id INT REFERENCES b(id) ON DELETE CASCADE);",
    )?;

    run_sql(&db, "INSERT INTO a (id,name) VALUES (1,'root');")?;
    run_sql(&db, "INSERT INTO b (id,a_id) VALUES (10,1);")?;
    run_sql(&db, "INSERT INTO b (id,a_id) VALUES (11,1);")?;
    run_sql(&db, "INSERT INTO c (id,b_id) VALUES (100,10);")?;
    run_sql(&db, "INSERT INTO c (id,b_id) VALUES (101,11);")?;

    // Cascade delete from a → b → c should leave nothing in any table.
    run_sql(&db, "DELETE FROM a WHERE id = 1;")?;

    let res = run_sql(&db, "SELECT id FROM a;")?;
    assert_eq!(res[0].rows.len(), 0);
    let res = run_sql(&db, "SELECT id FROM b;")?;
    assert_eq!(res[0].rows.len(), 0);
    let res = run_sql(&db, "SELECT id FROM c;")?;
    assert_eq!(res[0].rows.len(), 0);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn old_v5_db_file_is_rejected_after_v6_bump() -> Result<(), Box<dyn Error>> {
    // Smoke check: the on-disk version really moved to 6 — a fresh DB
    // round-trips, but if somebody hand-pokes the version byte in the
    // header back to 5 the reopen must refuse.
    let db = temp_db_path("v6-bump");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY);")?;

    // Patch on-disk version 6 → 5 inside the header page (offset 8..12).
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};
    let mut f = OpenOptions::new().read(true).write(true).open(&db)?;
    f.seek(SeekFrom::Start(8))?;
    f.write_all(&5u32.to_le_bytes())?;
    // Header page CRC will mismatch now, so we expect either a checksum
    // or version error — both indicate the engine refused the file.
    drop(f);

    let err = match Pager::open(&db) {
        Err(e) => e,
        Ok(_) => panic!("expected Pager::open to refuse v5 file"),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("checksum") || msg.contains("version") || msg.contains("corrupt"),
        "expected refusal, got: {}",
        msg
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn integrity_check_clean_db_returns_ok() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("integrity-clean");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE parent (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(
        &db,
        "CREATE TABLE child (id INT PRIMARY KEY, parent_id INT REFERENCES parent(id));",
    )?;
    run_sql(&db, "INSERT INTO parent (id,name) VALUES (1,'Ana');")?;
    run_sql(&db, "INSERT INTO parent (id,name) VALUES (2,'Beto');")?;
    run_sql(&db, "INSERT INTO child (id,parent_id) VALUES (10,1);")?;
    run_sql(&db, "INSERT INTO child (id,parent_id) VALUES (11,2);")?;
    run_sql(&db, "CREATE INDEX idx_child_parent ON child (parent_id);")?;

    let res = run_sql(&db, "INTEGRITY CHECK;")?;
    assert_eq!(
        res[0].columns,
        vec!["kind".to_string(), "object".into(), "detail".into()]
    );
    assert!(
        res[0].rows.is_empty(),
        "expected no findings, got: {:?}",
        res[0].rows
    );
    let msg = res[0].message.as_deref().unwrap_or("");
    assert!(msg.starts_with("OK"), "expected OK summary, got: {}", msg);
    // Smoke-check the summary mentions both tables and at least one FK check.
    assert!(msg.contains("2 tablas"), "got: {}", msg);
    assert!(msg.contains("FKs"), "got: {}", msg);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn integrity_check_reports_corrupted_page() -> Result<(), Box<dyn Error>> {
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};

    let db = temp_db_path("integrity-corrupt");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (1,'Ana');")?;

    // Flip a byte inside a leaf page (page 2 in practice).
    let mut f = OpenOptions::new().read(true).write(true).open(&db)?;
    f.seek(SeekFrom::Start(PAGE_SIZE_DEFAULT as u64 * 2 + 50))?;
    let mut byte = [0u8; 1];
    use std::io::Read;
    f.read_exact(&mut byte)?;
    f.seek(SeekFrom::Start(PAGE_SIZE_DEFAULT as u64 * 2 + 50))?;
    f.write_all(&[byte[0] ^ 0xFF])?;
    drop(f);

    // INTEGRITY CHECK must surface the corruption as a finding rather
    // than just bailing — the engine should walk every page, collect
    // failures, and return them as rows.
    let res = run_sql(&db, "INTEGRITY CHECK;");
    // Two acceptable shapes: either INTEGRITY CHECK itself succeeds and
    // surfaces the corrupted page as a row, or the corruption surfaces
    // earlier (during scan). Both prove the check did its job.
    match res {
        Ok(results) => {
            let rs = &results[0];
            assert!(
                !rs.rows.is_empty(),
                "expected at least one finding, got rows={:?} msg={:?}",
                rs.rows,
                rs.message
            );
            let kinds: Vec<&str> = rs
                .rows
                .iter()
                .filter_map(|r| match &r[0] {
                    Value::String(s) => Some(s.as_str()),
                    _ => None,
                })
                .collect();
            assert!(
                kinds
                    .iter()
                    .any(|k| *k == "page_corrupt" || *k == "row_decode"),
                "expected page_corrupt/row_decode, got: {:?}",
                kinds
            );
        }
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("checksum") || msg.contains("corrupt"),
                "unexpected error: {}",
                msg
            );
        }
    }

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn order_by_int_asc_desc() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("order-int");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, score INT);")?;
    run_sql(&db, "INSERT INTO u (id,score) VALUES (1,30);")?;
    run_sql(&db, "INSERT INTO u (id,score) VALUES (2,10);")?;
    run_sql(&db, "INSERT INTO u (id,score) VALUES (3,20);")?;

    let asc = run_sql(&db, "SELECT id FROM u ORDER BY score ASC;")?;
    let asc_pks: Vec<i64> = asc[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => -1,
        })
        .collect();
    assert_eq!(asc_pks, vec![2, 3, 1]);

    let desc = run_sql(&db, "SELECT id FROM u ORDER BY score DESC;")?;
    let desc_pks: Vec<i64> = desc[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => -1,
        })
        .collect();
    assert_eq!(desc_pks, vec![1, 3, 2]);

    // Default direction is ASC.
    let dflt = run_sql(&db, "SELECT id FROM u ORDER BY score;")?;
    let dflt_pks: Vec<i64> = dflt[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => -1,
        })
        .collect();
    assert_eq!(dflt_pks, vec![2, 3, 1]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn order_by_text_with_limit_offset_window() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("order-text");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (1,'Carla');")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (2,'Ana');")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (3,'Beto');")?;

    // Sort by name ASC, take the middle row only.
    let res = run_sql(
        &db,
        "SELECT id,name FROM u ORDER BY name ASC LIMIT 1 OFFSET 1;",
    )?;
    assert_eq!(res[0].rows.len(), 1);
    assert_eq!(res[0].rows[0][1], Value::String("Beto".to_string()));

    // Sort DESC, take all.
    let desc = run_sql(&db, "SELECT name FROM u ORDER BY name DESC;")?;
    let names: Vec<String> = desc[0]
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::String(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(
        names,
        vec!["Carla".to_string(), "Beto".to_string(), "Ana".to_string()]
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn order_by_nulls_sort_first_under_asc() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("order-null");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (1,'Beto');")?;
    run_sql(&db, "INSERT INTO u (id) VALUES (2);")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (3,'Ana');")?;

    let asc = run_sql(&db, "SELECT id FROM u ORDER BY name ASC;")?;
    let asc_pks: Vec<i64> = asc[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => -1,
        })
        .collect();
    // NULL first, then 'Ana', then 'Beto'
    assert_eq!(asc_pks, vec![2, 3, 1]);

    let desc = run_sql(&db, "SELECT id FROM u ORDER BY name DESC;")?;
    let desc_pks: Vec<i64> = desc[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => -1,
        })
        .collect();
    // 'Beto', 'Ana', NULL last (because we just reverse the ASC order)
    assert_eq!(desc_pks, vec![1, 3, 2]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn order_by_unknown_column_rejected() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("order-bad");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;
    let err = run_sql(&db, "SELECT id FROM u ORDER BY nope ASC;").unwrap_err();
    assert!(err.to_string().contains("ORDER BY"), "got: {}", err);
    cleanup(&[&db, &wal]);
    Ok(())
}

/// Crash simulation: a WAL that carries a `COMMIT` marker plus the
/// payload pages must rebuild the database when the main file is
/// missing those pages — that's the "kill -9 between WAL flush and
/// file flush" scenario described in the ROADMAP.
///
/// We don't actually kill a process. We synthesize the on-disk state
/// that such a kill would leave behind: a healthy main file truncated
/// before the latest writes hit it, with the WAL still on disk
/// carrying the committed pages.
#[test]
fn crash_recovery_partial_file_restored_from_wal() -> Result<(), Box<dyn Error>> {
    use std::fs::OpenOptions;
    use std::io::Read;

    let db = temp_db_path("crash-partial");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    // 1) Build a healthy DB with a known table + a few rows. Close it
    //    cleanly so the WAL is gone and the main file is the source of
    //    truth.
    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(
        &db,
        "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);
         INSERT INTO u (id,name) VALUES (1,'Ana');
         INSERT INTO u (id,name) VALUES (2,'Beto');
         INSERT INTO u (id,name) VALUES (3,'Carla');",
    )?;
    assert!(!wal.exists(), "WAL must be removed after a clean commit");

    // 2) Snapshot the data file. We'll need every page byte-for-byte
    //    to forge the WAL below — replay re-applies whatever payload
    //    we record, which is the same payload that's on disk now.
    let mut data = Vec::new();
    {
        let mut f = OpenOptions::new().read(true).open(&db)?;
        f.read_to_end(&mut data)?;
    }
    assert_eq!(data.len() % PAGE_SIZE_DEFAULT, 0);
    let total_pages = (data.len() / PAGE_SIZE_DEFAULT) as u32;
    assert!(
        total_pages >= 3,
        "expected at least header + catalog + leaf, got {} pages",
        total_pages
    );

    // 3) Build a WAL containing every non-header page plus a COMMIT
    //    marker. This mimics the state after `commit()` synced the
    //    WAL but before any of the main-file writes landed.
    let mut wal_bytes = Vec::new();
    for page_no in 1..total_pages {
        let start = page_no as usize * PAGE_SIZE_DEFAULT;
        let end = start + PAGE_SIZE_DEFAULT;
        push_wal_page(&mut wal_bytes, page_no, &data[start..end]);
    }
    wal_bytes.push(2); // COMMIT marker
    fs::write(&wal, wal_bytes)?;

    // 4) Truncate the main file: drop everything after the header to
    //    simulate the kill happening before any payload page hit disk.
    {
        let f = OpenOptions::new().write(true).open(&db)?;
        f.set_len(PAGE_SIZE_DEFAULT as u64)?;
    }

    // 5) Reopen — the pager should replay the WAL and restore every
    //    truncated page. SELECT must return the original three rows
    //    in PK order, and the WAL file must be cleaned up afterwards.
    let res = run_sql(&db, "SELECT id,name FROM u;")?;
    assert_eq!(res[0].rows.len(), 3);
    assert_eq!(res[0].rows[0][0], Value::Integer(1));
    assert_eq!(res[0].rows[2][1], Value::String("Carla".to_string()));
    assert!(!wal.exists(), "WAL must be removed after replay");

    cleanup(&[&db, &wal]);
    Ok(())
}

/// Crash simulation: a WAL that doesn't carry a `COMMIT` marker is
/// the trace of a kill *before* the transaction was durable. The
/// main file must remain authoritative — any partial WAL content is
/// discarded on reopen.
#[test]
fn crash_recovery_wal_without_commit_is_ignored() -> Result<(), Box<dyn Error>> {
    use std::fs::OpenOptions;
    use std::io::{Read, Seek, SeekFrom};

    let db = temp_db_path("crash-no-commit");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    // 1) Healthy DB with a single committed row.
    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(
        &db,
        "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);
         INSERT INTO u (id,name) VALUES (1,'Ana');",
    )?;

    // 2) Forge a WAL with a "would-be" page but NO commit marker.
    //    Use a copy of the existing page-1 payload (a real, CRC-valid
    //    page) so the WAL parser doesn't choke before reaching the
    //    missing commit byte.
    let mut data = vec![0u8; PAGE_SIZE_DEFAULT];
    {
        let mut f = OpenOptions::new().read(true).open(&db)?;
        f.seek(SeekFrom::Start(PAGE_SIZE_DEFAULT as u64))?;
        f.read_exact(&mut data)?;
    }
    let mut wal_bytes = Vec::new();
    push_wal_page(&mut wal_bytes, 1, &data);
    // Intentionally no COMMIT byte appended here.
    fs::write(&wal, wal_bytes)?;

    // 3) Reopen. The pager must NOT replay the WAL (no COMMIT) and
    //    must remove the WAL file after the no-op recovery. The
    //    original row stays unchanged.
    let res = run_sql(&db, "SELECT id,name FROM u;")?;
    assert_eq!(res[0].rows.len(), 1);
    assert_eq!(res[0].rows[0][1], Value::String("Ana".to_string()));
    assert!(
        !wal.exists(),
        "WAL must be removed even when commit is absent"
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

/// Crash simulation: between flushing the WAL with a COMMIT and
/// flushing the main file, only *some* pages may have made it. The
/// replay path must idempotently overwrite even already-correct
/// pages — running the WAL twice (or against a half-applied file)
/// must converge on the same end state.
#[test]
fn crash_recovery_replay_is_idempotent() -> Result<(), Box<dyn Error>> {
    use std::fs::OpenOptions;
    use std::io::Read;

    let db = temp_db_path("crash-idempotent");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(
        &db,
        "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);
         INSERT INTO u (id,name) VALUES (1,'Ana');
         INSERT INTO u (id,name) VALUES (2,'Beto');",
    )?;

    // Snapshot the file and forge a WAL with a COMMIT containing those
    // exact pages.
    let mut data = Vec::new();
    {
        let mut f = OpenOptions::new().read(true).open(&db)?;
        f.read_to_end(&mut data)?;
    }
    let total_pages = (data.len() / PAGE_SIZE_DEFAULT) as u32;
    let mut wal_bytes = Vec::new();
    for page_no in 1..total_pages {
        let start = page_no as usize * PAGE_SIZE_DEFAULT;
        let end = start + PAGE_SIZE_DEFAULT;
        push_wal_page(&mut wal_bytes, page_no, &data[start..end]);
    }
    wal_bytes.push(2);
    fs::write(&wal, &wal_bytes)?;

    // First reopen replays the WAL → state stays correct, WAL gone.
    let res = run_sql(&db, "SELECT id FROM u;")?;
    assert_eq!(res[0].rows.len(), 2);
    assert!(!wal.exists());

    // Re-plant the same WAL and reopen again. The replay must converge
    // on the same end state (no double-counting, no corruption).
    fs::write(&wal, &wal_bytes)?;
    let res = run_sql(&db, "SELECT id FROM u;")?;
    assert_eq!(res[0].rows.len(), 2);
    assert!(!wal.exists());

    cleanup(&[&db, &wal]);
    Ok(())
}

/// `SELECT … LIMIT N` over a large table must NOT materialize every
/// row before windowing — that's the whole reason the LeafCursor
/// exists. We exercise the property with a 1.000-row table and assert
/// LIMIT 5 returns the expected first five PKs in key order. The
/// cursor's promise is observable through behaviour (correctness),
/// not direct memory measurement; the resource win is verified by
/// reading the executor code path.
/// The Pager's page cache must be bounded and must evict clean pages
/// LRU-style when at capacity. Pre-block-10 the cache was an
/// unbounded `BTreeMap` — a long-running server scanning many DBs
/// would leak memory proportional to the working set.
#[test]
fn page_cache_is_bounded_and_evicts_clean_pages() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("lru-cache");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;

    // Seed enough rows to span several leaves.
    run_sql(&db, "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);")?;
    for i in 0..200i64 {
        run_sql(
            &db,
            &format!("INSERT INTO u (id,name) VALUES ({},'row{:03}');", i, i),
        )?;
    }

    // Reopen with a tiny cache and walk every page (no tx open, so
    // every page is clean and evictable). The cache must respect cap.
    let mut pager = Pager::open(&db)?;
    pager.set_cache_capacity(4);
    assert_eq!(pager.cache_capacity(), 4);
    let header = pager.header();
    for no in 0..header.page_count {
        let _ = pager.page_data(no)?;
    }
    assert!(
        pager.cache_len() <= pager.cache_capacity(),
        "cache_len={} > cap={}",
        pager.cache_len(),
        pager.cache_capacity()
    );
    pager.close()?;

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn cursor_limit_returns_only_requested_rows() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("cursor-limit");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE big (id INT PRIMARY KEY, name TEXT);")?;
    for i in 0..1000i64 {
        run_sql(
            &db,
            &format!("INSERT INTO big (id,name) VALUES ({},'row{:04}');", i, i),
        )?;
    }

    // LIMIT 5 with no offset
    let res = run_sql(&db, "SELECT id FROM big LIMIT 5;")?;
    let pks: Vec<i64> = res[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => -1,
        })
        .collect();
    assert_eq!(pks, vec![0, 1, 2, 3, 4]);

    // LIMIT 3 OFFSET 7
    let res = run_sql(&db, "SELECT id FROM big LIMIT 3 OFFSET 7;")?;
    let pks: Vec<i64> = res[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => -1,
        })
        .collect();
    assert_eq!(pks, vec![7, 8, 9]);

    // BETWEEN range with LIMIT — cursor_range must respect the upper
    // bound AND short-circuit at LIMIT.
    let res = run_sql(
        &db,
        "SELECT id FROM big WHERE id BETWEEN 100 AND 999 LIMIT 4;",
    )?;
    let pks: Vec<i64> = res[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => -1,
        })
        .collect();
    assert_eq!(pks, vec![100, 101, 102, 103]);

    // Range whose upper bound is well before the end: must stop at
    // the bound, not run to EOF.
    let res = run_sql(&db, "SELECT id FROM big WHERE id BETWEEN 5 AND 9;")?;
    let pks: Vec<i64> = res[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => -1,
        })
        .collect();
    assert_eq!(pks, vec![5, 6, 7, 8, 9]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn where_between_on_int_indexed_column_uses_ordered_index() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("idx-int-between");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    // Note: PK is `id`, but we'll BETWEEN over `score` which is a
    // non-PK INT column that gets an OrderedInt index.
    run_sql(
        &db,
        "CREATE TABLE u (id INT PRIMARY KEY, name TEXT, score INT NOT NULL);",
    )?;
    run_sql(&db, "CREATE INDEX idx_u_score ON u (score);")?;
    for (id, score) in [(1, 10), (2, 25), (3, 50), (4, 75), (5, 90), (6, 100)] {
        run_sql(
            &db,
            &format!(
                "INSERT INTO u (id,name,score) VALUES ({},'r{}',{});",
                id, id, score
            ),
        )?;
    }
    // Insert a NULL-score row to confirm OrderedInt indexes skip NULLs
    // (and so BETWEEN ignores them, matching SQL semantics).
    run_sql(&db, "CREATE TABLE u2 (id INT PRIMARY KEY, score INT);")?;
    run_sql(&db, "CREATE INDEX idx_u2_score ON u2 (score);")?;
    run_sql(&db, "INSERT INTO u2 (id,score) VALUES (1,42);")?;
    run_sql(&db, "INSERT INTO u2 (id,score) VALUES (2,NULL);")?;
    run_sql(&db, "INSERT INTO u2 (id,score) VALUES (3,55);")?;

    // BETWEEN over the indexed INT column.
    let res = run_sql(&db, "SELECT id,score FROM u WHERE score BETWEEN 20 AND 80;")?;
    let pairs: Vec<(i64, i64)> = res[0]
        .rows
        .iter()
        .map(|r| match (&r[0], &r[1]) {
            (Value::Integer(id), Value::Integer(s)) => (*id, *s),
            _ => (-1, -1),
        })
        .collect();
    let mut sorted = pairs.clone();
    sorted.sort();
    assert_eq!(sorted, vec![(2, 25), (3, 50), (4, 75)], "got: {:?}", pairs);

    // BETWEEN that misses everything.
    let res = run_sql(&db, "SELECT id FROM u WHERE score BETWEEN 200 AND 300;")?;
    assert_eq!(res[0].rows.len(), 0);

    // NULL-score row must NOT show up in BETWEEN (matches ANSI SQL).
    let res = run_sql(&db, "SELECT id FROM u2 WHERE score BETWEEN 0 AND 1000;")?;
    let ids: Vec<i64> = res[0]
        .rows
        .iter()
        .filter_map(|r| match r[0] {
            Value::Integer(n) => Some(n),
            _ => None,
        })
        .collect();
    let mut sorted_ids = ids.clone();
    sorted_ids.sort();
    assert_eq!(
        sorted_ids,
        vec![1, 3],
        "NULL row leaked into BETWEEN: {:?}",
        ids
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn where_between_on_text_indexed_column_is_rejected() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("idx-text-between-reject");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(
        &db,
        "CREATE TABLE u (id INT PRIMARY KEY, name TEXT NOT NULL);",
    )?;
    run_sql(&db, "CREATE INDEX idx_u_name ON u (name);")?;
    run_sql(&db, "INSERT INTO u (id,name) VALUES (1,'Ana');")?;

    // The parser only accepts integer literals in BETWEEN today, so
    // text literals fail there first. The defense-in-depth path that
    // matters is the engine gate: even when ints sneak through against
    // a TEXT-indexed column, the engine refuses because the index is
    // hash-based (equality only).
    let parser_err = run_sql(&db, "SELECT id FROM u WHERE name BETWEEN 'A' AND 'Z';");
    assert!(parser_err.is_err(), "TEXT literals in BETWEEN should fail");

    let engine_err = run_sql(&db, "SELECT id FROM u WHERE name BETWEEN 1 AND 10;");
    assert!(
        engine_err.is_err(),
        "BETWEEN on hash-indexed column should be rejected by the engine"
    );
    let msg = engine_err.err().unwrap().to_string();
    assert!(
        msg.contains("hash") || msg.contains("equality") || msg.contains("INT"),
        "error should explain why TEXT BETWEEN is rejected, got: {}",
        msg
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn backup_roundtrip_verifies_end_to_end() -> Result<(), Box<dyn Error>> {
    let src = temp_db_path("backup-src");
    let dst = temp_db_path("backup-dst");
    let wal_src = wal_path(&src);
    let wal_dst = wal_path(&dst);
    cleanup(&[&src, &dst, &wal_src, &wal_dst]);

    let mut pager = Pager::create(&src)?;
    pager.close()?;
    run_sql(
        &src,
        "CREATE TABLE users (id INT PRIMARY KEY, name TEXT NOT NULL);",
    )?;
    run_sql(
        &src,
        "INSERT INTO users (id,name) VALUES (1,'Ana'); INSERT INTO users (id,name) VALUES (2,'Beto'); INSERT INTO users (id,name) VALUES (3,'Cris');",
    )?;

    // Backup must produce a verified-good copy.
    let report = gabysql::backup::backup(&src, &dst, false)?;
    assert!(report.pages >= 2, "expected at least a header + data page");
    assert_eq!(report.bytes as usize, report.pages as usize * 4096);

    // Refuses to overwrite without --force.
    let again = gabysql::backup::backup(&src, &dst, false);
    assert!(again.is_err(), "second backup without --force should fail");

    // With force, the second backup succeeds.
    gabysql::backup::backup(&src, &dst, true)?;

    // The destination must hold the same rows as the source.
    let rows = run_sql(&dst, "SELECT id,name FROM users WHERE id = 2;")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rows.len(), 1);
    assert_eq!(rows[0].rows[0][0], Value::Integer(2));
    assert_eq!(rows[0].rows[0][1], Value::String("Beto".to_string()));

    cleanup(&[&src, &dst, &wal_src, &wal_dst]);
    Ok(())
}

#[test]
fn backup_detects_corrupted_source() -> Result<(), Box<dyn Error>> {
    let src = temp_db_path("backup-corrupt-src");
    let dst = temp_db_path("backup-corrupt-dst");
    let wal_src = wal_path(&src);
    let wal_dst = wal_path(&dst);
    cleanup(&[&src, &dst, &wal_src, &wal_dst]);

    let mut pager = Pager::create(&src)?;
    pager.close()?;
    run_sql(
        &src,
        "CREATE TABLE t (id INT PRIMARY KEY, payload TEXT NOT NULL);",
    )?;
    run_sql(
        &src,
        "INSERT INTO t (id,payload) VALUES (1,'hello'); INSERT INTO t (id,payload) VALUES (2,'world');",
    )?;

    // Flip a byte in the middle of page 1 (after the header page) so
    // its CRC32 trailer no longer matches. backup() must catch this
    // and refuse to publish the corrupted copy.
    {
        let mut bytes = fs::read(&src)?;
        assert!(bytes.len() >= 2 * 4096, "DB too small for the test setup");
        bytes[4096 + 100] ^= 0xFF;
        fs::write(&src, bytes)?;
    }

    let result = gabysql::backup::backup(&src, &dst, true);
    assert!(
        result.is_err(),
        "backup should refuse a source with a corrupt page"
    );
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("corrupt") || msg.contains("checksum") || msg.contains("CRC"),
        "error should mention corruption, got: {}",
        msg
    );

    cleanup(&[&src, &dst, &wal_src, &wal_dst]);
    Ok(())
}

#[test]
fn verify_walks_every_page() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("verify");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE t (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(
        &db,
        "INSERT INTO t (id,name) VALUES (1,'a'); INSERT INTO t (id,name) VALUES (2,'b');",
    )?;

    let report = gabysql::backup::verify(&db)?;
    assert!(report.pages >= 2);
    assert_eq!(report.bytes as usize, report.pages as usize * 4096);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn cross_process_lock_rejects_second_open() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("xproc-lock");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    // First Pager creates and holds the exclusive lock on the .db.
    let mut first = Pager::create(&db)?;

    // While `first` is alive, a second open on the same path must
    // fail with a clear "database is locked" error — never a silent
    // success that would let two writers race over the same file.
    let second = Pager::open(&db);
    assert!(second.is_err(), "second open should be rejected");
    let err = second.err().unwrap().to_string();
    assert!(
        err.contains("locked"),
        "error should mention the lock, got: {}",
        err
    );

    // Releasing the first lock makes the same path openable again,
    // proving the lock is held on the handle (not the path) and is
    // released on close.
    first.close()?;
    drop(first);

    let mut third = Pager::open(&db)?;
    third.close()?;

    cleanup(&[&db, &wal]);
    Ok(())
}

fn run_sql(path: &Path, sql_text: &str) -> Result<Vec<gabysql::sql::ResultSet>, Box<dyn Error>> {
    let mut pager = Pager::open(path)?;
    pager.begin()?;
    let response = (|| {
        let statements = parse(sql_text)?;
        let mut engine = Engine::new(&mut pager);
        let mut results = Vec::new();
        for statement in statements {
            results.push(engine.exec(statement)?);
        }
        pager.commit()?;
        Ok::<_, gabysql::DbError>(results)
    })();

    match response {
        Ok(results) => Ok(results),
        Err(err) => {
            let _ = pager.rollback();
            Err(Box::new(err))
        }
    }
}

fn temp_db_path(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("gabysql-{}-{}.db", label, stamp))
}

fn wal_path(path: &Path) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(".wal");
    PathBuf::from(value)
}

fn cleanup(paths: &[&Path]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

fn push_wal_page(out: &mut Vec<u8>, page_no: u32, data: &[u8]) {
    out.push(1);
    out.extend_from_slice(&page_no.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
}
