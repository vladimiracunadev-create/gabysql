//! Push 15 (2026-06-18): tests E2E sobre los endpoints de listado del
//! catálogo expuestos en los Pushes 7, 10 y 14:
//!
//!   /views /policies /triggers /procedures /functions /users /roles /grants
//!
//! Cada test crea una DB con `Pager` directo (sin pasar por el server),
//! ejecuta el SQL que puebla el catálogo, levanta el server en un thread
//! con puerto efímero, hace GET al endpoint y assertea presencia de los
//! campos clave del JSON.
//!
//! El parser de JSON es ad-hoc — sólo `body.contains("...")` y un find
//! de pares `"field":"value"` simple. Para los tests basta. Mantiene la
//! invariante ADR-0001 (cero deps externas) consistente con
//! `tests/m13_server.rs`, del que reusamos los helpers (copiados, no
//! módulo común — Rust integration tests no comparten módulos sin
//! infra extra).

use std::error::Error;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use gabysql::server::{run, ServerConfig};
use gabysql::storage::Pager;

// ---------------------------------------------------------------------------
// Helpers fixture (paralelos a los de m13_server.rs).
// ---------------------------------------------------------------------------

fn temp_db(label: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("gby_p15_{}_{}.db", label, std::process::id()));
    let _ = std::fs::remove_file(&p);
    let mut wal = p.clone();
    wal.set_extension("db.wal");
    let _ = std::fs::remove_file(&wal);
    p
}

fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn spawn_server(db_path: PathBuf) -> String {
    let port = pick_port();
    let addr = format!("127.0.0.1:{}", port);
    let server_addr = addr.clone();
    thread::spawn(move || {
        let _ = run(
            &server_addr,
            ServerConfig {
                single_db: Some(db_path),
                dir: None,
                token: None,
                max_connections: 16,
                log_json: false,
                logger: None,
            },
        );
    });
    for _ in 0..50 {
        if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(100)).is_ok() {
            return addr;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("server no arrancó a tiempo en {}", addr);
}

fn http_get(addr: &str, path: &str) -> Result<(u16, String), Box<dyn Error>> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, addr
    );
    stream.write_all(req.as_bytes())?;
    stream.flush()?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or("respuesta sin separador headers/body")?;
    let status_line = head.lines().next().ok_or("respuesta vacía")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or("status code missing")?
        .parse()?;
    Ok((status, body.to_string()))
}

/// Ejecuta SQL setup sobre la DB directo con Pager + Engine, fuera
/// del server. Acepta múltiples statements separados por `;` opcional.
fn setup_sql(db: &std::path::Path, sql: &[&str]) -> Result<(), Box<dyn Error>> {
    let mut pager = if db.exists() {
        Pager::open(db)?
    } else {
        Pager::create(db)?
    };
    pager.begin()?;
    {
        let mut engine = gabysql::sql::Engine::new(&mut pager);
        for s in sql {
            for stmt in gabysql::sql::parse(s)? {
                engine.exec(stmt)?;
            }
        }
    }
    pager.commit()?;
    pager.close()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests por endpoint.
// ---------------------------------------------------------------------------

#[test]
fn views_endpoint_lists_declared_view() -> Result<(), Box<dyn Error>> {
    let db = temp_db("views");
    setup_sql(
        &db,
        &[
            "CREATE TABLE t (id INT PRIMARY KEY, v INT)",
            "CREATE VIEW heavy AS SELECT id, v FROM t WHERE v > 50",
        ],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/views")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"ok\":true"), "body: {}", body);
    assert!(body.contains("\"name\":\"heavy\""), "body: {}", body);
    assert!(body.contains("\"source\":"), "body: {}", body);
    Ok(())
}

#[test]
fn policies_endpoint_lists_declared_policies() -> Result<(), Box<dyn Error>> {
    let db = temp_db("policies");
    setup_sql(
        &db,
        &[
            "CREATE TABLE customers (id INT PRIMARY KEY, country TEXT)",
            "CREATE ROLE bob",
            "CREATE POLICY p_bob_ar ON customers FOR SELECT TO bob USING (country = 'AR')",
        ],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/policies")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"name\":\"p_bob_ar\""), "body: {}", body);
    assert!(body.contains("\"action\":\"SELECT\""), "body: {}", body);
    assert!(body.contains("\"using\":"), "body: {}", body);
    Ok(())
}

