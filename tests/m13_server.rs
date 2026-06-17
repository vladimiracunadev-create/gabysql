//! Bloque M13 (2026-06-15): smoke E2E sobre el server HTTP — cross-request
//! transactions via /tx/begin + /exec?session=... + /tx/commit.
//!
//! El server se levanta en un thread del proceso de test, los requests
//! usan `TcpStream` raw (cero deps externas, alinea ADR-0001). Cada test
//! pide un puerto efímero con `TcpListener::bind("127.0.0.1:0")` para
//! evitar colisiones cuando la suite corre con threads múltiples.

use std::error::Error;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use gabysql::server::{run, ServerConfig};
use gabysql::storage::Pager;

// ---------------------------------------------------------------------------
// Helpers de fixture.
// ---------------------------------------------------------------------------

fn temp_db(label: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("gby_m13_{}_{}.db", label, std::process::id()));
    let _ = std::fs::remove_file(&p);
    let mut wal = p.clone();
    wal.set_extension("db.wal");
    let _ = std::fs::remove_file(&wal);
    p
}

fn cleanup(p: &std::path::Path) {
    let _ = std::fs::remove_file(p);
    let mut wal = p.as_os_str().to_owned();
    wal.push(".wal");
    let _ = std::fs::remove_file(PathBuf::from(wal));
}

/// Reserva un puerto efímero — el OS asigna uno libre.
fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Spawn server en background sobre `db_path`. Retorna addr.
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
            },
        );
    });
    // Esperar hasta que el listener acepte connections.
    for _ in 0..50 {
        if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(100)).is_ok() {
            return addr;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("server no arrancó a tiempo en {}", addr);
}

/// POST minimalista: serializa headers + body y devuelve `(status, body)`.
fn http_post(
    addr: &str,
    path: &str,
    body: &str,
    extra_headers: &[(&str, &str)],
) -> Result<(u16, String), Box<dyn Error>> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        path,
        addr,
        body.len()
    );
    for (k, v) in extra_headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    req.push_str("Connection: close\r\n\r\n");
    req.push_str(body);
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

