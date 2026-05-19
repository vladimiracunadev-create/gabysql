use crate::catalog::{Catalog, DefaultLiteral, TableMeta};
use crate::sql::{decode_row, parse, Engine, ResultSet, Statement, Value};
use crate::storage::Pager;
use crate::{DbError, DbResult};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_MAX_CONNECTIONS: usize = 64;

/// Bounded ring of recent latency samples used for the p50/p95 in
/// `/metrics`. Keeping it fixed-size means the metrics endpoint runs in
/// O(N log N) on a small slice (a few thousand entries at most) and the
/// memory footprint of the server stays bounded under sustained load.
const LATENCY_SAMPLE_RING: usize = 1024;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub single_db: Option<PathBuf>,
    pub dir: Option<PathBuf>,
    pub token: Option<String>,
    pub max_connections: usize,
    /// Emit one JSON line per request to stdout. Opt-in so the default
    /// stdout stays clean for humans / `tail -f` runbooks.
    pub log_json: bool,
}

impl ServerConfig {
    pub fn new(single_db: Option<PathBuf>, dir: Option<PathBuf>, token: Option<String>) -> Self {
        Self {
            single_db,
            dir,
            token,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            log_json: false,
        }
    }
}

/// In-memory metrics accumulated since the server started. Shared
/// across handler threads via `Arc<Mutex<Metrics>>`.
#[derive(Debug)]
pub struct Metrics {
    started_unix: u64,
    requests_by_status: HashMap<u16, u64>,
    errors_total: u64,
    latency_samples: Vec<u32>,
    latency_cursor: usize,
    latency_count: u64,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            started_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            requests_by_status: HashMap::new(),
            errors_total: 0,
            latency_samples: Vec::with_capacity(LATENCY_SAMPLE_RING),
            latency_cursor: 0,
            latency_count: 0,
        }
    }

    /// Record a finished request. Statuses ≥ 500 also bump
    /// `errors_total` so runbook alerts can wire to a single number.
    pub fn record(&mut self, status: u16, latency_ms: u32) {
        *self.requests_by_status.entry(status).or_insert(0) += 1;
        if status >= 500 {
            self.errors_total += 1;
        }
        if self.latency_samples.len() < LATENCY_SAMPLE_RING {
            self.latency_samples.push(latency_ms);
        } else {
            self.latency_samples[self.latency_cursor] = latency_ms;
            self.latency_cursor = (self.latency_cursor + 1) % LATENCY_SAMPLE_RING;
        }
        self.latency_count += 1;
    }

    /// JSON snapshot for the `/metrics` endpoint.
    pub fn snapshot_json(&self) -> String {
        let uptime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(self.started_unix);
        let requests_total: u64 = self.requests_by_status.values().sum();

        let (p50, p95) = percentiles(&self.latency_samples);

        let mut by_status: Vec<(u16, u64)> = self
            .requests_by_status
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect();
        by_status.sort_by_key(|(k, _)| *k);
        let by_status_json = by_status
            .iter()
            .map(|(status, count)| format!("\"{}\":{}", status, count))
            .collect::<Vec<_>>()
            .join(",");

        format!(
            "{{\"ok\":true,\"started_unix\":{},\"uptime_s\":{},\"requests_total\":{},\
             \"requests_by_status\":{{{}}},\"errors_total\":{},\
             \"latency_ms\":{{\"p50\":{},\"p95\":{},\"samples\":{},\"count\":{}}}}}",
            self.started_unix,
            uptime,
            requests_total,
            by_status_json,
            self.errors_total,
            p50,
            p95,
            self.latency_samples.len(),
            self.latency_count,
        )
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute p50 and p95 over a latency sample buffer. Returns (0, 0)
/// when the buffer is empty so the metrics endpoint stays well-formed
/// even on a freshly started server.
fn percentiles(samples: &[u32]) -> (u32, u32) {
    if samples.is_empty() {
        return (0, 0);
    }
    let mut sorted: Vec<u32> = samples.to_vec();
    sorted.sort_unstable();
    let pick = |q: f64| -> u32 {
        let idx = ((sorted.len() as f64 - 1.0) * q).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    };
    (pick(0.50), pick(0.95))
}

pub fn run(addr: &str, config: ServerConfig) -> DbResult<()> {
    if config.single_db.is_none() && config.dir.is_none() {
        return Err(DbError::new("falta -db <archivo.db> o -dir <carpeta>"));
    }
    if config.single_db.is_some() && config.dir.is_some() {
        return Err(DbError::new("usa solo uno: -db o -dir"));
    }

    let bind_addr = normalize_bind_addr(addr);
    let listener = TcpListener::bind(&bind_addr)?;
    let write_lock = Arc::new(Mutex::new(()));
    let active = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(Mutex::new(Metrics::new()));
    let max_connections = if config.max_connections == 0 {
        DEFAULT_MAX_CONNECTIONS
    } else {
        config.max_connections
    };

    eprintln!(
        "gabysql-server escuchando en {} (single={:?} dir={:?} max_connections={} log_json={})",
        bind_addr, config.single_db, config.dir, max_connections, config.log_json
    );

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let current = active.load(Ordering::Acquire);
                if current >= max_connections {
                    let body = format!(
                        "{{\"ok\":false,\"error\":{}}}",
                        json_string(&format!(
                            "server busy: {} active connections (max {})",
                            current, max_connections
                        ))
                    );
                    let _ = write_response(&mut stream, Response::json(503, body));
                    if let Ok(mut m) = metrics.lock() {
                        m.record(503, 0);
                    }
                    if config.log_json {
                        log_request_json("?", "?", 503, 0);
                    }
                    continue;
                }
                active.fetch_add(1, Ordering::AcqRel);
                let config = config.clone();
                let write_lock = Arc::clone(&write_lock);
                let active = Arc::clone(&active);
                let metrics_thread = Arc::clone(&metrics);
                thread::spawn(move || {
                    let start = Instant::now();
                    let parsed = read_request(&mut stream);
                    let (method, path) = match &parsed {
                        Ok(req) => (req.method.clone(), req.path.clone()),
                        Err(_) => ("?".to_string(), "?".to_string()),
                    };
                    let response = match parsed.and_then(|request| {
                        handle_request(request, &config, &write_lock, &metrics_thread)
                    }) {
                        Ok(response) => response,
                        Err(err) => Response::json(
                            500,
                            format!(
                                "{{\"ok\":false,\"error\":{}}}",
                                json_string(&err.to_string())
                            ),
                        ),
                    };
                    let status = response.status;
                    let _ = write_response(&mut stream, response);
                    let latency_ms = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX);
                    if let Ok(mut m) = metrics_thread.lock() {
                        m.record(status, latency_ms);
                    }
                    if config.log_json {
                        log_request_json(&method, &path, status, latency_ms);
                    }
                    active.fetch_sub(1, Ordering::AcqRel);
                });
            }
            Err(err) => eprintln!("accept error: {}", err),
        }
    }

    Ok(())
}

