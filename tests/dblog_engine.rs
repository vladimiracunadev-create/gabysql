//! Bloque L (2026-08-13): tests E2E del log de sentencias del motor.
//!
//! Cubren el enganche en `Engine::exec` — el filtro por nivel, la
//! captura del código `[GBY-NNNN]`, el `stmt_index` dentro de un batch y
//! el guard de anidamiento que impide que un `CALL` con loop escriba una
//! línea por iteración. Ver ADR-0094.

use gabysql::dblog::{DbLogger, LogLevel};
use gabysql::sql::{parse, Engine};
use gabysql::storage::Pager;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("gabysql-dblog-{label}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn wal_path(path: &Path) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(".wal");
    PathBuf::from(value)
}

/// Ejecuta `sql` contra `db` con el `logger` adjunto. Devuelve el error
/// del motor si lo hubo (varios tests dependen de que falle).
fn run_logged(db: &Path, logger: Option<&Arc<DbLogger>>, sql: &str) -> Result<(), Box<dyn Error>> {
    let mut pager = Pager::open(db)?;
    pager.begin()?;
    let response = (|| {
        let statements = parse(sql)?;
        let mut engine = Engine::new(&mut pager);
        if let Some(l) = logger {
            engine.attach_logger(Arc::clone(l));
            engine.set_log_source(sql);
        }
        for statement in statements {
            engine.exec(statement)?;
        }
        pager.commit()?;
        Ok::<_, gabysql::DbError>(())
    })();
    match response {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = pager.rollback();
            Err(Box::new(err))
        }
    }
}

fn new_db(dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let db = dir.join("test.db");
    let mut pager = Pager::create(&db)?;
    pager.close()?;
    Ok(db)
}

fn read_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn engine_without_logger_writes_nothing() -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("no-logger");
    let db = new_db(&dir)?;
    let log = dir.join("gabysql.log");

    run_logged(&db, None, "CREATE TABLE t (id INT PRIMARY KEY);")?;
    run_logged(&db, None, "INSERT INTO t (id) VALUES (1);")?;

    assert!(
        !log.exists(),
        "el motor sin logger no debe crear ningún archivo"
    );
    let _ = fs::remove_file(wal_path(&db));
    Ok(())
}

#[test]
fn error_level_captures_the_gby_code_and_skips_successes() -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("error-level");
    let db = new_db(&dir)?;
    let log = dir.join("gabysql.log");
    let logger = Arc::new(DbLogger::open(&log, LogLevel::Error, 0, 3)?);

    run_logged(&db, Some(&logger), "CREATE TABLE t (id INT PRIMARY KEY);")?;
    run_logged(&db, Some(&logger), "INSERT INTO t (id) VALUES (1);")?;
    // Segunda inserción con la misma PK → [GBY-3001].
    let err = run_logged(&db, Some(&logger), "INSERT INTO t (id) VALUES (1);");
    assert!(err.is_err(), "la PK duplicada debía fallar");

    let lines = read_lines(&log);
    assert_eq!(
        lines.len(),
        1,
        "sólo el fallo entra en level=error: {lines:?}"
    );
    let line = &lines[0];
    assert!(line.contains("\"code\":3001"), "{line}");
    assert!(line.contains("\"ok\":false"), "{line}");
    assert!(line.contains("\"kind\":\"INSERT\""), "{line}");
    // El SQL completo queda registrado (decisión explícita de la ADR).
    assert!(line.contains("INSERT INTO t (id) VALUES (1)"), "{line}");

    let _ = fs::remove_file(wal_path(&db));
    Ok(())
}

#[test]
fn mod_level_logs_ddl_and_dml_but_not_selects() -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("mod-level");
    let db = new_db(&dir)?;
    let log = dir.join("gabysql.log");
    let logger = Arc::new(DbLogger::open(&log, LogLevel::Mod, 0, 3)?);

    run_logged(&db, Some(&logger), "CREATE TABLE t (id INT PRIMARY KEY);")?;
    run_logged(&db, Some(&logger), "INSERT INTO t (id) VALUES (1);")?;
    run_logged(&db, Some(&logger), "SELECT id FROM t;")?;

    let lines = read_lines(&log);
    assert_eq!(lines.len(), 2, "el SELECT no debía loguearse: {lines:?}");
    assert!(
        lines[0].contains("\"kind\":\"CREATE TABLE\""),
        "{}",
        lines[0]
    );
    assert!(lines[1].contains("\"kind\":\"INSERT\""), "{}", lines[1]);
    assert!(lines.iter().all(|l| l.contains("\"mutating\":true")));

    let _ = fs::remove_file(wal_path(&db));
    Ok(())
}

#[test]
fn all_level_logs_selects_with_row_count_and_duration() -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("all-level");
    let db = new_db(&dir)?;
    let log = dir.join("gabysql.log");
    let logger = Arc::new(DbLogger::open(&log, LogLevel::All, 0, 3)?);

    run_logged(&db, Some(&logger), "CREATE TABLE t (id INT PRIMARY KEY);")?;
    run_logged(
        &db,
        Some(&logger),
        "INSERT INTO t (id) VALUES (1); INSERT INTO t (id) VALUES (2);",
    )?;
    run_logged(&db, Some(&logger), "SELECT id FROM t;")?;

    let lines = read_lines(&log);
    assert_eq!(lines.len(), 4, "{lines:?}");
    let select = lines.last().unwrap();
    assert!(select.contains("\"kind\":\"SELECT\""), "{select}");
    assert!(select.contains("\"mutating\":false"), "{select}");
    assert!(select.contains("\"rows\":2"), "{select}");
    assert!(select.contains("\"duration_us\":"), "{select}");

    let _ = fs::remove_file(wal_path(&db));
    Ok(())
}

