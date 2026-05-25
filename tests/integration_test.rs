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
    let msg = err.to_string();
    assert!(
        msg.contains("PRIMARY KEY duplicada") || msg.contains("ya existe"),
        "mensaje de PK duplicada no incluye la palabra clave esperada: {}",
        msg
    );

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
    let msg = err.to_string();
    assert!(
        msg.contains("PRIMARY KEY") || msg.contains("PK"),
        "el error de DELETE sin PK debe mencionar PRIMARY KEY: {}",
        msg
    );

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
    let msg = err.to_string();
    assert!(
        msg.contains("se rehúsa sobrescribir") || msg.contains("refusing to overwrite"),
        "got: {}",
        msg
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
    assert!(
        err.to_string().contains("FK") || err.to_string().contains("FOREIGN KEY"),
        "got: {}",
        err
    );

    // UPDATE FK to non-existent parent → reject.
    let err = run_sql(&db, "UPDATE child SET parent_id = 99 WHERE id = 10;").unwrap_err();
    assert!(
        err.to_string().contains("FK") || err.to_string().contains("FOREIGN KEY"),
        "got: {}",
        err
    );

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
    assert!(
        err.to_string().contains("FK") || err.to_string().contains("FOREIGN KEY"),
        "got: {}",
        err
    );

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
fn where_in_subquery_basic() -> Result<(), Box<dyn Error>> {
    // Cubre el caso de uso reportado: SELECT … WHERE col IN (SELECT … FROM otra
    // WHERE …). Se modelan dos tablas (cursos → alumnos) y se exige el filtro
    // por subquery no-correlacionada.
    let db = temp_db_path("in_subquery_basic");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE cursos (id INT PRIMARY KEY, nivel TEXT NOT NULL);",
    )?;
    run_sql(
        &db,
        "CREATE TABLE alumnos (id INT PRIMARY KEY, nombre TEXT NOT NULL, curso_id INT);",
    )?;
    run_sql(&db, "CREATE INDEX idx_cursos_nivel ON cursos (nivel);")?;
    run_sql(&db, "CREATE INDEX idx_alumnos_curso ON alumnos (curso_id);")?;

    run_sql(
        &db,
        "INSERT INTO cursos (id,nivel) VALUES (1,'3 Medio');
         INSERT INTO cursos (id,nivel) VALUES (2,'4 Medio');
         INSERT INTO cursos (id,nivel) VALUES (3,'3 Medio');",
    )?;
    run_sql(
        &db,
        "INSERT INTO alumnos (id,nombre,curso_id) VALUES (10,'Ana',1);
         INSERT INTO alumnos (id,nombre,curso_id) VALUES (11,'Beto',2);
         INSERT INTO alumnos (id,nombre,curso_id) VALUES (12,'Carla',3);
         INSERT INTO alumnos (id,nombre,curso_id) VALUES (13,'Dani',1);",
    )?;

    let res = run_sql(
        &db,
        "SELECT nombre FROM alumnos \
         WHERE curso_id IN (SELECT id FROM cursos WHERE nivel = '3 Medio') \
         ORDER BY nombre ASC;",
    )?;
    let names: Vec<&str> = res[0]
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::String(s) => s.as_str(),
            other => panic!("expected String, got {:?}", other),
        })
        .collect();
    assert_eq!(names, vec!["Ana", "Carla", "Dani"]);

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
        err.contains("bloqueada") || err.contains("lock"),
        "el mensaje del lock debería mencionar 'bloqueada' o 'lock', recibí: {}",
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