#[test]
fn policies_endpoint_filters_by_table() -> Result<(), Box<dyn Error>> {
    let db = temp_db("policies_filter");
    setup_sql(
        &db,
        &[
            "CREATE TABLE a (id INT PRIMARY KEY)",
            "CREATE TABLE b (id INT PRIMARY KEY)",
            "CREATE POLICY p_a ON a FOR SELECT USING (id > 0)",
            "CREATE POLICY p_b ON b FOR SELECT USING (id > 0)",
        ],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/policies?table=a")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"name\":\"p_a\""), "body: {}", body);
    assert!(!body.contains("\"name\":\"p_b\""), "body: {}", body);
    Ok(())
}

#[test]
fn triggers_endpoint_lists_declared_trigger() -> Result<(), Box<dyn Error>> {
    let db = temp_db("triggers");
    setup_sql(
        &db,
        &[
            "CREATE TABLE accounts (id INT PRIMARY KEY, balance INT)",
            "CREATE TABLE audit_log (id INT PRIMARY KEY, account_id INT, action TEXT)",
            "CREATE TRIGGER trg_audit AFTER UPDATE ON accounts FOR EACH ROW \
             INSERT INTO audit_log (id, account_id, action) VALUES (NEW.id, NEW.id, 'update')",
        ],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/triggers")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"name\":\"trg_audit\""), "body: {}", body);
    assert!(body.contains("\"timing\":\"AFTER\""), "body: {}", body);
    assert!(body.contains("\"event\":\"UPDATE\""), "body: {}", body);
    assert!(body.contains("\"table\":\"accounts\""), "body: {}", body);
    Ok(())
}

#[test]
fn procedures_endpoint_lists_declared_procedure() -> Result<(), Box<dyn Error>> {
    let db = temp_db("procs");
    setup_sql(
        &db,
        &[
            "CREATE TABLE counters (id INT PRIMARY KEY, n INT)",
            "CREATE PROCEDURE bump_counter() AS UPDATE counters SET n = n + 1 WHERE id = 1",
        ],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/procedures")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"name\":\"bump_counter\""), "body: {}", body);
    assert!(body.contains("\"params\":["), "body: {}", body);
    Ok(())
}

#[test]
fn functions_endpoint_lists_declared_function() -> Result<(), Box<dyn Error>> {
    let db = temp_db("fns");
    setup_sql(
        &db,
        &["CREATE FUNCTION doublev(n INT) RETURNS INT AS n * 2"],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/functions")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"name\":\"doublev\""), "body: {}", body);
    assert!(body.contains("\"returnType\":\"INT\""), "body: {}", body);
    assert!(body.contains("\"name\":\"n\""), "body: {}", body);
    Ok(())
}

#[test]
fn users_endpoint_lists_user_without_secret_material() -> Result<(), Box<dyn Error>> {
    let db = temp_db("users");
    setup_sql(&db, &["CREATE USER alice WITH PASSWORD 'hunter2'"])?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/users")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"name\":\"alice\""), "body: {}", body);
    // El scheme exacto del default del motor puede cambiar sin que
    // el contrato del endpoint cambie. El requisito real es que se
    // serialice un scheme conocido (no "unknown" — que delataría un
    // mapeo faltante en server.rs cuando el motor agregue un scheme
    // nuevo).
    let known_schemes = ["pbkdf2-sha256", "scrypt", "argon2id"];
    assert!(
        known_schemes
            .iter()
            .any(|s| body.contains(&format!("\"scheme\":\"{}\"", s))),
        "scheme no es uno de los conocidos del motor (pbkdf2-sha256/scrypt/argon2id). body: {}",
        body
    );
    // CRITICO: la API NUNCA debe filtrar hash ni salt.
    assert!(
        !body.contains("password_hash"),
        "leak: password_hash en body"
    );
    assert!(!body.contains("\"salt\""), "leak: salt en body");
    assert!(!body.contains("hunter2"), "leak: password en claro");
    Ok(())
}

