//! `gabysql-mcp` — Gateway MCP (Model Context Protocol) sobre el HTTP/JSON
//! existente de `gabysql-server`.
//!
//! Diseño y justificación: ver `docs/adr/0010-mcp-gateway.md`.
//!
//! - Transporte: stdio, mensajes JSON-RPC 2.0 delimitados por `\n`.
//! - Sin dependencias externas: parser/emitter JSON, cliente HTTP/1.1 y
//!   loop JSON-RPC implementados a mano (en la misma línea que el
//!   `json_string`/`extract_json_string` ya presentes en `src/server.rs`).
//! - No abre el `.db`: actúa como cliente del `gabysql-server` HTTP/JSON,
//!   reusando su `write_lock`, su tope de conexiones y su authz por bearer.

use std::env;
use std::io::{self, BufRead, Read, Write};
use std::net::TcpStream;
use std::process::ExitCode;
use std::time::Duration;

const DEFAULT_SERVER: &str = "http://127.0.0.1:7878";
const PROTO_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "gabysql-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().collect();
    let cfg = match Config::from_args(&argv) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("gabysql-mcp: {err}");
            eprintln!();
            print_help();
            return ExitCode::from(2);
        }
    };
    if cfg.show_help {
        print_help();
        return ExitCode::SUCCESS;
    }
    if let Err(err) = run_stdio(&cfg) {
        eprintln!("gabysql-mcp fatal: {err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn print_help() {
    eprintln!(
        "gabysql-mcp {SERVER_VERSION} — MCP gateway sobre gabysql-server (HTTP/JSON)\n\
         \n\
         Uso:\n\
           gabysql-mcp [--server URL] [--token T] [--read-only]\n\
         \n\
         Flags:\n\
           --server URL    URL del gabysql-server (default {DEFAULT_SERVER})\n\
           --token T       Bearer token para el server. Override de GABYSQL_TOKEN.\n\
           --read-only     Rechaza la tool gabysql_execute (mutaciones) sin tocar la red.\n\
           --help, -h      Imprime este texto y sale.\n\
         \n\
         Habla MCP (JSON-RPC 2.0, mensajes delimitados por '\\n') sobre stdio.\n\
         Pensado para ser lanzado por un cliente MCP-compatible (Claude Desktop,\n\
         Claude Code, Cursor, etc.). Ver docs/adr/0010-mcp-gateway.md."
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Config
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Config {
    server_url: String,
    token: Option<String>,
    read_only: bool,
    show_help: bool,
}

impl Config {
    fn from_args(argv: &[String]) -> Result<Self, String> {
        let mut server_url = env::var("GABYSQL_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.into());
        let mut token = env::var("GABYSQL_TOKEN").ok().filter(|t| !t.is_empty());
        let mut read_only = false;
        let mut show_help = false;
        let mut i = 1;
        while i < argv.len() {
            let arg = &argv[i];
            match arg.as_str() {
                "--help" | "-h" => show_help = true,
                "--read-only" => read_only = true,
                "--server" => {
                    i += 1;
                    server_url = argv
                        .get(i)
                        .cloned()
                        .ok_or_else(|| "--server requiere URL".to_string())?;
                }
                "--token" => {
                    i += 1;
                    token = Some(
                        argv.get(i)
                            .cloned()
                            .ok_or_else(|| "--token requiere valor".to_string())?,
                    );
                }
                other => return Err(format!("flag desconocida: {other}")),
            }
            i += 1;
        }
        Ok(Self {
            server_url,
            token,
            read_only,
            show_help,
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// JSON value + parser + emitter (recursive descent, sin deps)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn as_str(&self) -> Option<&str> {
        if let Json::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
    fn get(&self, key: &str) -> Option<&Json> {
        if let Json::Obj(entries) = self {
            for (k, v) in entries {
                if k == key {
                    return Some(v);
                }
            }
        }
        None
    }
    fn obj() -> Vec<(String, Json)> {
        Vec::new()
    }
}

struct JParser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> JParser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            src: s.as_bytes(),
            pos: 0,
        }
    }
    fn skip_ws(&mut self) {
        while self.pos < self.src.len() {
            match self.src[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }
    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("se esperaba '{}'", c as char))
        }
    }
    fn parse(&mut self) -> Result<Json, String> {
        self.skip_ws();
        let v = self.value()?;
        self.skip_ws();
        Ok(v)
    }
    fn value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self.peek().ok_or("EOF inesperado")? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Ok(Json::Str(self.string()?)),
            b't' | b'f' => self.boolean(),
            b'n' => self.null(),
            _ => self.number(),
        }
    }
    fn object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        self.skip_ws();
        let mut out = Json::obj();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(out));
        }
        loop {
            self.skip_ws();
            let k = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            let v = self.value()?;
            out.push((k, v));
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b'}') => break,
                _ => return Err("se esperaba ',' o '}'".into()),
            }
        }
        Ok(Json::Obj(out))
    }
    fn array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        self.skip_ws();
        let mut out = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(out));
        }
        loop {
            let v = self.value()?;
            out.push(v);
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b']') => break,
                _ => return Err("se esperaba ',' o ']'".into()),
            }
        }
        Ok(Json::Arr(out))
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let c = self.bump().ok_or("string sin cerrar")?;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let esc = self.bump().ok_or("escape sin cerrar")?;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'u' => {
                            if self.pos + 4 > self.src.len() {
                                return Err("\\u corto".into());
                            }
                            let hex = std::str::from_utf8(&self.src[self.pos..self.pos + 4])
                                .map_err(|e| e.to_string())?;
                            let cp = u32::from_str_radix(hex, 16)
                                .map_err(|_| "\\u inválido".to_string())?;
                            self.pos += 4;
                            if let Some(c) = char::from_u32(cp) {
                                out.push(c);
                            } else {
                                out.push('\u{FFFD}');
                            }
                        }
                        other => return Err(format!("escape desconocido: \\{}", other as char)),
                    }
                }
                c => out.push(c as char),
            }
        }
    }
    fn boolean(&mut self) -> Result<Json, String> {
        if self.src[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(Json::Bool(true))
        } else if self.src[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(Json::Bool(false))
        } else {
            Err("bool inválido".into())
        }
    }
    fn null(&mut self) -> Result<Json, String> {
        if self.src[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(Json::Null)
        } else {
            Err("null inválido".into())
        }
    }
    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let raw = std::str::from_utf8(&self.src[start..self.pos]).map_err(|e| e.to_string())?;
        raw.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("número inválido: {raw}"))
    }
}

