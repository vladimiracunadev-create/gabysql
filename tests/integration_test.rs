use gabysql::bptree::init_leaf_page;
use gabysql::sql::{parse, Engine, Value};
use gabysql::storage::{Header, Pager, PAGE_SIZE_DEFAULT};
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

    let mut leaf_page = vec![0; PAGE_SIZE_DEFAULT];
    init_leaf_page(&mut leaf_page);

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
    assert!(pager.header().page_count > 6, "expected splits to grow the file");

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