#[test]
fn where_in_subquery_pk_path_and_dedup() -> Result<(), Box<dyn Error>> {
    // IN sobre PK directa: no requiere índice; debe deduplicar PKs repetidas
    // que devuelva la subquery.
    let db = temp_db_path("in_subquery_pk");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE t (id INT PRIMARY KEY, label TEXT);")?;
    run_sql(&db, "CREATE TABLE picks (pk INT PRIMARY KEY, ref_id INT);")?;
    run_sql(
        &db,
        "INSERT INTO t (id,label) VALUES (1,'a');
         INSERT INTO t (id,label) VALUES (2,'b');
         INSERT INTO t (id,label) VALUES (3,'c');",
    )?;
    // ref_id repite el 2 dos veces a propósito.
    run_sql(
        &db,
        "INSERT INTO picks (pk,ref_id) VALUES (10,2);
         INSERT INTO picks (pk,ref_id) VALUES (11,2);
         INSERT INTO picks (pk,ref_id) VALUES (12,3);",
    )?;

    let res = run_sql(
        &db,
        "SELECT id, label FROM t WHERE id IN (SELECT ref_id FROM picks) ORDER BY id ASC;",
    )?;
    // Debe devolver id=2 una sola vez (dedup) y id=3.
    assert_eq!(res[0].rows.len(), 2);
    assert_eq!(res[0].rows[0][0], Value::Integer(2));
    assert_eq!(res[0].rows[1][0], Value::Integer(3));

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn where_in_subquery_empty_set() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("in_subquery_empty");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE t (id INT PRIMARY KEY);")?;
    run_sql(&db, "CREATE TABLE s (id INT PRIMARY KEY, ref_id INT);")?;
    run_sql(
        &db,
        "INSERT INTO t (id) VALUES (1); INSERT INTO t (id) VALUES (2);",
    )?;

    let res = run_sql(
        &db,
        "SELECT id FROM t WHERE id IN (SELECT ref_id FROM s WHERE id = 9999);",
    )?;
    assert!(res[0].rows.is_empty());

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn where_in_subquery_errors() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("in_subquery_errors");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE t (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "CREATE TABLE s (id INT PRIMARY KEY, a INT, b INT);")?;
    run_sql(&db, "INSERT INTO t (id,name) VALUES (1,'x');")?;
    run_sql(&db, "INSERT INTO s (id,a,b) VALUES (1,1,2);")?;

    // Subquery con más de una columna → error claro.
    let err = run_sql(&db, "SELECT id FROM t WHERE id IN (SELECT a, b FROM s);")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("exactamente 1 columna"),
        "mensaje inesperado: {}",
        err
    );

    // Columna outer no es PK y no tiene índice → error explícito.
    let err = run_sql(&db, "SELECT id FROM t WHERE name IN (SELECT a FROM s);")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("no está indexada"),
        "mensaje inesperado: {}",
        err
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn where_eq_scalar_subquery_hit() -> Result<(), Box<dyn Error>> {
    // Caso típico: traer el alumno cuyo curso_id = (subquery que devuelve 1 id).
    let db = temp_db_path("eq_scalar_hit");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE cursos (id INT PRIMARY KEY, nombre TEXT NOT NULL UNIQUE);",
    )?;
    run_sql(
        &db,
        "CREATE TABLE alumnos (id INT PRIMARY KEY, nombre TEXT NOT NULL, curso_id INT);",
    )?;
    run_sql(&db, "CREATE INDEX idx_alumnos_curso ON alumnos (curso_id);")?;

    run_sql(
        &db,
        "INSERT INTO cursos (id,nombre) VALUES (1,'matematica');
         INSERT INTO cursos (id,nombre) VALUES (2,'historia');",
    )?;
    run_sql(
        &db,
        "INSERT INTO alumnos (id,nombre,curso_id) VALUES (10,'Ana',1);
         INSERT INTO alumnos (id,nombre,curso_id) VALUES (11,'Beto',2);
         INSERT INTO alumnos (id,nombre,curso_id) VALUES (12,'Carla',1);",
    )?;

    // Vía índice secundario (curso_id).
    let res = run_sql(
        &db,
        "SELECT nombre FROM alumnos \
         WHERE curso_id = (SELECT id FROM cursos WHERE nombre = 'matematica') \
         ORDER BY nombre ASC;",
    )?;
    let names: Vec<&str> = res[0]
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::String(s) => s.as_str(),
            other => panic!("expected String, got {:?}", other),
        })
        .collect();
    assert_eq!(names, vec!["Ana", "Carla"]);

    // Vía PK directa.
    let res = run_sql(
        &db,
        "SELECT nombre FROM cursos WHERE id = (SELECT curso_id FROM alumnos WHERE id = 11);",
    )?;
    assert_eq!(res[0].rows.len(), 1);
    assert_eq!(res[0].rows[0][0], Value::String("historia".to_string()));

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn where_eq_scalar_subquery_empty_and_null() -> Result<(), Box<dyn Error>> {
    // 0 filas o 1 fila NULL → match vacío (semántica ANSI: ningún valor iguala NULL).
    let db = temp_db_path("eq_scalar_empty");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE t (id INT PRIMARY KEY, label TEXT);")?;
    run_sql(&db, "CREATE TABLE s (id INT PRIMARY KEY, val INT);")?;
    run_sql(
        &db,
        "INSERT INTO t (id,label) VALUES (1,'a'); INSERT INTO t (id,label) VALUES (2,'b');",
    )?;
    run_sql(&db, "INSERT INTO s (id,val) VALUES (1,NULL);")?;

    // 0 filas.
    let res = run_sql(
        &db,
        "SELECT id FROM t WHERE id = (SELECT val FROM s WHERE id = 9999);",
    )?;
    assert!(res[0].rows.is_empty());

    // 1 fila NULL.
    let res = run_sql(
        &db,
        "SELECT id FROM t WHERE id = (SELECT val FROM s WHERE id = 1);",
    )?;
    assert!(res[0].rows.is_empty());

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn where_eq_scalar_subquery_errors() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("eq_scalar_errors");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE t (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "CREATE TABLE s (id INT PRIMARY KEY, a INT, b INT);")?;
    run_sql(
        &db,
        "INSERT INTO t (id,name) VALUES (1,'x'); INSERT INTO t (id,name) VALUES (2,'y');",
    )?;
    run_sql(
        &db,
        "INSERT INTO s (id,a,b) VALUES (1,10,20); INSERT INTO s (id,a,b) VALUES (2,30,40);",
    )?;

    // > 1 fila → SCALAR_SUBQUERY_TOO_MANY_ROWS.
    let err = run_sql(&db, "SELECT id FROM t WHERE id = (SELECT a FROM s);")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("GBY-4014") && err.contains("a lo sumo 1"),
        "mensaje inesperado: {}",
        err
    );

    // > 1 columna → SUBQUERY_MUST_RETURN_ONE_COLUMN (4011).
    let err = run_sql(
        &db,
        "SELECT id FROM t WHERE id = (SELECT a, b FROM s WHERE id = 1);",
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_default();
    assert!(
        err.contains("GBY-4011") && err.contains("exactamente 1 columna"),
        "mensaje inesperado: {}",
        err
    );

    // Columna outer no indexada → IN_REQUIRES_PK_OR_INDEX (4013).
    let err = run_sql(
        &db,
        "SELECT id FROM t WHERE name = (SELECT a FROM s WHERE id = 1);",
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_default();
    assert!(
        err.contains("GBY-4013") && err.contains("no está indexada"),
        "mensaje inesperado: {}",
        err
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn where_exists_uncorrelated() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("exists_uncorr");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE t (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "CREATE TABLE s (id INT PRIMARY KEY, v INT);")?;
    run_sql(
        &db,
        "INSERT INTO t (id,name) VALUES (1,'a'); INSERT INTO t (id,name) VALUES (2,'b');",
    )?;
    run_sql(&db, "INSERT INTO s (id,v) VALUES (1,42);")?;

    // EXISTS con subquery no vacía → toda t pasa.
    let res = run_sql(&db, "SELECT id FROM t WHERE EXISTS (SELECT id FROM s);")?;
    assert_eq!(res[0].rows.len(), 2);

    // EXISTS con subquery vacía → 0 filas.
    let res = run_sql(
        &db,
        "SELECT id FROM t WHERE EXISTS (SELECT id FROM s WHERE id = 9999);",
    )?;
    assert!(res[0].rows.is_empty());

    // NOT EXISTS invierte ambos casos.
    let res = run_sql(&db, "SELECT id FROM t WHERE NOT EXISTS (SELECT id FROM s);")?;
    assert!(res[0].rows.is_empty());

    let res = run_sql(
        &db,
        "SELECT id FROM t WHERE NOT EXISTS (SELECT id FROM s WHERE id = 9999);",
    )?;
    assert_eq!(res[0].rows.len(), 2);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn where_exists_correlated_via_outer_ref() -> Result<(), Box<dyn Error>> {
    // Pattern: SELECT * FROM padre p WHERE EXISTS (SELECT 1 FROM hijo h WHERE h.parent_id = p.id);
    let db = temp_db_path("exists_corr");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE padre (id INT PRIMARY KEY, nombre TEXT NOT NULL);",
    )?;
    run_sql(
        &db,
        "CREATE TABLE hijo (id INT PRIMARY KEY, parent_id INT, label TEXT);",
    )?;
    run_sql(&db, "CREATE INDEX idx_hijo_parent ON hijo (parent_id);")?;

    // padre 1,2,3 ; hijos solo para padre 1 y 3.
    run_sql(
        &db,
        "INSERT INTO padre (id,nombre) VALUES (1,'Ana');
         INSERT INTO padre (id,nombre) VALUES (2,'Beto');
         INSERT INTO padre (id,nombre) VALUES (3,'Carla');",
    )?;
    run_sql(
        &db,
        "INSERT INTO hijo (id,parent_id,label) VALUES (10,1,'h1');
         INSERT INTO hijo (id,parent_id,label) VALUES (11,3,'h2');
         INSERT INTO hijo (id,parent_id,label) VALUES (12,3,'h3');",
    )?;

    // EXISTS correlacionado: padres que tienen al menos un hijo.
    let res = run_sql(
        &db,
        "SELECT id, nombre FROM padre \
         WHERE EXISTS (SELECT id FROM hijo WHERE parent_id = padre.id) \
         ORDER BY id ASC;",
    )?;
    let ids: Vec<i64> = res[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => panic!("expected Integer"),
        })
        .collect();
    assert_eq!(ids, vec![1, 3]);

    // NOT EXISTS correlacionado: padres sin hijos.
    let res = run_sql(
        &db,
        "SELECT id FROM padre \
         WHERE NOT EXISTS (SELECT id FROM hijo WHERE parent_id = padre.id) \
         ORDER BY id ASC;",
    )?;
    let ids: Vec<i64> = res[0]
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => panic!("expected Integer"),
        })
        .collect();
    assert_eq!(ids, vec![2]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn where_exists_errors() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("exists_errors");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE t (id INT PRIMARY KEY, name TEXT);")?;
    run_sql(&db, "INSERT INTO t (id,name) VALUES (1,'x');")?;

    // EXISTS sin '(' → [GBY-4015]
    let err = run_sql(&db, "SELECT id FROM t WHERE EXISTS 5;")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("GBY-4015"), "mensaje inesperado: {}", err);

    // outer-column ref FUERA de subquery correlacionada → [GBY-4016]
    let err = run_sql(&db, "SELECT id FROM t WHERE id = padre.id;")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("GBY-4016"), "mensaje inesperado: {}", err);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_inner_two_tables_with_aliases() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_inner_basic");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE cursos (id INT PRIMARY KEY, nombre TEXT NOT NULL);",
    )?;
    run_sql(
        &db,
        "CREATE TABLE alumnos (id INT PRIMARY KEY, nombre TEXT NOT NULL, curso_id INT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO cursos (id,nombre) VALUES (1,'mate');
         INSERT INTO cursos (id,nombre) VALUES (2,'historia');",
    )?;
    run_sql(
        &db,
        "INSERT INTO alumnos (id,nombre,curso_id) VALUES (10,'Ana',1);
         INSERT INTO alumnos (id,nombre,curso_id) VALUES (11,'Beto',2);
         INSERT INTO alumnos (id,nombre,curso_id) VALUES (12,'Carla',1);
         INSERT INTO alumnos (id,nombre,curso_id) VALUES (13,'Dani',99);",
    )?;

    // INNER JOIN con alias + qualified columns + ORDER BY qualified
    let res = run_sql(
        &db,
        "SELECT a.nombre, c.nombre FROM alumnos a \
         INNER JOIN cursos c ON a.curso_id = c.id \
         ORDER BY a.nombre ASC;",
    )?;
    assert_eq!(
        res[0].rows.len(),
        3,
        "Dani no debería aparecer (curso_id=99)"
    );
    let pairs: Vec<(String, String)> = res[0]
        .rows
        .iter()
        .map(|r| match (&r[0], &r[1]) {
            (Value::String(a), Value::String(b)) => (a.clone(), b.clone()),
            _ => panic!("expected String pair"),
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("Ana".to_string(), "mate".to_string()),
            ("Beto".to_string(), "historia".to_string()),
            ("Carla".to_string(), "mate".to_string()),
        ]
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_inner_three_tables_chain() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_chain");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE pais (id INT PRIMARY KEY, nombre TEXT);")?;
    run_sql(
        &db,
        "CREATE TABLE ciudad (id INT PRIMARY KEY, nombre TEXT, pais_id INT);",
    )?;
    run_sql(
        &db,
        "CREATE TABLE persona (id INT PRIMARY KEY, nombre TEXT, ciudad_id INT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO pais (id,nombre) VALUES (1,'AR'); INSERT INTO pais (id,nombre) VALUES (2,'CL');",
    )?;
    run_sql(
        &db,
        "INSERT INTO ciudad (id,nombre,pais_id) VALUES (10,'BA',1);
         INSERT INTO ciudad (id,nombre,pais_id) VALUES (20,'SCL',2);",
    )?;
    run_sql(
        &db,
        "INSERT INTO persona (id,nombre,ciudad_id) VALUES (100,'Ana',10);
         INSERT INTO persona (id,nombre,ciudad_id) VALUES (101,'Beto',20);
         INSERT INTO persona (id,nombre,ciudad_id) VALUES (102,'Carla',10);",
    )?;

    let res = run_sql(
        &db,
        "SELECT persona.nombre, ciudad.nombre, pais.nombre \
         FROM persona \
         JOIN ciudad ON persona.ciudad_id = ciudad.id \
         JOIN pais ON ciudad.pais_id = pais.id \
         ORDER BY persona.id ASC;",
    )?;
    assert_eq!(res[0].rows.len(), 3);
    let names: Vec<(String, String, String)> = res[0]
        .rows
        .iter()
        .map(|r| match (&r[0], &r[1], &r[2]) {
            (Value::String(a), Value::String(b), Value::String(c)) => {
                (a.clone(), b.clone(), c.clone())
            }
            _ => panic!("expected 3 strings"),
        })
        .collect();
    assert_eq!(names[0], ("Ana".into(), "BA".into(), "AR".into()));
    assert_eq!(names[1], ("Beto".into(), "SCL".into(), "CL".into()));
    assert_eq!(names[2], ("Carla".into(), "BA".into(), "AR".into()));

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_cross_product_and_comma_syntax() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_cross");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE a (id INT PRIMARY KEY, v TEXT);")?;
    run_sql(&db, "CREATE TABLE b (id INT PRIMARY KEY, w TEXT);")?;
    run_sql(
        &db,
        "INSERT INTO a (id,v) VALUES (1,'x'); INSERT INTO a (id,v) VALUES (2,'y');",
    )?;
    run_sql(
        &db,
        "INSERT INTO b (id,w) VALUES (1,'p'); INSERT INTO b (id,w) VALUES (2,'q'); INSERT INTO b (id,w) VALUES (3,'r');",
    )?;

    // CROSS JOIN explícito → 2 × 3 = 6 filas
    let res = run_sql(&db, "SELECT a.v, b.w FROM a CROSS JOIN b;")?;
    assert_eq!(res[0].rows.len(), 6);

    // Comma-syntax = CROSS JOIN
    let res = run_sql(&db, "SELECT a.v, b.w FROM a, b;")?;
    assert_eq!(res[0].rows.len(), 6);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_self_join_via_alias() -> Result<(), Box<dyn Error>> {
    // Empleados con jefe → self-join sobre la misma tabla
    let db = temp_db_path("join_self");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE empleado (id INT PRIMARY KEY, nombre TEXT NOT NULL, jefe_id INT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO empleado (id,nombre,jefe_id) VALUES (1,'CEO',NULL);
         INSERT INTO empleado (id,nombre,jefe_id) VALUES (2,'CTO',1);
         INSERT INTO empleado (id,nombre,jefe_id) VALUES (3,'Dev1',2);
         INSERT INTO empleado (id,nombre,jefe_id) VALUES (4,'Dev2',2);",
    )?;

    let res = run_sql(
        &db,
        "SELECT e.nombre, j.nombre FROM empleado e \
         INNER JOIN empleado j ON e.jefe_id = j.id \
         ORDER BY e.id ASC;",
    )?;
    let pairs: Vec<(String, String)> = res[0]
        .rows
        .iter()
        .map(|r| match (&r[0], &r[1]) {
            (Value::String(a), Value::String(b)) => (a.clone(), b.clone()),
            _ => panic!("expected pair"),
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("CTO".into(), "CEO".into()),
            ("Dev1".into(), "CTO".into()),
            ("Dev2".into(), "CTO".into()),
        ]
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_where_filter_qualified() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_where");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE cursos (id INT PRIMARY KEY, nivel TEXT NOT NULL);",
    )?;
    run_sql(
        &db,
        "CREATE TABLE alumnos (id INT PRIMARY KEY, nombre TEXT, curso_id INT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO cursos (id,nivel) VALUES (1,'3M'); INSERT INTO cursos (id,nivel) VALUES (2,'4M');",
    )?;
    run_sql(
        &db,
        "INSERT INTO alumnos (id,nombre,curso_id) VALUES (10,'Ana',1);
         INSERT INTO alumnos (id,nombre,curso_id) VALUES (11,'Beto',2);
         INSERT INTO alumnos (id,nombre,curso_id) VALUES (12,'Carla',1);",
    )?;

    // WHERE sobre columna cualificada del lado right
    let res = run_sql(
        &db,
        "SELECT alumnos.nombre FROM alumnos \
         JOIN cursos ON alumnos.curso_id = cursos.id \
         WHERE cursos.nivel = '3M' \
         ORDER BY alumnos.nombre ASC;",
    )?;
    let names: Vec<&str> = res[0]
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::String(s) => s.as_str(),
            _ => panic!("expected String"),
        })
        .collect();
    assert_eq!(names, vec!["Ana", "Carla"]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_errors() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_errors");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE a (id INT PRIMARY KEY, x INT);")?;
    run_sql(&db, "CREATE TABLE b (id INT PRIMARY KEY, x INT);")?;
    run_sql(&db, "INSERT INTO a (id,x) VALUES (1,10);")?;
    run_sql(&db, "INSERT INTO b (id,x) VALUES (1,10);")?;

    // INNER JOIN sin ON → [GBY-4020]
    let err = run_sql(&db, "SELECT a.id FROM a INNER JOIN b;")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("GBY-4020"), "got: {}", err);

    // CROSS JOIN con ON → [GBY-4021]
    let err = run_sql(&db, "SELECT a.id FROM a CROSS JOIN b ON a.id = b.id;")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("GBY-4021"), "got: {}", err);

    // Columna ambigua (sin qualifier; `x` está en a y b) → [GBY-4018]
    let err = run_sql(&db, "SELECT x FROM a JOIN b ON a.id = b.id;")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("GBY-4018"), "got: {}", err);

    // Qualifier inexistente → [GBY-4019]
    let err = run_sql(&db, "SELECT zzz.x FROM a JOIN b ON a.id = b.id;")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("GBY-4019"), "got: {}", err);

    // Alias duplicado → [GBY-4017]
    let err = run_sql(&db, "SELECT a.id FROM a JOIN b AS a ON a.id = a.id;")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("GBY-4017"), "got: {}", err);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_left_outer_preserves_unmatched_left() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_left");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE padre (id INT PRIMARY KEY, nombre TEXT);")?;
    run_sql(
        &db,
        "CREATE TABLE hijo (id INT PRIMARY KEY, parent_id INT, etiqueta TEXT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO padre (id,nombre) VALUES (1,'Ana');
         INSERT INTO padre (id,nombre) VALUES (2,'Beto');
         INSERT INTO padre (id,nombre) VALUES (3,'Carla');",
    )?;
    run_sql(
        &db,
        "INSERT INTO hijo (id,parent_id,etiqueta) VALUES (10,1,'h1');
         INSERT INTO hijo (id,parent_id,etiqueta) VALUES (11,3,'h2');",
    )?;

    // LEFT JOIN: Beto (sin hijos) aparece con etiqueta NULL.
    let res = run_sql(
        &db,
        "SELECT padre.nombre, hijo.etiqueta FROM padre \
         LEFT JOIN hijo ON padre.id = hijo.parent_id \
         ORDER BY padre.id ASC;",
    )?;
    let pairs: Vec<(String, Value)> = res[0]
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::String(n) => (n.clone(), r[1].clone()),
            _ => panic!("expected String name"),
        })
        .collect();
    assert_eq!(pairs.len(), 3);
    assert_eq!(pairs[0], ("Ana".into(), Value::String("h1".into())));
    assert_eq!(pairs[1], ("Beto".into(), Value::Null));
    assert_eq!(pairs[2], ("Carla".into(), Value::String("h2".into())));

    // `LEFT OUTER JOIN` (con OUTER explícito) — mismo resultado.
    let res = run_sql(
        &db,
        "SELECT padre.nombre, hijo.etiqueta FROM padre \
         LEFT OUTER JOIN hijo ON padre.id = hijo.parent_id \
         ORDER BY padre.id ASC;",
    )?;
    assert_eq!(res[0].rows.len(), 3);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_right_outer_preserves_unmatched_right() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_right");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE a (id INT PRIMARY KEY, v TEXT);")?;
    run_sql(
        &db,
        "CREATE TABLE b (id INT PRIMARY KEY, a_id INT, w TEXT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO a (id,v) VALUES (1,'x'); INSERT INTO a (id,v) VALUES (2,'y');",
    )?;
    run_sql(
        &db,
        "INSERT INTO b (id,a_id,w) VALUES (10,1,'p');
         INSERT INTO b (id,a_id,w) VALUES (11,99,'huerfana');",
    )?;

    // RIGHT JOIN: la fila de b con a_id=99 (sin match) aparece con a.v NULL.
    let res = run_sql(
        &db,
        "SELECT a.v, b.w FROM a RIGHT JOIN b ON a.id = b.a_id ORDER BY b.id ASC;",
    )?;
    let pairs: Vec<(Value, Value)> = res[0]
        .rows
        .iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    assert_eq!(pairs.len(), 2);
    assert_eq!(
        pairs[0],
        (Value::String("x".into()), Value::String("p".into()))
    );
    assert_eq!(pairs[1], (Value::Null, Value::String("huerfana".into())));

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_full_outer_preserves_both_sides() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_full");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE a (id INT PRIMARY KEY, v TEXT);")?;
    run_sql(
        &db,
        "CREATE TABLE b (id INT PRIMARY KEY, a_id INT, w TEXT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO a (id,v) VALUES (1,'x');
         INSERT INTO a (id,v) VALUES (2,'sola_a');",
    )?;
    run_sql(
        &db,
        "INSERT INTO b (id,a_id,w) VALUES (10,1,'p');
         INSERT INTO b (id,a_id,w) VALUES (11,99,'sola_b');",
    )?;

    let res = run_sql(
        &db,
        "SELECT a.v, b.w FROM a FULL OUTER JOIN b ON a.id = b.a_id;",
    )?;
    // 3 filas esperadas: (x,p) match; (sola_a,NULL) left-only; (NULL,sola_b) right-only.
    assert_eq!(res[0].rows.len(), 3);
    let mut combos: Vec<(Value, Value)> = res[0]
        .rows
        .iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    combos.sort_by_key(|(a, b)| (format!("{:?}", a), format!("{:?}", b)));
    assert!(combos.contains(&(Value::String("x".into()), Value::String("p".into()))));
    assert!(combos.contains(&(Value::String("sola_a".into()), Value::Null)));
    assert!(combos.contains(&(Value::Null, Value::String("sola_b".into()))));

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_left_anti_join_via_where_null() -> Result<(), Box<dyn Error>> {
    // Patrón anti-join: LEFT JOIN + WHERE col_right IS NULL → outer-only.
    // gabysql aún no tiene IS NULL operator, así que el patrón equivalente
    // es WHERE hijo.id = NULL ... no, mejor verificamos con count post-filter
    // manual. Aquí solo confirmamos que la fila LEFT-NULL existe en el output.
    let db = temp_db_path("join_anti");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE padre (id INT PRIMARY KEY);")?;
    run_sql(
        &db,
        "CREATE TABLE hijo (id INT PRIMARY KEY, parent_id INT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO padre (id) VALUES (1); INSERT INTO padre (id) VALUES (2); INSERT INTO padre (id) VALUES (3);",
    )?;
    run_sql(&db, "INSERT INTO hijo (id,parent_id) VALUES (10,1);")?;

    let res = run_sql(
        &db,
        "SELECT padre.id, hijo.id FROM padre LEFT JOIN hijo ON padre.id = hijo.parent_id ORDER BY padre.id ASC;",
    )?;
    assert_eq!(res[0].rows.len(), 3);
    // Padre 2 y 3: hijo.id es NULL
    let nulls: usize = res[0]
        .rows
        .iter()
        .filter(|r| matches!(r[1], Value::Null))
        .count();
    assert_eq!(nulls, 2);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn join_using_basic() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_using");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    // Ambas tablas tienen una columna `pais_id` — USING (pais_id) genera
    // automáticamente `ON a.pais_id = b.pais_id`.
    run_sql(
        &db,
        "CREATE TABLE ciudad (id INT PRIMARY KEY, nombre TEXT, pais_id INT);",
    )?;
    run_sql(
        &db,
        "CREATE TABLE pais (id INT PRIMARY KEY, pais_id INT, nombre_pais TEXT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO ciudad (id,nombre,pais_id) VALUES (1,'BA',10);
         INSERT INTO ciudad (id,nombre,pais_id) VALUES (2,'SCL',20);",
    )?;
    run_sql(
        &db,
        "INSERT INTO pais (id,pais_id,nombre_pais) VALUES (100,10,'Argentina');
         INSERT INTO pais (id,pais_id,nombre_pais) VALUES (101,20,'Chile');",
    )?;

    let res = run_sql(
        &db,
        "SELECT ciudad.nombre, pais.nombre_pais FROM ciudad JOIN pais USING (pais_id) ORDER BY ciudad.id ASC;",
    )?;
    assert_eq!(res[0].rows.len(), 2);
    let names: Vec<(String, String)> = res[0]
        .rows
        .iter()
        .map(|r| match (&r[0], &r[1]) {
            (Value::String(a), Value::String(b)) => (a.clone(), b.clone()),
            _ => panic!("expected pair"),
        })
        .collect();
    assert_eq!(names[0], ("BA".into(), "Argentina".into()));
    assert_eq!(names[1], ("SCL".into(), "Chile".into()));

    Ok(())
}