#[test]
fn roles_endpoint_lists_declared_role() -> Result<(), Box<dyn Error>> {
    let db = temp_db("roles");
    setup_sql(&db, &["CREATE ROLE auditor"])?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/roles")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"name\":\"auditor\""), "body: {}", body);
    Ok(())
}

#[test]
fn grants_endpoint_lists_privileges_as_keyword_array() -> Result<(), Box<dyn Error>> {
    let db = temp_db("grants");
    setup_sql(
        &db,
        &[
            "CREATE TABLE t (id INT PRIMARY KEY)",
            "CREATE ROLE reader",
            "GRANT SELECT, INSERT ON t TO reader",
        ],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/grants")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"grantee\":\"reader\""), "body: {}", body);
    assert!(body.contains("\"object\":\"t\""), "body: {}", body);
    assert!(body.contains("\"SELECT\""), "body: {}", body);
    assert!(body.contains("\"INSERT\""), "body: {}", body);
    // No otorgamos UPDATE → no debe estar en el array.
    assert!(!body.contains("\"UPDATE\""), "body: {}", body);
    Ok(())
}

#[test]
fn stats_endpoint_returns_row_count_after_analyze() -> Result<(), Box<dyn Error>> {
    let db = temp_db("stats");
    setup_sql(
        &db,
        &[
            "CREATE TABLE t (id INT PRIMARY KEY, v INT)",
            "INSERT INTO t (id, v) VALUES (1, 10)",
            "INSERT INTO t (id, v) VALUES (2, 20)",
            "INSERT INTO t (id, v) VALUES (3, 30)",
            "ANALYZE TABLE t",
        ],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/stats")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"name\":\"t\""), "body: {}", body);
    assert!(body.contains("\"rowCount\":3"), "body: {}", body);
    assert!(body.contains("\"columnCount\":"), "body: {}", body);
    // Default compact — no debería incluir el array `columns`.
    assert!(
        !body.contains("\"columns\":["),
        "default shape no debería ser full: {}",
        body
    );
    Ok(())
}

#[test]
fn stats_endpoint_full_includes_per_column_stats() -> Result<(), Box<dyn Error>> {
    let db = temp_db("stats_full");
    setup_sql(
        &db,
        &[
            "CREATE TABLE t (id INT PRIMARY KEY, v INT)",
            "INSERT INTO t (id, v) VALUES (1, 10)",
            "ANALYZE TABLE t",
        ],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/stats?full=1")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"columns\":["), "body: {}", body);
    assert!(body.contains("\"ndv\":"), "body: {}", body);
    Ok(())
}

#[test]
fn objects_endpoint_summarizes_catalog() -> Result<(), Box<dyn Error>> {
    let db = temp_db("objects");
    setup_sql(
        &db,
        &[
            "CREATE TABLE a (id INT PRIMARY KEY)",
            "CREATE TABLE b (id INT PRIMARY KEY)",
            "CREATE VIEW v AS SELECT id FROM a",
            "CREATE ROLE r",
        ],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/objects")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"tables\":2"), "body: {}", body);
    assert!(body.contains("\"views\":1"), "body: {}", body);
    assert!(body.contains("\"roles\":1"), "body: {}", body);
    assert!(body.contains("\"total\":"), "body: {}", body);
    Ok(())
}

#[test]
fn grants_endpoint_filters_by_grantee() -> Result<(), Box<dyn Error>> {
    let db = temp_db("grants_filter");
    setup_sql(
        &db,
        &[
            "CREATE TABLE t (id INT PRIMARY KEY)",
            "CREATE ROLE alice_role",
            "CREATE ROLE bob_role",
            "GRANT SELECT ON t TO alice_role",
            "GRANT INSERT ON t TO bob_role",
        ],
    )?;
    let addr = spawn_server(db);
    let (status, body) = http_get(&addr, "/grants?grantee=bob_role")?;
    assert_eq!(status, 200, "body: {}", body);
    assert!(body.contains("\"grantee\":\"bob_role\""), "body: {}", body);
    assert!(!body.contains("alice_role"), "body: {}", body);
    Ok(())
}