/// Emit a single JSON line describing a finished request. Goes to
/// stdout (not stderr) so the runbook can pipe it through `jq`/`tee`
/// while the startup banner on stderr stays readable. Opt-in via
/// `-log-json` so the default UX stays quiet.
fn log_request_json(method: &str, path: &str, status: u16, latency_ms: u32) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    println!(
        "{{\"ts_unix\":{},\"method\":{},\"path\":{},\"status\":{},\"latency_ms\":{}}}",
        ts,
        json_string(method),
        json_string(path),
        status,
        latency_ms
    );
}

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug)]
struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn json(status: u16, body: String) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: body.into_bytes(),
        }
    }

    fn text(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.into().into_bytes(),
        }
    }
}

fn handle_request(
    request: Request,
    config: &ServerConfig,
    write_lock: &Arc<Mutex<()>>,
    metrics: &Arc<Mutex<Metrics>>,
) -> DbResult<Response> {
    // CORS preflight: every browser-originated request that carries
    // Authorization (which gabymodeler does when a token is set) sends
    // an OPTIONS first. Always answer 204 with the CORS headers (added
    // unconditionally in write_response) and skip auth — preflights
    // never carry credentials.
    if request.method == "OPTIONS" {
        return Ok(Response::text(204, ""));
    }
    if let Some(token) = &config.token {
        let auth_ok = request
            .headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(|value| value.trim() == token)
            .unwrap_or(false)
            || request
                .headers
                .get("x-gabysql-token")
                .map(|value| value == token)
                .unwrap_or(false);
        if !auth_ok {
            return Ok(Response::json(
                401,
                "{\"ok\":false,\"error\":\"unauthorized\"}".to_string(),
            ));
        }
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => Ok(Response::json(200, health_json(config))),
        ("GET", "/metrics") => {
            let body = metrics
                .lock()
                .map(|m| m.snapshot_json())
                .unwrap_or_else(|_| {
                    "{\"ok\":false,\"error\":\"metrics lock poisoned\"}".to_string()
                });
            Ok(Response::json(200, body))
        }
        ("GET", "/dbs") => Ok(Response::json(200, dbs_json(config)?)),
        ("POST", "/dbs") => create_db(request, config, write_lock),
        ("GET", "/tables") => tables(request, config),
        ("GET", "/schema") => schema(request, config),
        ("GET", "/rows") => rows(request, config),
        ("POST", "/exec") => exec_sql(request, config, write_lock),
        _ => Ok(Response::text(404, "not found")),
    }
}