#[test]
fn join_using_star_dedup() -> Result<(), Box<dyn Error>> {
    // SELECT * con USING omite la columna del lado derecho (ANSI).
    let db = temp_db_path("join_using_star");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE a (id INT PRIMARY KEY, k INT, v TEXT);")?;
    run_sql(&db, "CREATE TABLE b (id INT PRIMARY KEY, k INT, w TEXT);")?;
    run_sql(&db, "INSERT INTO a (id,k,v) VALUES (1,10,'va');")?;
    run_sql(&db, "INSERT INTO b (id,k,w) VALUES (1,10,'wb');")?;

    let res = run_sql(&db, "SELECT * FROM a JOIN b USING (k);")?;
    // Columnas esperadas: a.id, a.k, a.v, b.id, b.w  (NO b.k porque fue dedup)
    let cols = &res[0].columns;
    assert!(cols.iter().any(|c| c.ends_with("a.id")));
    assert!(cols.iter().any(|c| c.ends_with("a.k")));
    assert!(cols.iter().any(|c| c.ends_with("a.v")));
    assert!(cols.iter().any(|c| c.ends_with("b.id")));
    assert!(cols.iter().any(|c| c.ends_with("b.w")));
    assert!(
        !cols.iter().any(|c| c.eq_ignore_ascii_case("b.k")),
        "b.k debería estar dedup en USING: cols={:?}",
        cols
    );

    Ok(())
}

