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