fn health_json(config: &ServerConfig) -> String {
    let unix_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!(
        "{{\"ok\":true,\"name\":\"gabysql-server\",\"single\":{},\"dir\":{},\"timeUnix\":{}}}",
        if config.single_db.is_some() {
            "true"
        } else {
            "false"
        },
        match &config.dir {
            Some(dir) => json_string(&dir.display().to_string()),
            None => "null".to_string(),
        },
        unix_time
    )
}

fn dbs_json(config: &ServerConfig) -> DbResult<String> {
    if let Some(single_db) = &config.single_db {
        let name = single_db
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| single_db.display().to_string());
        return Ok(format!(
            "{{\"ok\":true,\"mode\":\"single-db\",\"dbs\":[{}],\"single\":{}}}",
            json_string(&name),
            json_string(&name)
        ));
    }

    let dir = config.dir.as_ref().ok_or_else(|| {
        DbError::new(
            "el servidor está en modo single-DB (-db); esta operación requiere -dir <carpeta>",
        )
    })?;
    let dbs = list_dbs(dir)?;
    Ok(format!(
        "{{\"ok\":true,\"mode\":\"multi-db\",\"dbs\":{}}}",
        string_array_json(&dbs)
    ))
}

fn create_db(
    request: Request,
    config: &ServerConfig,
    write_lock: &Arc<Mutex<()>>,
) -> DbResult<Response> {
    if config.single_db.is_some() {
        return Ok(Response::text(405, "use GET"));
    }
    let dir = config.dir.as_ref().ok_or_else(|| {
        DbError::new(
            "el servidor está en modo single-DB (-db); esta operación requiere -dir <carpeta>",
        )
    })?;
    let body = String::from_utf8_lossy(&request.body);
    let mut name = if body.trim_start().starts_with('{') {
        extract_json_string(&body, "db").unwrap_or_default()
    } else {
        body.trim().to_string()
    };
    name = normalize_db_name(&name)?;
    let path = dir.join(&name);

    let _guard = write_lock.lock().map_err(|_| {
        DbError::new("mutex envenenado: otro thread paniqueó mientras tenía el write_lock")
    })?;
    if path.exists() {
        return Ok(Response::json(
            409,
            "{\"ok\":false,\"error\":\"ya existe\"}".to_string(),
        ));
    }

    let mut pager = Pager::create(&path)?;
    pager.close()?;
    Ok(Response::json(
        201,
        format!("{{\"ok\":true,\"db\":{}}}", json_string(&name)),
    ))
}