fn json_parse(s: &str) -> Result<Json, String> {
    JParser::new(s).parse()
}

fn json_emit(v: &Json, out: &mut String) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Num(n) => {
            if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e16 {
                out.push_str(&format!("{}", *n as i64));
            } else if n.is_finite() {
                out.push_str(&format!("{n}"));
            } else {
                out.push_str("null");
            }
        }
        Json::Str(s) => json_emit_string(s, out),
        Json::Arr(items) => {
            out.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_emit(it, out);
            }
            out.push(']');
        }
        Json::Obj(entries) => {
            out.push('{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_emit_string(k, out);
                out.push(':');
                json_emit(v, out);
            }
            out.push('}');
        }
    }
}

fn json_emit_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn json_to_string(v: &Json) -> String {
    let mut s = String::new();
    json_emit(v, &mut s);
    s
}

// ────────────────────────────────────────────────────────────────────────────
// Cliente HTTP/1.1 minimal (TCP, sin deps)
// ────────────────────────────────────────────────────────────────────────────

struct HttpResponse {
    status: u16,
    body: String,
}

fn http_request(
    server_url: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> Result<HttpResponse, String> {
    let (host, port) = parse_authority(server_url)?;
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .ok();
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .ok();

    let body_bytes = body.unwrap_or("").as_bytes();
    let mut req = String::new();
    req.push_str(&format!("{method} {path} HTTP/1.1\r\n"));
    req.push_str(&format!("Host: {host}:{port}\r\n"));
    req.push_str("User-Agent: gabysql-mcp/");
    req.push_str(SERVER_VERSION);
    req.push_str("\r\n");
    req.push_str("Connection: close\r\n");
    if let Some(t) = token {
        req.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    if body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", body_bytes.len()));
    }
    req.push_str("\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    if !body_bytes.is_empty() {
        stream.write_all(body_bytes).map_err(|e| e.to_string())?;
    }
    stream.flush().ok();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let split = find_subslice(&buf, b"\r\n\r\n").ok_or("respuesta HTTP sin headers")?;
    let head = std::str::from_utf8(&buf[..split]).map_err(|e| e.to_string())?;
    let body = String::from_utf8_lossy(&buf[split + 4..]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or("status line vacía")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or("status code faltante")?
        .parse()
        .map_err(|_| "status code no numérico".to_string())?;
    Ok(HttpResponse { status, body })
}

fn parse_authority(url: &str) -> Result<(String, u16), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("URL no http://: {url}"))?;
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return Err(format!("URL sin host: {url}"));
    }
    if let Some((h, p)) = authority.rsplit_once(':') {
        let port: u16 = p.parse().map_err(|_| format!("puerto inválido en {url}"))?;
        Ok((h.to_string(), port))
    } else {
        Ok((authority.to_string(), 80))
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

// ────────────────────────────────────────────────────────────────────────────
// MCP / JSON-RPC dispatch
// ────────────────────────────────────────────────────────────────────────────

fn run_stdio(cfg: &Config) -> Result<(), String> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    let mut line = String::new();
    loop {
        line.clear();
        let n = input.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(()); // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(reply) = dispatch(trimmed, cfg) {
            writeln!(output, "{reply}").map_err(|e| e.to_string())?;
            output.flush().ok();
        }
    }
}

