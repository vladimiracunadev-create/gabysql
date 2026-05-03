use crate::catalog::{Catalog, TableMeta};
use crate::sql::{decode_row, parse, Engine, ResultSet, Value};
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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_MAX_CONNECTIONS: usize = 64;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub single_db: Option<PathBuf>,
    pub dir: Option<PathBuf>,
    pub token: Option<String>,
    pub max_connections: usize,
}

impl ServerConfig {
    pub fn new(single_db: Option<PathBuf>, dir: Option<PathBuf>, token: Option<String>) -> Self {
        Self {
            single_db,
            dir,
            token,
            max_connections: DEFAULT_MAX_CONNECTIONS,
        }
    }
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
    let max_connections = if config.max_connections == 0 {
        DEFAULT_MAX_CONNECTIONS
    } else {
        config.max_connections
    };

    eprintln!(
        "gabysql-server escuchando en {} (single={:?} dir={:?} max_connections={})",
        bind_addr, config.single_db, config.dir, max_connections
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
                    continue;
                }
                active.fetch_add(1, Ordering::AcqRel);
                let config = config.clone();
                let write_lock = Arc::clone(&write_lock);
                let active = Arc::clone(&active);
                thread::spawn(move || {
                    let response = match read_request(&mut stream)
                        .and_then(|request| handle_request(request, &config, &write_lock))
                    {
                        Ok(response) => response,
                        Err(err) => Response::json(
                            500,
                            format!(
                                "{{\"ok\":false,\"error\":{}}}",
                                json_string(&err.to_string())
                            ),
                        ),
                    };
                    let _ = write_response(&mut stream, response);
                    active.fetch_sub(1, Ordering::AcqRel);
                });
            }
            Err(err) => eprintln!("accept error: {}", err),
        }
    }

    Ok(())
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
) -> DbResult<Response> {
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

    let dir = config
        .dir
        .as_ref()
        .ok_or_else(|| DbError::new("servidor sin directorio configurado"))?;
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
    let dir = config
        .dir
        .as_ref()
        .ok_or_else(|| DbError::new("servidor sin directorio configurado"))?;
    let body = String::from_utf8_lossy(&request.body);
    let mut name = if body.trim_start().starts_with('{') {
        extract_json_string(&body, "db").unwrap_or_default()
    } else {
        body.trim().to_string()
    };
    name = normalize_db_name(&name)?;
    let path = dir.join(&name);

    let _guard = write_lock
        .lock()
        .map_err(|_| DbError::new("mutex poisoned"))?;
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
        .ok_or_else(|| DbError::new("falta table"))?;
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
        .ok_or_else(|| DbError::new("falta table"))?;

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
    let _guard = write_lock
        .lock()
        .map_err(|_| DbError::new("mutex poisoned"))?;
    let (mut pager, _) = open_pager_from_request(config, requested_db.as_ref())?;
    pager.begin()?;

    let response = (|| -> DbResult<Response> {
        let statements = parse(&sql)?;
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

    let dir = config
        .dir
        .as_ref()
        .ok_or_else(|| DbError::new("falta db (modo -dir)"))?;
    let requested = requested_db.ok_or_else(|| DbError::new("falta db (modo -dir)"))?;
    let name = normalize_db_name(requested)?;
    Ok((Pager::open(dir.join(&name))?, name))
}

fn normalize_db_name(value: &str) -> DbResult<String> {
    let mut name = value.trim().to_string();
    if name.is_empty() {
        return Err(DbError::new("db vacío"));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(DbError::new("db inválida"));
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
            format!(
                "{{\"name\":{},\"type\":{},\"pk\":{}}}",
                json_string(&column.name),
                json_string(column.column_type.as_sql()),
                if column.name.eq_ignore_ascii_case(&meta.primary_key) {
                    "true"
                } else {
                    "false"
                }
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{{\"name\":{},\"primaryKey\":{},\"rootPage\":{},\"columns\":[{}]}}",
        json_string(&meta.name),
        json_string(&meta.primary_key),
        meta.root_page,
        columns
    )
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
            return Err(DbError::new("conexión cerrada"));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_bytes(&buffer, b"\r\n\r\n") {
            header_end = position + 4;
            break;
        }
        if buffer.len() > 1024 * 1024 {
            return Err(DbError::new("request header demasiado grande"));
        }
    }

    let header_text = String::from_utf8(buffer[..header_end].to_vec())?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| DbError::new("request line vacía"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| DbError::new("método faltante"))?
        .to_string();
    let target = parts
        .next()
        .ok_or_else(|| DbError::new("path faltante"))?
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
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
                    return Err(DbError::new("escape URL inválido"));
                }
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                    .map_err(|_| DbError::new("escape URL inválido"))?;
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