fn tables(request: Request, config: &ServerConfig) -> DbResult<Response> {
    let (mut pager, _) = open_pager_from_request(config, request.query.get("db"))?;
    let mut catalog = Catalog::open(&mut pager);
    let tables = catalog.list_tables()?;
    let tables_json = tables
        .iter()
        .map(table_meta_json)
        .collect::<Vec<_>>()
        .join(",");
    Ok(Response::json(
        200,
        format!("{{\"ok\":true,\"tables\":[{}]}}", tables_json),
    ))
}

fn schema(request: Request, config: &ServerConfig) -> DbResult<Response> {
    let table = request
        .query
        .get("table")
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            DbError::new("falta el parámetro 'table' en la query string (ej: ?table=users)")
        })?;
    let (mut pager, _) = open_pager_from_request(config, request.query.get("db"))?;
    let mut catalog = Catalog::open(&mut pager);
    let table_meta = match catalog.get_table(&table)? {
        Some(meta) => meta,
        None => {
            return Ok(Response::json(
                404,
                "{\"ok\":false,\"error\":\"tabla no existe\"}".to_string(),
            ))
        }
    };

    Ok(Response::json(
        200,
        format!("{{\"ok\":true,\"table\":{}}}", table_meta_json(&table_meta)),
    ))
}

fn rows(request: Request, config: &ServerConfig) -> DbResult<Response> {
    let table = request
        .query
        .get("table")
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            DbError::new("falta el parámetro 'table' en la query string (ej: ?table=users)")
        })?;

    let mut limit = request
        .query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(25);
    let offset = request
        .query
        .get("offset")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if limit == 0 {
        limit = 25;
    }
    if limit > 1000 {
        limit = 1000;
    }

    let (mut pager, db_name) = open_pager_from_request(config, request.query.get("db"))?;
    let mut catalog = Catalog::open(&mut pager);
    let table_meta = match catalog.get_table(&table)? {
        Some(meta) => meta,
        None => {
            return Ok(Response::json(
                404,
                "{\"ok\":false,\"error\":\"tabla no existe\"}".to_string(),
            ))
        }
    };

    let total = catalog.scan_rows(table_meta.root_page, 0, None)?.len();
    let kvs = catalog.scan_rows(table_meta.root_page, offset, Some(limit))?;
    let columns = table_meta
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();

    let mut rows_json = Vec::new();
    for kv in kvs {
        let row = decode_row(&table_meta, &kv.value)?;
        let values = columns
            .iter()
            .map(|column| {
                row.get(&column.to_ascii_lowercase())
                    .cloned()
                    .unwrap_or(Value::Null)
            })
            .collect::<Vec<_>>();
        rows_json.push(value_array_json(&values));
    }

    Ok(Response::json(
        200,
        format!(
            "{{\"ok\":true,\"db\":{},\"table\":{},\"total\":{},\"limit\":{},\"offset\":{},\"columns\":{},\"rows\":[{}]}}",
            json_string(&db_name),
            json_string(&table),
            total,
            limit,
            offset,
            string_array_json(&columns),
            rows_json.join(",")
        ),
    ))
}