/// Extrae el valor de un campo JSON top-level por nombre (string only).
/// Parser ad-hoc — el tests assertion no necesita más.
fn json_str_field(body: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", field);
    let pos = body.find(&needle)?;
    let after = &body[pos + needle.len()..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// Smoke flow completo: BEGIN → insert across 2 requests → COMMIT →
/// verificar persistencia.
#[test]
fn m13_cross_request_tx_persists_on_commit() -> Result<(), Box<dyn Error>> {
    let db = temp_db("commit");
    // Pre-create DB con tabla (schema setup vía un Pager local — no
    // pasa por el server).
    {
        let mut pager = Pager::create(&db)?;
        pager.begin()?;
        {
            let mut engine = gabysql::sql::Engine::new(&mut pager);
            for stmt in gabysql::sql::parse("CREATE TABLE t (id INT PRIMARY KEY, v INT)")? {
                engine.exec(stmt)?;
            }
        }
        pager.commit()?;
        pager.close()?;
    }

    let addr = spawn_server(db.clone());

    // 1) /tx/begin → recibimos session ID.
    let (status, body) = http_post(&addr, "/tx/begin", "{}", &[])?;
    assert_eq!(status, 200, "begin status: {}", body);
    let session = json_str_field(&body, "session").expect("session field");
    assert_eq!(session.len(), 16, "session id de 16 hex chars");

    // 2) /exec con el session — INSERT 1.
    let (s1, b1) = http_post(
        &addr,
        "/exec",
        "{\"sql\":\"INSERT INTO t (id, v) VALUES (1, 100)\"}",
        &[("X-Gabysql-Session", session.as_str())],
    )?;
    assert_eq!(s1, 200, "exec1: {}", b1);

    // 3) /exec con el mismo session — INSERT 2.
    let (s2, b2) = http_post(
        &addr,
        "/exec",
        "{\"sql\":\"INSERT INTO t (id, v) VALUES (2, 200)\"}",
        &[("X-Gabysql-Session", session.as_str())],
    )?;
    assert_eq!(s2, 200, "exec2: {}", b2);

    // 4) /tx/commit?session=...
    let (sc, bc) = http_post(&addr, &format!("/tx/commit?session={}", session), "", &[])?;
    assert_eq!(sc, 200, "commit: {}", bc);
    assert!(bc.contains("\"ok\":true"), "commit body: {}", bc);

    // 5) Verificar persistencia con un request auto-commit normal.
    let (sv, bv) = http_post(&addr, "/exec", "{\"sql\":\"SELECT COUNT(*) FROM t\"}", &[])?;
    assert_eq!(sv, 200, "select: {}", bv);
    assert!(bv.contains("[[2]]"), "esperaba count=2 ([[2]]) en: {}", bv);

    cleanup(&db);
    Ok(())
}

/// rollback discards all session changes.
#[test]
fn m13_cross_request_tx_rollback_discards() -> Result<(), Box<dyn Error>> {
    let db = temp_db("rollback");
    {
        let mut pager = Pager::create(&db)?;
        pager.begin()?;
        {
            let mut engine = gabysql::sql::Engine::new(&mut pager);
            for stmt in gabysql::sql::parse("CREATE TABLE t (id INT PRIMARY KEY, v INT)")? {
                engine.exec(stmt)?;
            }
        }
        pager.commit()?;
        pager.close()?;
    }

    let addr = spawn_server(db.clone());

    let (_, body) = http_post(&addr, "/tx/begin", "{}", &[])?;
    let session = json_str_field(&body, "session").unwrap();

    http_post(
        &addr,
        "/exec",
        "{\"sql\":\"INSERT INTO t (id, v) VALUES (42, 999)\"}",
        &[("X-Gabysql-Session", session.as_str())],
    )?;

    let (sr, br) = http_post(&addr, &format!("/tx/rollback?session={}", session), "", &[])?;
    assert_eq!(sr, 200, "rollback: {}", br);

    // La fila no debe estar.
    let (_, bv) = http_post(&addr, "/exec", "{\"sql\":\"SELECT COUNT(*) FROM t\"}", &[])?;
    assert!(
        bv.contains("[[0]]"),
        "esperaba count=0 ([[0]]) post-rollback, vi: {}",
        bv
    );

    cleanup(&db);
    Ok(())
}

/// Solo UNA sesión activa a la vez — segundo /tx/begin debe 409.
#[test]
fn m13_double_begin_rejected_409() -> Result<(), Box<dyn Error>> {
    let db = temp_db("doublebegin");
    {
        let mut pager = Pager::create(&db)?;
        pager.begin()?;
        {
            let mut engine = gabysql::sql::Engine::new(&mut pager);
            for stmt in gabysql::sql::parse("CREATE TABLE t (id INT PRIMARY KEY)")? {
                engine.exec(stmt)?;
            }
        }
        pager.commit()?;
        pager.close()?;
    }

    let addr = spawn_server(db.clone());

    let (s1, _) = http_post(&addr, "/tx/begin", "{}", &[])?;
    assert_eq!(s1, 200);
    let (s2, b2) = http_post(&addr, "/tx/begin", "{}", &[])?;
    assert_eq!(s2, 409, "esperaba 409 en double begin, vi: {}", b2);

    cleanup(&db);
    Ok(())
}

/// session ID inválido → 404 en /exec.
#[test]
fn m13_invalid_session_id_404() -> Result<(), Box<dyn Error>> {
    let db = temp_db("invalid");
    {
        let mut pager = Pager::create(&db)?;
        pager.begin()?;
        {
            let mut engine = gabysql::sql::Engine::new(&mut pager);
            for stmt in gabysql::sql::parse("CREATE TABLE t (id INT PRIMARY KEY)")? {
                engine.exec(stmt)?;
            }
        }
        pager.commit()?;
        pager.close()?;
    }

    let addr = spawn_server(db.clone());

    let (s, b) = http_post(
        &addr,
        "/exec",
        "{\"sql\":\"SELECT 1\"}",
        &[("X-Gabysql-Session", "deadbeefdeadbeef")],
    )?;
    assert_eq!(s, 404, "esperaba 404 session inválida, vi: {}", b);

    cleanup(&db);
    Ok(())
}