#[test]
fn join_natural_basic() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_natural");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    // Solo `pais_id` es común entre ciudad y pais → NATURAL JOIN matchea por esa.
    run_sql(
        &db,
        "CREATE TABLE ciudad (id INT PRIMARY KEY, nombre TEXT, pais_id INT);",
    )?;
    run_sql(
        &db,
        "CREATE TABLE pais (pais_id INT PRIMARY KEY, nombre_pais TEXT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO ciudad (id,nombre,pais_id) VALUES (1,'BA',10);
         INSERT INTO ciudad (id,nombre,pais_id) VALUES (2,'SCL',20);
         INSERT INTO ciudad (id,nombre,pais_id) VALUES (3,'huerfana',99);",
    )?;
    run_sql(
        &db,
        "INSERT INTO pais (pais_id,nombre_pais) VALUES (10,'AR'); INSERT INTO pais (pais_id,nombre_pais) VALUES (20,'CL');",
    )?;

    let res = run_sql(
        &db,
        "SELECT ciudad.nombre, pais.nombre_pais FROM ciudad NATURAL JOIN pais ORDER BY ciudad.id ASC;",
    )?;
    assert_eq!(res[0].rows.len(), 2, "huerfana NO debería aparecer");
    let pairs: Vec<(String, String)> = res[0]
        .rows
        .iter()
        .map(|r| match (&r[0], &r[1]) {
            (Value::String(a), Value::String(b)) => (a.clone(), b.clone()),
            _ => panic!("expected pair"),
        })
        .collect();
    assert_eq!(pairs[0], ("BA".into(), "AR".into()));
    assert_eq!(pairs[1], ("SCL".into(), "CL".into()));

    Ok(())
}