fn exec_sql(
    request: Request,
    config: &ServerConfig,
    write_lock: &Arc<Mutex<()>>,
) -> DbResult<Response> {
    let body = String::from_utf8(request.body).map_err(DbError::from)?;
    let sql = extract_json_string(&body, "sql").unwrap_or_default();
    if sql.trim().is_empty() {
        return Ok(Response::json(
            400,
            "{\"ok\":false,\"error\":\"sql vacío\"}".to_string(),
        ));
    }
    let requested_db = extract_json_string(&body, "db");
    let _guard = write_lock.lock().map_err(|_| {
        DbError::new("mutex envenenado: otro thread paniqueó mientras tenía el write_lock")
    })?;

    // Pre-parse to detect DB-level statements that don't operate on a
    // single .db file but on the directory. They are dispatched here
    // BEFORE we open any Pager. Mixing them with table-level statements
    // in the same /exec call is rejected: their semantics are
    // incompatible (no shared transaction).
    let statements = match parse(&sql) {
        Ok(v) => v,
        Err(err) => {
            return Ok(Response::json(
                400,
                format!(
                    "{{\"ok\":false,\"error\":{}}}",
                    json_string(&err.to_string())
                ),
            ));
        }
    };
    let has_db_level = statements.iter().any(is_db_level_stmt);
    if has_db_level {
        if statements.iter().any(|s| !is_db_level_stmt(s)) {
            return Ok(Response::json(
                400,
                "{\"ok\":false,\"error\":\"no se admite mezclar CREATE/DROP/SHOW DATABASE con sentencias de tabla en el mismo /exec\"}".to_string(),
            ));
        }
        return exec_db_level(statements, config);
    }

    let (mut pager, _) = open_pager_from_request(config, requested_db.as_ref())?;
    pager.begin()?;

    let response = (|| -> DbResult<Response> {
        let mut engine = Engine::new(&mut pager);
        let mut results = Vec::new();
        for statement in statements {
            results.push(engine.exec(statement)?);
        }
        pager.commit()?;
        let results_json = results
            .iter()
            .map(result_set_json)
            .collect::<Vec<_>>()
            .join(",");
        Ok(Response::json(
            200,
            format!("{{\"ok\":true,\"results\":[{}]}}", results_json),
        ))
    })();

    match response {
        Ok(response) => Ok(response),
        Err(err) => {
            let _ = pager.rollback();
            Ok(Response::json(
                400,
                format!(
                    "{{\"ok\":false,\"error\":{}}}",
                    json_string(&err.to_string())
                ),
            ))
        }
    }
}

fn is_db_level_stmt(s: &Statement) -> bool {
    matches!(
        s,
        Statement::CreateDatabase(_) | Statement::DropDatabase(_) | Statement::ShowDatabases
    )
}

/// Dispatch CREATE DATABASE / DROP DATABASE / SHOW DATABASES against the
/// directory configured with `-dir`. Refused outright in single-DB mode.
fn exec_db_level(stmts: Vec<Statement>, config: &ServerConfig) -> DbResult<Response> {
    if config.single_db.is_some() {
        return Ok(Response::json(
            405,
            "{\"ok\":false,\"error\":\"CREATE/DROP/SHOW DATABASE requieren modo -dir\"}"
                .to_string(),
        ));
    }
    let dir = config.dir.as_ref().ok_or_else(|| {
        DbError::new(
            "el servidor está en modo single-DB (-db); esta operación requiere -dir <carpeta>",
        )
    })?;

    let mut results = Vec::new();
    for stmt in stmts {
        match stmt {
            Statement::CreateDatabase(s) => {
                let name = normalize_db_name(&s.name)?;
                let path = dir.join(&name);
                if path.exists() {
                    if s.if_not_exists {
                        results.push(format!(
                            "{{\"columns\":[],\"rows\":[],\"message\":\"OK (already exists)\",\"db\":{}}}",
                            json_string(&name)
                        ));
                        continue;
                    }
                    return Ok(Response::json(
                        409,
                        format!(
                            "{{\"ok\":false,\"error\":{}}}",
                            json_string(&format!("base de datos '{}' ya existe", name))
                        ),
                    ));
                }
                let mut pager = Pager::create(&path)?;
                pager.close()?;
                results.push(format!(
                    "{{\"columns\":[],\"rows\":[],\"message\":\"OK\",\"db\":{}}}",
                    json_string(&name)
                ));
            }
            Statement::DropDatabase(s) => {
                let name = normalize_db_name(&s.name)?;
                let path = dir.join(&name);
                let wal = {
                    let mut p = path.clone().into_os_string();
                    p.push(".wal");
                    std::path::PathBuf::from(p)
                };
                if !path.exists() {
                    if s.if_exists {
                        results.push(format!(
                            "{{\"columns\":[],\"rows\":[],\"message\":\"OK (did not exist)\",\"db\":{}}}",
                            json_string(&name)
                        ));
                        continue;
                    }
                    return Ok(Response::json(
                        404,
                        format!(
                            "{{\"ok\":false,\"error\":{}}}",
                            json_string(&format!("base de datos '{}' no existe", name))
                        ),
                    ));
                }
                fs::remove_file(&path)?;
                let _ = fs::remove_file(&wal);
                results.push(format!(
                    "{{\"columns\":[],\"rows\":[],\"message\":\"OK\",\"db\":{}}}",
                    json_string(&name)
                ));
            }
            Statement::ShowDatabases => {
                let dbs = list_dbs(dir)?;
                let rows = dbs
                    .iter()
                    .map(|name| format!("[{}]", json_string(name)))
                    .collect::<Vec<_>>()
                    .join(",");
                results.push(format!(
                    "{{\"columns\":[\"database\"],\"rows\":[{}],\"message\":null}}",
                    rows
                ));
            }
            _ => unreachable!("filtered by is_db_level_stmt"),
        }
    }

    Ok(Response::json(
        200,
        format!("{{\"ok\":true,\"results\":[{}]}}", results.join(",")),
    ))
}