fn dispatch(raw: &str, cfg: &Config) -> Option<String> {
    let req = match json_parse(raw) {
        Ok(j) => j,
        Err(e) => return Some(rpc_error_no_id(-32700, &format!("parse error: {e}"))),
    };
    let id = req.get("id").cloned();
    let method = match req.get("method").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => return Some(rpc_error(id, -32600, "request inválido: falta method")),
    };
    // Notifications (sin id) no llevan respuesta. MCP usa
    // notifications/initialized y notifications/cancelled.
    if id.is_none() && method.starts_with("notifications/") {
        return None;
    }
    let params = req.get("params").cloned().unwrap_or(Json::Obj(Json::obj()));
    let result = match method.as_str() {
        "initialize" => Ok(handle_initialize()),
        "ping" => Ok(Json::Obj(Json::obj())),
        "tools/list" => Ok(handle_tools_list(cfg)),
        "tools/call" => handle_tools_call(&params, cfg),
        "resources/list" => Ok(handle_resources_list()),
        "resources/read" => handle_resources_read(&params, cfg),
        other => Err(format!("método no soportado: {other}")),
    };
    match result {
        Ok(r) => Some(rpc_result(id, r)),
        Err(msg) => Some(rpc_error(id, -32000, &msg)),
    }
}

fn rpc_envelope(id: Option<Json>) -> Vec<(String, Json)> {
    let mut out = Json::obj();
    out.push(("jsonrpc".into(), Json::Str("2.0".into())));
    out.push(("id".into(), id.unwrap_or(Json::Null)));
    out
}

fn rpc_result(id: Option<Json>, result: Json) -> String {
    let mut env = rpc_envelope(id);
    env.push(("result".into(), result));
    json_to_string(&Json::Obj(env))
}

fn rpc_error(id: Option<Json>, code: i64, message: &str) -> String {
    let mut env = rpc_envelope(id);
    let mut err = Json::obj();
    err.push(("code".into(), Json::Num(code as f64)));
    err.push(("message".into(), Json::Str(message.to_string())));
    env.push(("error".into(), Json::Obj(err)));
    json_to_string(&Json::Obj(env))
}

fn rpc_error_no_id(code: i64, message: &str) -> String {
    rpc_error(None, code, message)
}

// ────────────────────────────────────────────────────────────────────────────
// MCP handlers
// ────────────────────────────────────────────────────────────────────────────