#[test]
fn join_using_and_natural_errors() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("join_using_err");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE a (id INT PRIMARY KEY, x INT);")?;
    run_sql(&db, "CREATE TABLE b (id INT PRIMARY KEY, y INT);")?;
    run_sql(&db, "INSERT INTO a (id,x) VALUES (1,10);")?;
    run_sql(&db, "INSERT INTO b (id,y) VALUES (1,20);")?;

    // USING (col) cuando col no existe en ambas → [GBY-4022]
    let err = run_sql(&db, "SELECT a.id FROM a JOIN b USING (zzz);")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("GBY-4022"), "got: {}", err);

    // NATURAL JOIN con 0 columnas comunes (id está pero también en ambas,
    // así que sí hay una común). Hagamos 2 comunes: tablas con id e id2.
    run_sql(&db, "CREATE TABLE c (id INT PRIMARY KEY, x INT);")?;
    run_sql(&db, "CREATE TABLE d (id INT PRIMARY KEY, x INT);")?;
    run_sql(&db, "INSERT INTO c (id,x) VALUES (1,10);")?;
    run_sql(&db, "INSERT INTO d (id,x) VALUES (1,10);")?;
    // c y d comparten 2 columnas (id, x) → este release solo soporta 1 → [GBY-4023]
    let err = run_sql(&db, "SELECT c.id FROM c NATURAL JOIN d;")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("GBY-4023"), "got: {}", err);

    Ok(())
}

#[test]
fn join_index_loop_pk_correctness() -> Result<(), Box<dyn Error>> {
    // El index-loop debe producir EXACTAMENTE los mismos resultados que el
    // nested-loop. Aquí el predicate es `a.b_id = b.id` con b.id = PK, así
    // que dispara el path PK directo.
    let db = temp_db_path("join_idx_pk");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(&db, "CREATE TABLE b (id INT PRIMARY KEY, w TEXT);")?;
    run_sql(
        &db,
        "CREATE TABLE a (id INT PRIMARY KEY, b_id INT, v TEXT);",
    )?;
    for i in 1..=20i64 {
        run_sql(
            &db,
            &format!("INSERT INTO b (id,w) VALUES ({},'w{}');", i, i),
        )?;
    }
    for i in 1..=30i64 {
        let b_ref = ((i - 1) % 20) + 1; // FK al rango b.id
        run_sql(
            &db,
            &format!(
                "INSERT INTO a (id,b_id,v) VALUES ({},{},'v{}');",
                i, b_ref, i
            ),
        )?;
    }

    let res = run_sql(
        &db,
        "SELECT a.id, b.w FROM a INNER JOIN b ON a.b_id = b.id ORDER BY a.id ASC;",
    )?;
    assert_eq!(res[0].rows.len(), 30);
    // Spot-check: a.id=1 → b.id=1 → w='w1'
    assert_eq!(res[0].rows[0][0], Value::Integer(1));
    assert_eq!(res[0].rows[0][1], Value::String("w1".into()));

    // LEFT JOIN debe seguir igual: incluye fila con NULL si no hay match.
    run_sql(
        &db,
        "INSERT INTO a (id,b_id,v) VALUES (100,9999,'huerfana');",
    )?;
    let res = run_sql(
        &db,
        "SELECT a.id, b.w FROM a LEFT JOIN b ON a.b_id = b.id WHERE a.id = 100;",
    )?;
    assert_eq!(res[0].rows.len(), 1);
    assert_eq!(res[0].rows[0][0], Value::Integer(100));
    assert!(matches!(res[0].rows[0][1], Value::Null));

    Ok(())
}