fn open_pager_from_request(
    config: &ServerConfig,
    requested_db: Option<&String>,
) -> DbResult<(Pager, String)> {
    if let Some(single_db) = &config.single_db {
        let name = single_db
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| single_db.display().to_string());
        return Ok((Pager::open(single_db)?, name));
    }

    let dir = config.dir.as_ref().ok_or_else(|| {
        DbError::new(
            "el servidor está en modo single-DB (-db); las rutas multi-DB (?db=...) \
             requieren arrancar con -dir <carpeta>",
        )
    })?;
    let requested = requested_db.ok_or_else(|| {
        DbError::new(
            "falta el parámetro 'db' en la query string (ej: ?db=demo.db); \
             requerido en modo -dir",
        )
    })?;
    let name = normalize_db_name(requested)?;
    Ok((Pager::open(dir.join(&name))?, name))
}

fn normalize_db_name(value: &str) -> DbResult<String> {
    let mut name = value.trim().to_string();
    if name.is_empty() {
        return Err(DbError::new(
            "parámetro 'db' vacío: indique el nombre del archivo .db dentro del directorio configurado",
        ));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(DbError::new(format!(
            "nombre de db inválido '{}': no se admiten separadores de path \
             ('/' o '\\\\'); la DB debe estar directamente en el directorio configurado con -dir",
            name
        )));
    }
    if !name.to_ascii_lowercase().ends_with(".db") {
        name.push_str(".db");
    }
    Ok(name)
}

fn list_dbs(dir: &Path) -> DbResult<Vec<String>> {
    let mut dbs = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name()?.to_string_lossy().to_string();
                if name.to_ascii_lowercase().ends_with(".db") {
                    return Some(name);
                }
            }
            None
        })
        .collect::<Vec<_>>();
    dbs.sort();
    Ok(dbs)
}