fn handle_initialize() -> Json {
    let mut info = Json::obj();
    info.push(("name".into(), Json::Str(SERVER_NAME.into())));
    info.push(("version".into(), Json::Str(SERVER_VERSION.into())));

    let mut tools_cap = Json::obj();
    tools_cap.push(("listChanged".into(), Json::Bool(false)));
    let mut res_cap = Json::obj();
    res_cap.push(("subscribe".into(), Json::Bool(false)));
    res_cap.push(("listChanged".into(), Json::Bool(false)));
    let mut caps = Json::obj();
    caps.push(("tools".into(), Json::Obj(tools_cap)));
    caps.push(("resources".into(), Json::Obj(res_cap)));

    let mut out = Json::obj();
    out.push(("protocolVersion".into(), Json::Str(PROTO_VERSION.into())));
    out.push(("serverInfo".into(), Json::Obj(info)));
    out.push(("capabilities".into(), Json::Obj(caps)));
    Json::Obj(out)
}

fn tool_def(name: &str, description: &str, schema: Json) -> Json {
    let mut t = Json::obj();
    t.push(("name".into(), Json::Str(name.into())));
    t.push(("description".into(), Json::Str(description.into())));
    t.push(("inputSchema".into(), schema));
    Json::Obj(t)
}

fn schema_object(properties: Vec<(&str, &str, &str)>, required: &[&str]) -> Json {
    // properties: list of (name, json_type, description)
    let mut props = Json::obj();
    for (name, ty, desc) in &properties {
        let mut p = Json::obj();
        p.push(("type".into(), Json::Str((*ty).into())));
        p.push(("description".into(), Json::Str((*desc).into())));
        props.push((name.to_string(), Json::Obj(p)));
    }
    let mut req = Vec::new();
    for r in required {
        req.push(Json::Str((*r).into()));
    }
    let mut s = Json::obj();
    s.push(("type".into(), Json::Str("object".into())));
    s.push(("properties".into(), Json::Obj(props)));
    s.push(("required".into(), Json::Arr(req)));
    Json::Obj(s)
}

fn handle_tools_list(cfg: &Config) -> Json {
    let empty_schema = schema_object(vec![], &[]);
    let db_only = schema_object(
        vec![("db", "string", "Nombre del archivo .db (opcional en single-db mode)")],
        &[],
    );
    let db_required = schema_object(
        vec![("db", "string", "Nombre del archivo .db (opcional en single-db mode)")],
        &[],
    );
    let query_schema = schema_object(
        vec![
            ("db", "string", "Nombre del archivo .db (opcional en single-db mode)"),
            ("sql", "string", "Sentencia SELECT/SHOW/DESCRIBE a ejecutar"),
        ],
        &["sql"],
    );
    let exec_schema = schema_object(
        vec![
            ("db", "string", "Nombre del archivo .db (opcional en single-db mode)"),
            ("sql", "string", "Sentencia DDL/DML (INSERT/UPDATE/DELETE/CREATE/ALTER/DROP)"),
        ],
        &["sql"],
    );

    let mut tools = vec![
        tool_def(
            "gabysql_list_databases",
            "Lista las bases de datos disponibles en el server gabysql.",
            empty_schema,
        ),
        tool_def(
            "gabysql_describe_database",
            "Devuelve el catálogo completo (tablas + columnas + tipos) de una DB en JSON estructurado.",
            db_only,
        ),
        tool_def(
            "gabysql_query",
            "Ejecuta una sentencia de lectura (SELECT/SHOW/DESCRIBE) y devuelve el ResultSet en JSON.",
            query_schema,
        ),
    ];
    if !cfg.read_only {
        tools.push(tool_def(
            "gabysql_execute",
            "Ejecuta una sentencia mutadora (INSERT/UPDATE/DELETE/DDL). Deshabilitada si --read-only.",
            exec_schema,
        ));
    }
    tools.push(tool_def(
        "gabysql_integrity_check",
        "Ejecuta INTEGRITY CHECK sobre una DB y devuelve el reporte.",
        db_required,
    ));

    let mut out = Json::obj();
    out.push(("tools".into(), Json::Arr(tools)));
    Json::Obj(out)
}