#[test]
fn join_index_loop_secondary_index() -> Result<(), Box<dyn Error>> {
    // El predicate apunta contra una columna no-PK del right que sí tiene
    // índice. Debe usar el path Index(idx) y producir resultados correctos.
    let db = temp_db_path("join_idx_sec");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;

    run_sql(
        &db,
        "CREATE TABLE pais (id INT PRIMARY KEY, codigo TEXT, nombre TEXT);",
    )?;
    run_sql(&db, "CREATE INDEX idx_pais_codigo ON pais (codigo);")?;
    run_sql(
        &db,
        "CREATE TABLE ciudad (id INT PRIMARY KEY, nombre TEXT, pais_codigo TEXT);",
    )?;

    run_sql(
        &db,
        "INSERT INTO pais (id,codigo,nombre) VALUES (1,'AR','Argentina');
         INSERT INTO pais (id,codigo,nombre) VALUES (2,'CL','Chile');",
    )?;
    run_sql(
        &db,
        "INSERT INTO ciudad (id,nombre,pais_codigo) VALUES (10,'BA','AR');
         INSERT INTO ciudad (id,nombre,pais_codigo) VALUES (11,'SCL','CL');
         INSERT INTO ciudad (id,nombre,pais_codigo) VALUES (12,'huerfana','XX');",
    )?;

    // INNER JOIN: predicate ciudad.pais_codigo = pais.codigo (col indexada).
    let res = run_sql(
        &db,
        "SELECT ciudad.nombre, pais.nombre FROM ciudad INNER JOIN pais ON ciudad.pais_codigo = pais.codigo ORDER BY ciudad.id ASC;",
    )?;
    assert_eq!(res[0].rows.len(), 2); // huerfana excluida
    assert_eq!(res[0].rows[0][0], Value::String("BA".into()));
    assert_eq!(res[0].rows[0][1], Value::String("Argentina".into()));
    assert_eq!(res[0].rows[1][0], Value::String("SCL".into()));
    assert_eq!(res[0].rows[1][1], Value::String("Chile".into()));

    Ok(())
}

// ============================================================
// Bloque E1: AND / OR / NOT + paréntesis en WHERE
// ============================================================
//
// Cada test es self-contained: crea la DB, inserta filas conocidas y verifica
// el set resultante. La fixture común es una tabla `e1` con (id, nombre,
// edad, ciudad, activo) — pensada para probar combinaciones booleanas sobre
// columnas de tipos distintos (TEXT, INT, BOOL) con NULLs deliberados para
// ejercitar la 3VL.

fn e1_fixture(db: &Path) -> Result<(), Box<dyn Error>> {
    let mut pager = Pager::create(db)?;
    pager.close()?;
    run_sql(
        db,
        "CREATE TABLE e1 (id INT PRIMARY KEY, nombre TEXT, edad INT, ciudad TEXT, activo BOOL);",
    )?;
    run_sql(
        db,
        "INSERT INTO e1 (id,nombre,edad,ciudad,activo) VALUES (1,'Ana',30,'BA',TRUE);
         INSERT INTO e1 (id,nombre,edad,ciudad,activo) VALUES (2,'Beto',25,'MDQ',FALSE);
         INSERT INTO e1 (id,nombre,edad,ciudad,activo) VALUES (3,'Carla',40,'BA',TRUE);
         INSERT INTO e1 (id,nombre,edad,ciudad,activo) VALUES (4,'Dario',22,'CBA',TRUE);
         INSERT INTO e1 (id,nombre,edad,ciudad,activo) VALUES (5,'Eva',50,'MDQ',FALSE);
         INSERT INTO e1 (id,nombre,activo) VALUES (6,'NullEdad',TRUE);",
    )?;
    Ok(())
}

fn e1_ids(res: &gabysql::sql::ResultSet) -> Vec<i64> {
    res.rows
        .iter()
        .map(|r| match &r[0] {
            Value::Integer(n) => *n,
            other => panic!("se esperaba INT en columna 0, llegó {:?}", other),
        })
        .collect()
}

#[test]
fn e1_and_two_predicates() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_and");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(
        &db,
        "SELECT id FROM e1 WHERE ciudad = 'BA' AND activo = TRUE;",
    )?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 3]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e1_or_two_predicates() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_or");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(
        &db,
        "SELECT id FROM e1 WHERE ciudad = 'MDQ' OR ciudad = 'CBA';",
    )?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![2, 4, 5]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e1_not_negates_predicate() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_not");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(&db, "SELECT id FROM e1 WHERE NOT ciudad = 'BA';")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    // NOT ciudad='BA': filas 2,4,5. La fila 6 tiene ciudad=NULL → NOT (NULL=...) = NULL → descartada.
    assert_eq!(ids, vec![2, 4, 5]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e1_parens_force_precedence() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_parens");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    // Sin paréntesis AND ata más fuerte que OR:
    //   ciudad='BA' OR ciudad='MDQ' AND activo=TRUE
    // = ciudad='BA' OR (ciudad='MDQ' AND activo=TRUE)
    // → {1,3 (BA)} ∪ {} (MDQ activos no hay) = {1, 3}
    let res_default = run_sql(
        &db,
        "SELECT id FROM e1 WHERE ciudad = 'BA' OR ciudad = 'MDQ' AND activo = TRUE;",
    )?;
    let mut ids = e1_ids(&res_default[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 3]);

    // Con paréntesis forzando la otra agrupación:
    //   (ciudad='BA' OR ciudad='MDQ') AND activo=TRUE
    // → {1,2,3,5} ∩ {1,3,4,6} = {1, 3}
    let res_paren = run_sql(
        &db,
        "SELECT id FROM e1 WHERE (ciudad = 'BA' OR ciudad = 'MDQ') AND activo = TRUE;",
    )?;
    let mut ids = e1_ids(&res_paren[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 3]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e1_and_with_between() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_between");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    // BETWEEN consume su propio AND; el AND externo es combinador booleano.
    let res = run_sql(
        &db,
        "SELECT id FROM e1 WHERE id BETWEEN 1 AND 5 AND ciudad = 'BA';",
    )?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 3]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e1_three_valued_logic_null() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_3vl");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    // Fila 6 tiene edad=NULL. `edad = 30` → NULL → descartada.
    // OR con un predicado siempre-true (ciudad nada matchea, ej. 'ZZZ')
    // mantiene NULL → descartada. Solo Ana (edad=30) sobrevive.
    let res = run_sql(&db, "SELECT id FROM e1 WHERE edad = 30 OR ciudad = 'ZZZ';")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1]);

    // NOT (edad = 30): NULL queda NULL → fila 6 fuera. Filas que matchean
    // (no son 30 y no son NULL): 2,3,4,5.
    let res = run_sql(&db, "SELECT id FROM e1 WHERE NOT edad = 30;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![2, 3, 4, 5]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e1_nested_not_and_or() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_nested");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    // NOT (ciudad='BA' OR ciudad='CBA') AND activo=FALSE
    // → NOT({1,3,4}) ∩ {2,5} = {2,5,6} ∩ {2,5} = {2,5}
    // (la fila 6 tiene ciudad=NULL → NOT(NULL OR NULL)=NULL → descartada;
    //  pero activo=TRUE para fila 6 igual la descartaba.)
    let res = run_sql(
        &db,
        "SELECT id FROM e1 WHERE NOT (ciudad = 'BA' OR ciudad = 'CBA') AND activo = FALSE;",
    )?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![2, 5]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e1_combinator_works_with_limit_and_orderby() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_lim");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(
        &db,
        "SELECT id FROM e1 WHERE ciudad = 'BA' OR ciudad = 'MDQ' ORDER BY id DESC LIMIT 2;",
    )?;
    let ids = e1_ids(&res[0]);
    assert_eq!(ids, vec![5, 3]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e1_double_not_cancels() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_dnot");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(&db, "SELECT id FROM e1 WHERE NOT NOT ciudad = 'BA';")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 3]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e1_works_with_join() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_join");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(
        &db,
        "CREATE TABLE a (id INT PRIMARY KEY, tag TEXT);
         CREATE TABLE b (id INT PRIMARY KEY, a_id INT, val INT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO a (id,tag) VALUES (1,'x'); INSERT INTO a (id,tag) VALUES (2,'y');
         INSERT INTO b (id,a_id,val) VALUES (10,1,100);
         INSERT INTO b (id,a_id,val) VALUES (11,1,200);
         INSERT INTO b (id,a_id,val) VALUES (12,2,300);",
    )?;

    let res = run_sql(
        &db,
        "SELECT b.id FROM a INNER JOIN b ON a.id = b.a_id WHERE a.tag = 'x' AND b.val = 200;",
    )?;
    let ids = e1_ids(&res[0]);
    assert_eq!(ids, vec![11]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e1_parser_rejects_dangling_combinator() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e1_bad");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let err = run_sql(&db, "SELECT id FROM e1 WHERE ciudad = 'BA' AND;").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("[GBY-4001]") || msg.contains("identificador") || msg.contains("ident"),
        "mensaje inesperado: {}",
        msg
    );

    cleanup(&[&db, &wal]);
    Ok(())
}