fn table_meta_json(meta: &TableMeta) -> String {
    let columns = meta
        .columns
        .iter()
        .map(|column| {
            // A column is reported as `unique` whenever there is a UNIQUE
            // secondary index whose single column is this one. PK is also
            // unique by construction (the B+Tree enforces it) and is
            // surfaced separately via `pk: true`.
            let is_unique = meta
                .indexes
                .iter()
                .any(|idx| idx.unique && idx.column.eq_ignore_ascii_case(&column.name));
            let default_field = match &column.default {
                Some(default) => format!(
                    ",\"hasDefault\":true,\"default\":{}",
                    default_literal_json(default)
                ),
                None => ",\"hasDefault\":false,\"default\":null".to_string(),
            };
            let references_field = match &column.references {
                Some(fk) => format!(
                    ",\"references\":{{\"table\":{},\"column\":{},\"onDelete\":{}}}",
                    json_string(&fk.table),
                    json_string(&fk.column),
                    json_string(fk.on_delete.as_sql())
                ),
                None => ",\"references\":null".to_string(),
            };
            format!(
                "{{\"name\":{},\"type\":{},\"pk\":{},\"notNull\":{},\"unique\":{}{}{}}}",
                json_string(&column.name),
                json_string(column.column_type.as_sql()),
                if column.name.eq_ignore_ascii_case(&meta.primary_key) {
                    "true"
                } else {
                    "false"
                },
                column.not_null,
                is_unique,
                default_field,
                references_field
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    let indexes = meta
        .indexes
        .iter()
        .map(|idx| {
            format!(
                "{{\"name\":{},\"column\":{},\"rootPage\":{},\"unique\":{}}}",
                json_string(&idx.name),
                json_string(&idx.column),
                idx.root_page,
                idx.unique
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{{\"name\":{},\"primaryKey\":{},\"rootPage\":{},\"columns\":[{}],\"indexes\":[{}]}}",
        json_string(&meta.name),
        json_string(&meta.primary_key),
        meta.root_page,
        columns,
        indexes
    )
}

/// JSON encoding for catalog `DefaultLiteral` values, mirroring the
/// `value_json` shape used elsewhere so consumers can treat both the
/// row-level cells and the column-level defaults uniformly.
fn default_literal_json(default: &DefaultLiteral) -> String {
    match default {
        DefaultLiteral::Null => "null".to_string(),
        DefaultLiteral::Integer(n) => n.to_string(),
        DefaultLiteral::Float(n) => {
            if n.is_finite() {
                n.to_string()
            } else {
                "null".to_string()
            }
        }
        DefaultLiteral::Bool(b) => b.to_string(),
        DefaultLiteral::String(s) => json_string(s),
    }
}

fn result_set_json(result: &ResultSet) -> String {
    let rows = result
        .rows
        .iter()
        .map(|row| value_array_json(row))
        .collect::<Vec<_>>()
        .join(",");
    let message = match &result.message {
        Some(message) => format!(",\"message\":{}", json_string(message)),
        None => String::new(),
    };
    format!(
        "{{\"columns\":{},\"rows\":[{}]{} }}",
        string_array_json(&result.columns),
        rows,
        message
    )
}

fn value_array_json(values: &[Value]) -> String {
    let encoded = values.iter().map(value_json).collect::<Vec<_>>().join(",");
    format!("[{}]", encoded)
}

fn value_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Integer(number) => number.to_string(),
        Value::Float(number) => {
            if number.is_finite() {
                let rendered = format!("{}", number);
                if rendered.contains('.') {
                    rendered
                } else {
                    format!("{}.0", rendered)
                }
            } else {
                "null".to_string()
            }
        }
        Value::Bool(flag) => {
            if *flag {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::String(text) => json_string(text),
    }
}

fn string_array_json(values: &[String]) -> String {
    let encoded = values
        .iter()
        .map(|value| json_string(value))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{}]", encoded)
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn normalize_bind_addr(addr: &str) -> String {
    if addr.starts_with(':') {
        format!("0.0.0.0{}", addr)
    } else {
        addr.to_string()
    }
}

fn read_request(stream: &mut TcpStream) -> DbResult<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end;
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(DbError::new(
                "conexión cerrada por el peer antes de recibir el header HTTP completo \
                 (esperaba terminar con CRLFCRLF)",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_bytes(&buffer, b"\r\n\r\n") {
            header_end = position + 4;
            break;
        }
        if buffer.len() > 1024 * 1024 {
            return Err(DbError::new(format!(
                "header de request demasiado grande: {} bytes acumulados sin encontrar CRLFCRLF \
                 (límite duro: 1 MiB)",
                buffer.len()
            )));
        }
    }

    let header_text = String::from_utf8(buffer[..header_end].to_vec())?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().ok_or_else(|| {
        DbError::new("request line HTTP vacía: el header no contiene 'METHOD path HTTP/x.y'")
    })?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| DbError::new(format!("request line HTTP sin método: '{}'", request_line)))?
        .to_string();
    let target = parts
        .next()
        .ok_or_else(|| DbError::new(format!("request line HTTP sin path: '{}'", request_line)))?
        .to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);

    let (path, query) = split_target(&target)?;
    Ok(Request {
        method,
        path,
        query,
        headers,
        body,
    })
}

fn write_response(stream: &mut TcpStream, response: Response) -> DbResult<()> {
    let status_text = match response.status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    };
    // CORS: open the API to browser-side tooling like gabymodeler. The
    // server has no notion of "this origin only" — anyone with the API
    // token (when configured) can talk to it; without a token it's
    // already meant for local trusted networks. Allowing * keeps the
    // contract honest. Preflight requests are handled in handle_request.
    let headers = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Authorization, Content-Type, X-Gabysql-Token\r\n\
         Access-Control-Max-Age: 600\r\n\
         Connection: close\r\n\r\n",
        response.status,
        status_text,
        response.content_type,
        response.body.len()
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()?;
    Ok(())
}

fn split_target(target: &str) -> DbResult<(String, HashMap<String, String>)> {
    let (path, query_string) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), Some(query)),
        None => (target.to_string(), None),
    };
    let mut query = HashMap::new();
    if let Some(query_string) = query_string {
        for pair in query_string.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (key, value),
                None => (pair, ""),
            };
            query.insert(percent_decode(key)?, percent_decode(value)?);
        }
    }
    Ok((path, query))
}