fn handle_tools_call(params: &Json, cfg: &Config) -> Result<Json, String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("falta tools/call.params.name")?
        .to_string();
    let args = params.get("arguments").cloned().unwrap_or(Json::Obj(Json::obj()));
    let db = args.get("db").and_then(|v| v.as_str()).map(|s| s.to_string());
    let sql_arg = args.get("sql").and_then(|v| v.as_str()).map(|s| s.to_string());

    let body = match name.as_str() {
        "gabysql_list_databases" => {
            let resp = http_request(&cfg.server_url, "GET", "/dbs", None, cfg.token.as_deref())?;
            check_http_ok(&resp)?;
            resp.body
        }
        "gabysql_describe_database" => describe_database(cfg, db.as_deref())?,
        "gabysql_query" => exec_via_server(cfg, db.as_deref(), &sql_arg.unwrap_or_default())?,
        "gabysql_execute" => {
            if cfg.read_only {
                return Err("gabysql_execute deshabilitada (--read-only)".into());
            }
            exec_via_server(cfg, db.as_deref(), &sql_arg.unwrap_or_default())?
        }
        "gabysql_integrity_check" => exec_via_server(cfg, db.as_deref(), "INTEGRITY CHECK")?,
        other => return Err(format!("tool desconocida: {other}")),
    };

    Ok(content_text(&body))
}

fn describe_database(cfg: &Config, db: Option<&str>) -> Result<String, String> {
    let q = db.map(|d| format!("?db={d}")).unwrap_or_default();
    let tables = http_request(
        &cfg.server_url,
        "GET",
        &format!("/tables{q}"),
        None,
        cfg.token.as_deref(),
    )?;
    check_http_ok(&tables)?;
    Ok(tables.body)
}

fn exec_via_server(cfg: &Config, db: Option<&str>, sql: &str) -> Result<String, String> {
    if sql.trim().is_empty() {
        return Err("sql vacío".into());
    }
    let mut payload = Json::obj();
    payload.push(("sql".into(), Json::Str(sql.to_string())));
    if let Some(d) = db {
        payload.push(("db".into(), Json::Str(d.to_string())));
    }
    let body = json_to_string(&Json::Obj(payload));
    let resp = http_request(
        &cfg.server_url,
        "POST",
        "/exec",
        Some(&body),
        cfg.token.as_deref(),
    )?;
    // /exec puede devolver 400 con {ok:false,error:...} ante SQL inválido;
    // pasamos el body al modelo igual — el agente decide si reintenta.
    Ok(resp.body)
}

fn check_http_ok(resp: &HttpResponse) -> Result<(), String> {
    if (200..300).contains(&resp.status) {
        Ok(())
    } else {
        Err(format!(
            "gabysql-server respondió {}: {}",
            resp.status, resp.body
        ))
    }
}

fn content_text(text: &str) -> Json {
    let mut item = Json::obj();
    item.push(("type".into(), Json::Str("text".into())));
    item.push(("text".into(), Json::Str(text.to_string())));
    let mut out = Json::obj();
    out.push(("content".into(), Json::Arr(vec![Json::Obj(item)])));
    out.push(("isError".into(), Json::Bool(false)));
    Json::Obj(out)
}

// ────────────────────────────────────────────────────────────────────────────
// Resources
// ────────────────────────────────────────────────────────────────────────────

fn handle_resources_list() -> Json {
    let mut catalog = Json::obj();
    catalog.push(("uri".into(), Json::Str("gabysql://catalog".into())));
    catalog.push(("name".into(), Json::Str("Catálogo de bases".into())));
    catalog.push((
        "description".into(),
        Json::Str("Lista de archivos .db disponibles en el server.".into()),
    ));
    catalog.push(("mimeType".into(), Json::Str("application/json".into())));

    let mut schema = Json::obj();
    schema.push(("uriTemplate".into(), Json::Str("gabysql://schema/{db}".into())));
    schema.push(("name".into(), Json::Str("Schema por DB".into())));
    schema.push((
        "description".into(),
        Json::Str("Tablas y columnas de una DB concreta, en JSON estructurado.".into()),
    ));
    schema.push(("mimeType".into(), Json::Str("application/json".into())));

    let mut out = Json::obj();
    out.push(("resources".into(), Json::Arr(vec![Json::Obj(catalog)])));
    out.push(("resourceTemplates".into(), Json::Arr(vec![Json::Obj(schema)])));
    Json::Obj(out)
}