#[test]
fn stmt_index_disambiguates_statements_inside_one_batch() -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("stmt-index");
    let db = new_db(&dir)?;
    let log = dir.join("gabysql.log");
    let logger = Arc::new(DbLogger::open(&log, LogLevel::All, 0, 3)?);

    run_logged(
        &db,
        Some(&logger),
        "CREATE TABLE t (id INT PRIMARY KEY); INSERT INTO t (id) VALUES (1); SELECT id FROM t;",
    )?;

    let lines = read_lines(&log);
    assert_eq!(lines.len(), 3, "{lines:?}");
    for (i, line) in lines.iter().enumerate() {
        assert!(line.contains(&format!("\"stmt_index\":{i}")), "{line}");
        // Las tres comparten el texto del batch completo.
        assert!(line.contains("CREATE TABLE t"), "{line}");
    }

    let _ = fs::remove_file(wal_path(&db));
    Ok(())
}

#[test]
fn nested_exec_from_a_procedure_loop_logs_one_line_not_one_per_iteration(
) -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("nested-proc");
    let db = new_db(&dir)?;
    let log = dir.join("gabysql.log");

    // Setup sin logger para que el archivo sólo contenga el CALL.
    // El body sigue el patrón ya validado contra el motor en
    // integration_test.rs (`loop_n`): un `DECLARE` local no se resuelve
    // dentro de un `VALUES`, pero un parámetro del procedure sí.
    run_logged(&db, None, "CREATE TABLE t (id INT PRIMARY KEY);")?;
    run_logged(
        &db,
        None,
        "CREATE PROCEDURE loop_n(p_n INT) AS \
         BEGIN \
            DECLARE i INT DEFAULT 0; \
            WHILE i < p_n LOOP \
               SET i = i + 1; \
            END LOOP; \
            IF i = p_n THEN INSERT INTO t (id) VALUES (p_n); END IF; \
         END;",
    )?;

    let logger = Arc::new(DbLogger::open(&log, LogLevel::All, 0, 3)?);
    run_logged(&db, Some(&logger), "CALL loop_n(20);")?;

    // El CALL dispara ~40 `exec` anidados (WHILE + 20×SET + IF + INSERT).
    let lines = read_lines(&log);
    assert_eq!(
        lines.len(),
        1,
        "el guard de anidamiento debía colapsar las 20 iteraciones: {lines:?}"
    );
    assert!(lines[0].contains("\"kind\":\"CALL\""), "{}", lines[0]);

    // Sanity: el procedure realmente corrió el loop hasta el final.
    let mut pager = Pager::open(&db)?;
    pager.begin()?;
    let mut engine = Engine::new(&mut pager);
    let rs = engine.exec(parse("SELECT id FROM t")?.remove(0))?;
    assert_eq!(rs.rows.len(), 1, "el IF post-loop debía insertar la fila");
    pager.rollback()?;

    let _ = fs::remove_file(wal_path(&db));
    Ok(())
}

#[test]
fn nested_exec_from_a_trigger_does_not_duplicate_entries() -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("nested-trigger");
    let db = new_db(&dir)?;
    let log = dir.join("gabysql.log");

    run_logged(&db, None, "CREATE TABLE t (id INT PRIMARY KEY);")?;
    run_logged(&db, None, "CREATE TABLE t_audit (id INT PRIMARY KEY);")?;
    run_logged(
        &db,
        None,
        "CREATE TRIGGER trg AFTER INSERT ON t FOR EACH ROW \
         BEGIN INSERT INTO t_audit (id) VALUES (NEW.id); END",
    )?;

    let logger = Arc::new(DbLogger::open(&log, LogLevel::Mod, 0, 3)?);
    run_logged(&db, Some(&logger), "INSERT INTO t (id) VALUES (1);")?;

    let lines = read_lines(&log);
    assert_eq!(
        lines.len(),
        1,
        "el INSERT del trigger no debía generar su propia entrada: {lines:?}"
    );
    assert!(lines[0].contains("\"kind\":\"INSERT\""), "{}", lines[0]);

    let _ = fs::remove_file(wal_path(&db));
    Ok(())
}

#[test]
fn transaction_control_is_logged_at_mod_level() -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("txn-control");
    let db = new_db(&dir)?;
    let log = dir.join("gabysql.log");
    let logger = Arc::new(DbLogger::open(&log, LogLevel::Mod, 0, 3)?);

    run_logged(&db, Some(&logger), "CREATE TABLE t (id INT PRIMARY KEY);")?;
    run_logged(
        &db,
        Some(&logger),
        "BEGIN; INSERT INTO t (id) VALUES (1); COMMIT;",
    )?;

    let lines = read_lines(&log);
    let kinds: Vec<&str> = lines
        .iter()
        .map(|l| {
            let start = l.find("\"kind\":\"").unwrap() + 8;
            let rest = &l[start..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect();
    assert_eq!(kinds, vec!["CREATE TABLE", "BEGIN", "INSERT", "COMMIT"]);

    let _ = fs::remove_file(wal_path(&db));
    Ok(())
}

#[test]
fn explain_without_analyze_is_not_a_mutation() -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("explain");
    let db = new_db(&dir)?;
    let log = dir.join("gabysql.log");

    run_logged(&db, None, "CREATE TABLE t (id INT PRIMARY KEY);")?;

    let logger = Arc::new(DbLogger::open(&log, LogLevel::Mod, 0, 3)?);
    run_logged(&db, Some(&logger), "EXPLAIN SELECT id FROM t;")?;
    assert!(
        read_lines(&log).is_empty(),
        "EXPLAIN pelado sólo planifica: no entra en level=mod"
    );

    let _ = fs::remove_file(wal_path(&db));
    Ok(())
}