fn percent_decode(value: &str) -> DbResult<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(DbError::new(format!(
                        "escape URL inválido en offset {}: '%' truncado, faltan dígitos hexadecimales",
                        index
                    )));
                }
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).map_err(|_| {
                    DbError::new(format!(
                        "escape URL inválido en offset {}: dígitos hexadecimales no son UTF-8",
                        index
                    ))
                })?;
                let value = u8::from_str_radix(hex, 16)?;
                out.push(value);
                index += 3;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    Ok(String::from_utf8(out)?)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn extract_json_string(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let start = body.find(&needle)? + needle.len();
    let remainder = body[start..].trim_start();
    let remainder = remainder.strip_prefix(':')?.trim_start();
    let chars: Vec<char> = remainder.chars().collect();
    if chars.first().copied()? != '"' {
        return None;
    }
    let mut out = String::new();
    let mut escape = false;
    for ch in chars.into_iter().skip(1) {
        if escape {
            out.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                other => other,
            });
            escape = false;
            continue;
        }
        match ch {
            '\\' => escape = true,
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_records_status_and_latency() {
        let mut m = Metrics::new();
        m.record(200, 5);
        m.record(200, 15);
        m.record(400, 7);
        m.record(500, 50);

        let snap = m.snapshot_json();
        assert!(snap.contains("\"200\":2"), "snap: {}", snap);
        assert!(snap.contains("\"400\":1"), "snap: {}", snap);
        assert!(snap.contains("\"500\":1"), "snap: {}", snap);
        assert!(snap.contains("\"errors_total\":1"), "snap: {}", snap);
        assert!(snap.contains("\"requests_total\":4"), "snap: {}", snap);
    }

    #[test]
    fn metrics_percentiles_basic() {
        // 1..=100 → p50 ≈ 50, p95 ≈ 95
        let samples: Vec<u32> = (1..=100).collect();
        let (p50, p95) = percentiles(&samples);
        assert!(
            (49..=51).contains(&p50),
            "p50 should be around 50, got {}",
            p50
        );
        assert!(
            (94..=96).contains(&p95),
            "p95 should be around 95, got {}",
            p95
        );
    }

    #[test]
    fn metrics_percentiles_empty_safe() {
        assert_eq!(percentiles(&[]), (0, 0));
    }

    #[test]
    fn metrics_latency_ring_bounded() {
        let mut m = Metrics::new();
        for i in 0..(LATENCY_SAMPLE_RING * 3) {
            m.record(200, (i % 1000) as u32);
        }
        // Sample buffer caps at the ring size, but total count keeps growing.
        assert_eq!(m.latency_samples.len(), LATENCY_SAMPLE_RING);
        assert_eq!(m.latency_count as usize, LATENCY_SAMPLE_RING * 3);
    }
}