fn handle_resources_read(params: &Json, cfg: &Config) -> Result<Json, String> {
    let uri = params
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or("falta resources/read.params.uri")?
        .to_string();
    let body = if uri == "gabysql://catalog" {
        let r = http_request(&cfg.server_url, "GET", "/dbs", None, cfg.token.as_deref())?;
        check_http_ok(&r)?;
        r.body
    } else if let Some(db) = uri.strip_prefix("gabysql://schema/") {
        if db.is_empty() {
            return Err("URI schema sin db".into());
        }
        describe_database(cfg, Some(db))?
    } else {
        return Err(format!("URI no reconocida: {uri}"));
    };
    let mut item = Json::obj();
    item.push(("uri".into(), Json::Str(uri)));
    item.push(("mimeType".into(), Json::Str("application/json".into())));
    item.push(("text".into(), Json::Str(body)));
    let mut out = Json::obj();
    out.push(("contents".into(), Json::Arr(vec![Json::Obj(item)])));
    Ok(Json::Obj(out))
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_basic() {
        let v = json_parse(r#"{"a":1,"b":"x","c":[true,null,2.5]}"#).unwrap();
        assert_eq!(v.get("a"), Some(&Json::Num(1.0)));
        assert_eq!(v.get("b").and_then(|j| j.as_str()), Some("x"));
        let serialized = json_to_string(&v);
        let v2 = json_parse(&serialized).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn json_string_escapes() {
        let v = json_parse(r#""hola\n\"mundo\\""#).unwrap();
        assert_eq!(v.as_str(), Some("hola\n\"mundo\\"));
    }

    #[test]
    fn dispatch_initialize_returns_proto_version() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
        };
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let reply = dispatch(req, &cfg).unwrap();
        let parsed = json_parse(&reply).unwrap();
        assert_eq!(parsed.get("id"), Some(&Json::Num(1.0)));
        let result = parsed.get("result").unwrap();
        assert_eq!(
            result.get("protocolVersion").and_then(|j| j.as_str()),
            Some(PROTO_VERSION)
        );
    }

    #[test]
    fn dispatch_tools_list_includes_execute_when_writable() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
        };
        let reply = dispatch(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            &cfg,
        )
        .unwrap();
        assert!(reply.contains("gabysql_execute"));
        assert!(reply.contains("gabysql_query"));
        assert!(reply.contains("gabysql_integrity_check"));
    }

    #[test]
    fn dispatch_tools_list_omits_execute_in_read_only() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: true,
            show_help: false,
        };
        let reply = dispatch(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
            &cfg,
        )
        .unwrap();
        assert!(!reply.contains("gabysql_execute"));
        assert!(reply.contains("gabysql_query"));
    }

    #[test]
    fn dispatch_resources_list_advertises_catalog_and_schema() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
        };
        let reply = dispatch(
            r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#,
            &cfg,
        )
        .unwrap();
        assert!(reply.contains("gabysql://catalog"));
        assert!(reply.contains("gabysql://schema/{db}"));
    }

    #[test]
    fn dispatch_ping_returns_empty_result() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
        };
        let reply = dispatch(r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#, &cfg).unwrap();
        let parsed = json_parse(&reply).unwrap();
        assert_eq!(parsed.get("id"), Some(&Json::Num(5.0)));
        assert!(matches!(parsed.get("result"), Some(Json::Obj(_))));
    }

    #[test]
    fn dispatch_unknown_method_returns_error() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
        };
        let reply = dispatch(
            r#"{"jsonrpc":"2.0","id":6,"method":"definitely/not/a/method"}"#,
            &cfg,
        )
        .unwrap();
        let parsed = json_parse(&reply).unwrap();
        assert!(parsed.get("error").is_some());
    }

    #[test]
    fn dispatch_notification_returns_no_reply() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
        };
        let reply = dispatch(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &cfg,
        );
        assert!(reply.is_none());
    }

    #[test]
    fn parse_authority_default_port() {
        assert_eq!(
            parse_authority("http://127.0.0.1:7878").unwrap(),
            ("127.0.0.1".into(), 7878u16)
        );
        assert_eq!(
            parse_authority("http://example.com").unwrap(),
            ("example.com".into(), 80u16)
        );
    }
}