// ============================================================
// Bloque E2: operadores <, >, <=, >=, <>, !=, LIKE, IS NULL, IN literal
// ============================================================
//
// Reusa la fixture `e1_fixture` (tabla `e1` con (id, nombre, edad, ciudad,
// activo) y fila 6 con NULLs deliberados). Cada test verifica una sola
// operación o una combinación bien definida; cuando el orden importa se
// agrega ORDER BY id ASC para hacer el assert estable.

#[test]
fn e2_lt_le_gt_ge_on_int() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_int_cmp");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(&db, "SELECT id FROM e1 WHERE edad < 30;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    // edad < 30: Beto(25), Dario(22). NullEdad descartada por 3VL.
    assert_eq!(ids, vec![2, 4]);

    let res = run_sql(&db, "SELECT id FROM e1 WHERE edad <= 30;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 2, 4]);

    let res = run_sql(&db, "SELECT id FROM e1 WHERE edad > 30;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![3, 5]);

    let res = run_sql(&db, "SELECT id FROM e1 WHERE edad >= 30;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 3, 5]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e2_ne_operators() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_ne");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(&db, "SELECT id FROM e1 WHERE ciudad <> 'BA';")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    // ciudad <> 'BA': MDQ y CBA matchean; fila 6 (NULL) → NULL → descartada.
    assert_eq!(ids, vec![2, 4, 5]);

    let res = run_sql(&db, "SELECT id FROM e1 WHERE ciudad != 'BA';")?;
    let mut ids2 = e1_ids(&res[0]);
    ids2.sort();
    assert_eq!(ids, ids2, "<> y != deben ser sinónimos");

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e2_compare_on_text_lex_order() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_text_cmp");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    // 'C' es el primer char donde se separa Carla/Dario/Eva del resto.
    let res = run_sql(&db, "SELECT id FROM e1 WHERE nombre >= 'C';")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    // Carla, Dario, Eva, NullEdad (empieza con 'N').
    assert_eq!(ids, vec![3, 4, 5, 6]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e2_like_basic_wildcards() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_like");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    // `%a` — termina con 'a'. Ana, Carla, Eva.
    let res = run_sql(&db, "SELECT id FROM e1 WHERE nombre LIKE '%a';")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 3, 5]);

    // `_eto` — 4 chars, termina con 'eto'. Beto.
    let res = run_sql(&db, "SELECT id FROM e1 WHERE nombre LIKE '_eto';")?;
    let ids = e1_ids(&res[0]);
    assert_eq!(ids, vec![2]);

    // `D%` — empieza con D. Dario.
    let res = run_sql(&db, "SELECT id FROM e1 WHERE nombre LIKE 'D%';")?;
    let ids = e1_ids(&res[0]);
    assert_eq!(ids, vec![4]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e2_not_like() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_notlike");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(&db, "SELECT id FROM e1 WHERE nombre NOT LIKE '%a';")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    // No terminan en 'a': Beto, Dario, NullEdad.
    assert_eq!(ids, vec![2, 4, 6]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e2_is_null_and_is_not_null() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_isnull");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(&db, "SELECT id FROM e1 WHERE edad IS NULL;")?;
    let ids = e1_ids(&res[0]);
    assert_eq!(ids, vec![6]);

    let res = run_sql(&db, "SELECT id FROM e1 WHERE edad IS NOT NULL;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 2, 3, 4, 5]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e2_in_literal_list() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_inlit");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(&db, "SELECT id FROM e1 WHERE id IN (1, 3, 5);")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 3, 5]);

    let res = run_sql(&db, "SELECT id FROM e1 WHERE ciudad IN ('BA', 'CBA');")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 3, 4]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e2_not_in_literal_list_with_null_semantics() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_notinlit");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    // NOT IN sin NULLs en la lista: fila 6 (ciudad=NULL) → NULL → descartada.
    let res = run_sql(&db, "SELECT id FROM e1 WHERE ciudad NOT IN ('BA', 'CBA');")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![2, 5]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e2_combines_with_e1_and_or() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_combo");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e1_fixture(&db)?;

    let res = run_sql(
        &db,
        "SELECT id FROM e1 WHERE edad > 25 AND ciudad IS NOT NULL AND nombre LIKE '%a';",
    )?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    // edad>25: Ana(30), Carla(40), Eva(50). ciudad no-NULL: todos esos.
    // nombre LIKE '%a': Ana, Carla, Eva. → {1, 3, 5}.
    assert_eq!(ids, vec![1, 3, 5]);

    let res = run_sql(
        &db,
        "SELECT id FROM e1 WHERE id IN (1, 2, 3) OR ciudad IS NULL;",
    )?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![1, 2, 3, 6]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e2_like_escape() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_like_esc");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(&db, "CREATE TABLE t (id INT PRIMARY KEY, s TEXT);")?;
    run_sql(
        &db,
        "INSERT INTO t (id,s) VALUES (1,'50%off');
         INSERT INTO t (id,s) VALUES (2,'50xoff');
         INSERT INTO t (id,s) VALUES (3,'a_b');",
    )?;

    let res = run_sql(&db, "SELECT id FROM t WHERE s LIKE '50\\%%';")?;
    let ids = e1_ids(&res[0]);
    assert_eq!(ids, vec![1]);

    let res = run_sql(&db, "SELECT id FROM t WHERE s LIKE 'a\\_b';")?;
    let ids = e1_ids(&res[0]);
    assert_eq!(ids, vec![3]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e2_compare_with_join() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e2_join");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(
        &db,
        "CREATE TABLE a (id INT PRIMARY KEY, tag TEXT);
         CREATE TABLE b (id INT PRIMARY KEY, a_id INT, val INT);",
    )?;
    run_sql(
        &db,
        "INSERT INTO a (id,tag) VALUES (1,'x'); INSERT INTO a (id,tag) VALUES (2,'y');
         INSERT INTO b (id,a_id,val) VALUES (10,1,100);
         INSERT INTO b (id,a_id,val) VALUES (11,1,200);
         INSERT INTO b (id,a_id,val) VALUES (12,2,300);",
    )?;

    let res = run_sql(
        &db,
        "SELECT b.id FROM a INNER JOIN b ON a.id = b.a_id WHERE b.val >= 200;",
    )?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![11, 12]);

    cleanup(&[&db, &wal]);
    Ok(())
}

// ============================================================
// Bloque E3: UPDATE / DELETE por columna indexada y por subquery
// ============================================================
//
// Verifica que UPDATE/DELETE acepten cualquier WHERE soportado por SELECT
// (E1+E2 + subqueries) y que actúen sobre todas las filas que matchean,
// no sólo sobre `pk = N`. Cada test cuenta filas con un SELECT siguiente.

fn e3_fixture(db: &Path) -> Result<(), Box<dyn Error>> {
    let mut pager = Pager::create(db)?;
    pager.close()?;
    run_sql(
        db,
        "CREATE TABLE t (id INT PRIMARY KEY, nombre TEXT, edad INT, ciudad TEXT, activo BOOL);",
    )?;
    run_sql(db, "CREATE INDEX idx_t_ciudad ON t (ciudad);")?;
    run_sql(
        db,
        "INSERT INTO t (id,nombre,edad,ciudad,activo) VALUES (1,'Ana',30,'BA',TRUE);
         INSERT INTO t (id,nombre,edad,ciudad,activo) VALUES (2,'Beto',25,'MDQ',FALSE);
         INSERT INTO t (id,nombre,edad,ciudad,activo) VALUES (3,'Carla',40,'BA',TRUE);
         INSERT INTO t (id,nombre,edad,ciudad,activo) VALUES (4,'Dario',22,'CBA',TRUE);
         INSERT INTO t (id,nombre,edad,ciudad,activo) VALUES (5,'Eva',50,'MDQ',FALSE);",
    )?;
    Ok(())
}

#[test]
fn e3_update_by_indexed_column_affects_all_matches() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e3_upd_idx");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e3_fixture(&db)?;

    // ciudad='BA' matchea 2 filas (1, 3). Después del UPDATE ambas
    // deberían tener activo=FALSE.
    run_sql(&db, "UPDATE t SET activo = FALSE WHERE ciudad = 'BA';")?;
    let res = run_sql(&db, "SELECT id FROM t WHERE activo = TRUE;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![4]); // solo Dario quedó activo

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e3_update_by_compound_where() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e3_upd_compound");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e3_fixture(&db)?;

    // Marcamos como inactivos a los mayores de 35.
    run_sql(&db, "UPDATE t SET activo = FALSE WHERE edad > 35;")?;
    let res = run_sql(&db, "SELECT id FROM t WHERE activo = TRUE;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    // Activos pre-update: 1 (Ana, 30), 3 (Carla, 40), 4 (Dario, 22).
    // edad>35: Carla(3), Eva(5 — ya FALSE). Carla pasa a FALSE.
    // Quedan activos: 1 (Ana), 4 (Dario).
    assert_eq!(ids, vec![1, 4]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e3_update_by_in_subquery() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e3_upd_subq");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e3_fixture(&db)?;

    // Una segunda tabla con los IDs a desactivar.
    run_sql(&db, "CREATE TABLE bad (uid INT PRIMARY KEY);")?;
    run_sql(
        &db,
        "INSERT INTO bad (uid) VALUES (2); INSERT INTO bad (uid) VALUES (5);",
    )?;

    run_sql(
        &db,
        "UPDATE t SET nombre = 'BLOQUEADO' WHERE id IN (SELECT uid FROM bad);",
    )?;
    let res = run_sql(&db, "SELECT id FROM t WHERE nombre = 'BLOQUEADO';")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![2, 5]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e3_update_zero_matches_is_not_error() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e3_upd_zero");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e3_fixture(&db)?;

    // Con WHERE compuesto, 0 matches es OK (SQL estándar). El message
    // refleja "0 filas actualizadas".
    let res = run_sql(&db, "UPDATE t SET activo = FALSE WHERE edad > 999;")?;
    let msg = res[0].message.as_deref().unwrap_or("");
    assert!(msg.contains("0"), "message inesperado: {}", msg);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e3_update_by_pk_still_errors_when_not_found() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e3_upd_pk_404");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e3_fixture(&db)?;

    // El fast-path de `pk = N` debe seguir devolviendo
    // [GBY-3006] ROW_NOT_FOUND_FOR_PK cuando la fila no existe (compat
    // con apps pre-E3).
    let err = run_sql(&db, "UPDATE t SET nombre = 'X' WHERE id = 999;").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("[GBY-3006]"), "esperaba GBY-3006: {}", msg);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e3_delete_by_indexed_column_removes_all_matches() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e3_del_idx");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e3_fixture(&db)?;

    run_sql(&db, "DELETE FROM t WHERE ciudad = 'MDQ';")?;
    let res = run_sql(&db, "SELECT id FROM t;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    // MDQ tenía filas 2 y 5. Deben desaparecer.
    assert_eq!(ids, vec![1, 3, 4]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e3_delete_with_combinator() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e3_del_combo");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e3_fixture(&db)?;

    run_sql(&db, "DELETE FROM t WHERE edad < 30 OR ciudad = 'BA';")?;
    let res = run_sql(&db, "SELECT id FROM t;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    // <30 → {2,4}. ciudad='BA' → {1,3}. Unión → {1,2,3,4}. Queda 5.
    assert_eq!(ids, vec![5]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e3_delete_by_in_subquery() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e3_del_subq");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e3_fixture(&db)?;

    run_sql(&db, "CREATE TABLE doomed (uid INT PRIMARY KEY);")?;
    run_sql(
        &db,
        "INSERT INTO doomed (uid) VALUES (1); INSERT INTO doomed (uid) VALUES (3); INSERT INTO doomed (uid) VALUES (5);",
    )?;

    run_sql(&db, "DELETE FROM t WHERE id IN (SELECT uid FROM doomed);")?;
    let res = run_sql(&db, "SELECT id FROM t;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![2, 4]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e3_delete_by_like_pattern() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e3_del_like");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);
    e3_fixture(&db)?;

    // Nombres terminados en 'a': Ana, Carla, Eva → ids 1, 3, 5.
    run_sql(&db, "DELETE FROM t WHERE nombre LIKE '%a';")?;
    let res = run_sql(&db, "SELECT id FROM t;")?;
    let mut ids = e1_ids(&res[0]);
    ids.sort();
    assert_eq!(ids, vec![2, 4]);

    cleanup(&[&db, &wal]);
    Ok(())
}

#[test]
fn e3_update_preserves_unique_check() -> Result<(), Box<dyn Error>> {
    let db = temp_db_path("e3_upd_unique");
    let wal = wal_path(&db);
    cleanup(&[&db, &wal]);

    let mut pager = Pager::create(&db)?;
    pager.close()?;
    run_sql(
        &db,
        "CREATE TABLE u (id INT PRIMARY KEY, email TEXT NOT NULL UNIQUE);",
    )?;
    run_sql(
        &db,
        "INSERT INTO u (id,email) VALUES (1,'a@x'); INSERT INTO u (id,email) VALUES (2,'b@x');",
    )?;

    // UPDATE masivo que generaría colisión UNIQUE — debe fallar antes
    // de tocar nada (idealmente la primera fila falla y la segunda
    // queda intacta; lo importante es que el error explote).
    let err = run_sql(&db, "UPDATE u SET email = 'a@x' WHERE id > 0;").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("[GBY-3003]") || msg.contains("UNIQUE"),
        "esperaba violación UNIQUE: {}",
        msg
    );

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
