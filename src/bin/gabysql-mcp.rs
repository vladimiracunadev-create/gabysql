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
use std::fs::OpenOptions;
use std::io::{self, BufRead, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
           --audit-log P   Ruta a un archivo JSONL donde anexar cada llamada\n\
                           mutadora (gabysql_execute, INTEGRITY CHECK). Captura\n\
                           clientInfo de initialize y el campo 'reason' que el\n\
                           agente puede pasar como justificación semántica.\n\
                           Override de GABYSQL_AUDIT_LOG. Ver ADR-0012.\n\
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
    audit_log: Option<PathBuf>,
}

impl Config {
    fn from_args(argv: &[String]) -> Result<Self, String> {
        let mut server_url = env::var("GABYSQL_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.into());
        let mut token = env::var("GABYSQL_TOKEN").ok().filter(|t| !t.is_empty());
        let mut audit_log = env::var("GABYSQL_AUDIT_LOG")
            .ok()
            .filter(|t| !t.is_empty())
            .map(PathBuf::from);
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
                "--audit-log" => {
                    i += 1;
                    audit_log = Some(PathBuf::from(
                        argv.get(i)
                            .cloned()
                            .ok_or_else(|| "--audit-log requiere ruta".to_string())?,
                    ));
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
            audit_log,
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Estado runtime (mutable a través del loop) — ADR-0012
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
struct RuntimeState {
    client_info: Option<ClientInfo>,
}

#[derive(Debug, Clone)]
struct ClientInfo {
    name: String,
    version: String,
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
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(30))).ok();

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
    let state = Mutex::new(RuntimeState::default());
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
        if let Some(reply) = dispatch(trimmed, cfg, &state) {
            writeln!(output, "{reply}").map_err(|e| e.to_string())?;
            output.flush().ok();
        }
    }
}

fn dispatch(raw: &str, cfg: &Config, state: &Mutex<RuntimeState>) -> Option<String> {
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
        "initialize" => Ok(handle_initialize(&params, state)),
        "ping" => Ok(Json::Obj(Json::obj())),
        "tools/list" => Ok(handle_tools_list(cfg)),
        "tools/call" => handle_tools_call(&params, cfg, state),
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

fn handle_initialize(params: &Json, state: &Mutex<RuntimeState>) -> Json {
    // Capturamos clientInfo para enriquecer el audit log (ADR-0012). Si el
    // cliente no manda clientInfo, seguimos — el audit log usa "unknown".
    if let Some(ci) = params.get("clientInfo") {
        let name = ci
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let version = ci
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        if let Ok(mut s) = state.lock() {
            s.client_info = Some(ClientInfo { name, version });
        }
    }

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
        vec![(
            "db",
            "string",
            "Nombre del archivo .db (opcional en single-db mode)",
        )],
        &[],
    );
    let db_required = schema_object(
        vec![(
            "db",
            "string",
            "Nombre del archivo .db (opcional en single-db mode)",
        )],
        &[],
    );
    let query_schema = schema_object(
        vec![
            (
                "db",
                "string",
                "Nombre del archivo .db (opcional en single-db mode)",
            ),
            ("sql", "string", "Sentencia SELECT/SHOW/DESCRIBE a ejecutar"),
        ],
        &["sql"],
    );
    let exec_schema = schema_object(
        vec![
            (
                "db",
                "string",
                "Nombre del archivo .db (opcional en single-db mode)",
            ),
            (
                "sql",
                "string",
                "Sentencia DDL/DML (INSERT/UPDATE/DELETE/CREATE/ALTER/DROP)",
            ),
            (
                "reason",
                "string",
                "Justificación semántica opcional. Si --audit-log está activo, queda registrada con el SQL en el JSONL. Ver ADR-0012.",
            ),
        ],
        &["sql"],
    );
    let audit_tail_schema = schema_object(
        vec![(
            "n",
            "integer",
            "Cuántas entradas recientes devolver (default 50)",
        )],
        &[],
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
    tools.push(tool_def(
        "gabysql_vector_search",
        "Búsqueda vectorial top-k sobre una columna TEXT que contiene un array JSON de floats. \
         Hace SELECT completo + cálculo de distancia en el gateway (no toca el motor). Métricas: \
         cosine (default), euclidean, dot. Adecuada hasta decenas de miles de filas; ver ADR-0011.",
        vector_search_schema(),
    ));
    tools.push(tool_def(
        "gabysql_audit_tail",
        "Devuelve las últimas N entradas del audit log (JSONL del gateway). \
         Útil para que el agente revise qué mutaciones realizó. Sin efecto si \
         no se lanzó con --audit-log. Ver ADR-0012.",
        audit_tail_schema,
    ));

    let mut out = Json::obj();
    out.push(("tools".into(), Json::Arr(tools)));
    Json::Obj(out)
}

fn vector_search_schema() -> Json {
    let mut props = Json::obj();
    let mut add = |name: &str, ty: &str, desc: &str| {
        let mut p = Json::obj();
        p.push(("type".into(), Json::Str(ty.into())));
        p.push(("description".into(), Json::Str(desc.into())));
        props.push((name.into(), Json::Obj(p)));
    };
    add("db", "string", "Nombre del archivo .db (opcional en single-db mode)");
    add("table", "string", "Tabla a escanear");
    add(
        "pk_column",
        "string",
        "Columna PK a devolver para identificar la fila (default 'id')",
    );
    add(
        "vector_column",
        "string",
        "Columna TEXT que contiene el vector como array JSON de floats",
    );
    // query es array<number>; describimos type genérico para máxima compat con clientes MCP
    let mut q = Json::obj();
    q.push(("type".into(), Json::Str("array".into())));
    q.push((
        "description".into(),
        Json::Str("Vector de consulta como array de floats".into()),
    ));
    let mut items = Json::obj();
    items.push(("type".into(), Json::Str("number".into())));
    q.push(("items".into(), Json::Obj(items)));
    props.push(("query".into(), Json::Obj(q)));
    add("top_k", "integer", "Cuántos resultados devolver (default 10)");
    add(
        "metric",
        "string",
        "Distancia: 'cosine' (default), 'euclidean', 'dot'",
    );

    let req = vec![
        Json::Str("table".into()),
        Json::Str("vector_column".into()),
        Json::Str("query".into()),
    ];
    let mut s = Json::obj();
    s.push(("type".into(), Json::Str("object".into())));
    s.push(("properties".into(), Json::Obj(props)));
    s.push(("required".into(), Json::Arr(req)));
    Json::Obj(s)
}

fn handle_tools_call(
    params: &Json,
    cfg: &Config,
    state: &Mutex<RuntimeState>,
) -> Result<Json, String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("falta tools/call.params.name")?
        .to_string();
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or(Json::Obj(Json::obj()));
    let db = args
        .get("db")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let sql_arg = args
        .get("sql")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let reason = args
        .get("reason")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let exec_result = match name.as_str() {
        "gabysql_list_databases" => {
            let resp = http_request(&cfg.server_url, "GET", "/dbs", None, cfg.token.as_deref())?;
            check_http_ok(&resp)?;
            Ok(resp.body)
        }
        "gabysql_describe_database" => describe_database(cfg, db.as_deref()),
        "gabysql_query" => {
            exec_via_server(cfg, db.as_deref(), &sql_arg.clone().unwrap_or_default())
        }
        "gabysql_execute" => {
            if cfg.read_only {
                return Err("gabysql_execute deshabilitada (--read-only)".into());
            }
            exec_via_server(cfg, db.as_deref(), &sql_arg.clone().unwrap_or_default())
        }
        "gabysql_integrity_check" => exec_via_server(cfg, db.as_deref(), "INTEGRITY CHECK"),
        "gabysql_vector_search" => vector_search(cfg, db.as_deref(), &args),
        "gabysql_audit_tail" => {
            let n = args
                .get("n")
                .and_then(|v| if let Json::Num(n) = v { Some(*n as usize) } else { None })
                .unwrap_or(50);
            return audit_tail(cfg, n).map(|s| content_text(&s));
        }
        other => return Err(format!("tool desconocida: {other}")),
    };

    // Audit log para mutaciones — sin efecto si --audit-log no está activo.
    if matches!(
        name.as_str(),
        "gabysql_execute" | "gabysql_integrity_check"
    ) {
        let entry = AuditEntry {
            ts_unix: now_unix(),
            tool: name.clone(),
            db: db.clone(),
            sql: sql_arg.clone(),
            reason: reason.clone(),
            client: state.lock().ok().and_then(|s| s.client_info.clone()),
            ok: exec_result.is_ok(),
            error: exec_result.as_ref().err().cloned(),
        };
        // Append best-effort: si falla escribir el log, no rompemos el flujo
        // de la tool (el agente ya hizo la llamada al motor).
        if let Some(path) = cfg.audit_log.as_ref() {
            if let Err(e) = audit_append(path, &entry) {
                eprintln!("gabysql-mcp: audit append falló: {e}");
            }
        }
    }

    let body = exec_result?;
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
    schema.push((
        "uriTemplate".into(),
        Json::Str("gabysql://schema/{db}".into()),
    ));
    schema.push(("name".into(), Json::Str("Schema por DB".into())));
    schema.push((
        "description".into(),
        Json::Str("Tablas y columnas de una DB concreta, en JSON estructurado.".into()),
    ));
    schema.push(("mimeType".into(), Json::Str("application/json".into())));

    let mut out = Json::obj();
    out.push(("resources".into(), Json::Arr(vec![Json::Obj(catalog)])));
    out.push((
        "resourceTemplates".into(),
        Json::Arr(vec![Json::Obj(schema)]),
    ));
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
// Vector search del lado del gateway (ADR-0011)
//
// Tesis: meter búsqueda vectorial sin tocar el motor. El vector vive en una
// columna TEXT como `[0.1,0.2,...]`. El gateway hace SELECT completo y
// computa distancias en Rust. Es O(n·d) por query — adecuado hasta decenas
// de miles de filas. Para escala mayor se promueve a `VECTOR(n)` nativo
// con index ANN (otra ADR, otro bump de formato, otro día).
// ────────────────────────────────────────────────────────────────────────────

fn vector_search(cfg: &Config, db: Option<&str>, args: &Json) -> Result<String, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("falta argumento 'table'")?
        .to_string();
    let vector_column = args
        .get("vector_column")
        .and_then(|v| v.as_str())
        .ok_or("falta argumento 'vector_column'")?
        .to_string();
    let pk_column = args
        .get("pk_column")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "id".to_string());
    let query_arr = args.get("query").ok_or("falta argumento 'query'")?;
    let query = parse_vector_json(query_arr)?;
    if query.is_empty() {
        return Err("'query' no puede estar vacío".into());
    }
    let top_k = args
        .get("top_k")
        .and_then(|v| if let Json::Num(n) = v { Some(*n as usize) } else { None })
        .unwrap_or(10);
    if top_k == 0 {
        return Err("'top_k' debe ser >= 1".into());
    }
    let metric_raw = args
        .get("metric")
        .and_then(|v| v.as_str())
        .unwrap_or("cosine")
        .to_ascii_lowercase();
    let metric = parse_metric(&metric_raw)?;

    // Identificadores tienen que ser seguros — los interpolamos al SQL.
    safe_ident(&table).map_err(|e| format!("table: {e}"))?;
    safe_ident(&vector_column).map_err(|e| format!("vector_column: {e}"))?;
    safe_ident(&pk_column).map_err(|e| format!("pk_column: {e}"))?;

    let sql = format!("SELECT {pk_column}, {vector_column} FROM {table}");
    let raw = exec_via_server(cfg, db, &sql)?;
    let parsed = json_parse(&raw).map_err(|e| format!("respuesta del server no es JSON: {e}"))?;
    if parsed.get("ok").and_then(|v| if let Json::Bool(b) = v { Some(*b) } else { None })
        != Some(true)
    {
        return Err(format!("server reportó error: {raw}"));
    }
    let result_set = parsed
        .get("results")
        .and_then(|v| if let Json::Arr(a) = v { a.first() } else { None })
        .ok_or("respuesta sin results[0]")?;
    let rows = result_set
        .get("rows")
        .and_then(|v| if let Json::Arr(a) = v { Some(a) } else { None })
        .ok_or("results[0].rows ausente")?;

    // Heap de tamaño top_k: distancia, pk_json, vector_json (para devolver verbatim)
    let mut top: Vec<VectorMatch> = Vec::with_capacity(top_k + 1);
    let mut skipped = 0usize;
    for row in rows {
        let row_arr = match row {
            Json::Arr(a) if a.len() >= 2 => a,
            _ => {
                skipped += 1;
                continue;
            }
        };
        let pk = &row_arr[0];
        let vec_text = match &row_arr[1] {
            Json::Str(s) => s,
            Json::Null => {
                skipped += 1;
                continue;
            }
            _ => {
                skipped += 1;
                continue;
            }
        };
        let vec_json = match json_parse(vec_text) {
            Ok(j) => j,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let v = match parse_vector_json(&vec_json) {
            Ok(v) => v,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if v.len() != query.len() {
            skipped += 1;
            continue;
        }
        let d = match distance(metric, &query, &v) {
            Some(d) => d,
            None => {
                skipped += 1;
                continue;
            }
        };
        push_top_k(&mut top, top_k, VectorMatch { distance: d, pk: pk.clone(), vector: vec_json });
    }
    // Orden ascendente: distancia menor = más parecido (cosine y euclidean).
    // Para "dot" invertimos el signo de la distancia para que también ordene
    // ascendente (mayor producto interno → distancia "más negativa").
    top.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));

    let mut matches = Vec::with_capacity(top.len());
    for m in &top {
        let mut item = Json::obj();
        item.push(("pk".into(), m.pk.clone()));
        item.push(("distance".into(), Json::Num(m.distance)));
        item.push(("vector".into(), m.vector.clone()));
        matches.push(Json::Obj(item));
    }
    let mut out = Json::obj();
    out.push(("ok".into(), Json::Bool(true)));
    out.push(("metric".into(), Json::Str(metric.as_name().into())));
    out.push(("scanned".into(), Json::Num(rows.len() as f64)));
    out.push(("skipped".into(), Json::Num(skipped as f64)));
    out.push(("matches".into(), Json::Arr(matches)));
    Ok(json_to_string(&Json::Obj(out)))
}

#[derive(Clone)]
struct VectorMatch {
    distance: f64,
    pk: Json,
    vector: Json,
}

fn push_top_k(top: &mut Vec<VectorMatch>, k: usize, item: VectorMatch) {
    top.push(item);
    if top.len() > k {
        // Encuentra el peor (mayor distancia) y descártalo.
        let mut worst_idx = 0;
        let mut worst = top[0].distance;
        for (i, m) in top.iter().enumerate().skip(1) {
            if m.distance > worst {
                worst = m.distance;
                worst_idx = i;
            }
        }
        top.swap_remove(worst_idx);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Metric {
    Cosine,
    Euclidean,
    Dot,
}

impl Metric {
    fn as_name(self) -> &'static str {
        match self {
            Metric::Cosine => "cosine",
            Metric::Euclidean => "euclidean",
            Metric::Dot => "dot",
        }
    }
}

fn parse_metric(raw: &str) -> Result<Metric, String> {
    match raw {
        "cosine" => Ok(Metric::Cosine),
        "euclidean" | "l2" => Ok(Metric::Euclidean),
        "dot" | "ip" => Ok(Metric::Dot),
        other => Err(format!("metric desconocida: {other} (usa cosine|euclidean|dot)")),
    }
}

fn distance(metric: Metric, a: &[f64], b: &[f64]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    match metric {
        Metric::Euclidean => {
            let mut sum = 0.0;
            for i in 0..a.len() {
                let d = a[i] - b[i];
                sum += d * d;
            }
            Some(sum.sqrt())
        }
        Metric::Dot => {
            let mut dot = 0.0;
            for i in 0..a.len() {
                dot += a[i] * b[i];
            }
            // Negamos para que sort ascendente devuelva mayor dot primero.
            Some(-dot)
        }
        Metric::Cosine => {
            let mut dot = 0.0;
            let mut na = 0.0;
            let mut nb = 0.0;
            for i in 0..a.len() {
                dot += a[i] * b[i];
                na += a[i] * a[i];
                nb += b[i] * b[i];
            }
            if na == 0.0 || nb == 0.0 {
                return None;
            }
            let cos_sim = dot / (na.sqrt() * nb.sqrt());
            // Distancia coseno = 1 - similitud. Rango [0, 2].
            Some(1.0 - cos_sim)
        }
    }
}

fn parse_vector_json(v: &Json) -> Result<Vec<f64>, String> {
    let arr = if let Json::Arr(a) = v {
        a
    } else {
        return Err("se esperaba array de números".into());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        match item {
            Json::Num(n) => out.push(*n),
            _ => return Err("array contiene un valor no numérico".into()),
        }
    }
    Ok(out)
}

fn safe_ident(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("identificador vacío".into());
    }
    let first = s.chars().next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(format!("'{s}' no empieza con letra o '_'"));
    }
    for c in s.chars() {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("'{s}' contiene carácter inválido '{c}'"));
        }
    }
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Audit log enriquecido (ADR-0012)
//
// Cada llamada mutadora (gabysql_execute, gabysql_integrity_check) puede
// dejar una entrada JSON en un archivo JSONL. La entrada lleva:
//
//   - ts_unix       (epoch seconds)
//   - tool          (nombre de la tool MCP)
//   - db, sql       (qué se ejecutó)
//   - reason        ("por qué" semántico que el agente puede pasar)
//   - client        (clientInfo capturado en initialize)
//   - ok / error    (resultado)
//
// El append es best-effort: si el filesystem rompe, el agente ya pegó al
// motor y no podemos deshacer eso — solo loggeamos a stderr.
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct AuditEntry {
    ts_unix: u64,
    tool: String,
    db: Option<String>,
    sql: Option<String>,
    reason: Option<String>,
    client: Option<ClientInfo>,
    ok: bool,
    error: Option<String>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn audit_entry_to_json(e: &AuditEntry) -> Json {
    let mut o = Json::obj();
    o.push(("ts_unix".into(), Json::Num(e.ts_unix as f64)));
    o.push(("tool".into(), Json::Str(e.tool.clone())));
    o.push((
        "db".into(),
        match &e.db {
            Some(s) => Json::Str(s.clone()),
            None => Json::Null,
        },
    ));
    o.push((
        "sql".into(),
        match &e.sql {
            Some(s) => Json::Str(s.clone()),
            None => Json::Null,
        },
    ));
    o.push((
        "reason".into(),
        match &e.reason {
            Some(s) => Json::Str(s.clone()),
            None => Json::Null,
        },
    ));
    o.push((
        "client".into(),
        match &e.client {
            Some(c) => {
                let mut ci = Json::obj();
                ci.push(("name".into(), Json::Str(c.name.clone())));
                ci.push(("version".into(), Json::Str(c.version.clone())));
                Json::Obj(ci)
            }
            None => Json::Null,
        },
    ));
    o.push(("ok".into(), Json::Bool(e.ok)));
    o.push((
        "error".into(),
        match &e.error {
            Some(s) => Json::Str(s.clone()),
            None => Json::Null,
        },
    ));
    Json::Obj(o)
}

fn audit_append(path: &std::path::Path, entry: &AuditEntry) -> Result<(), String> {
    let line = json_to_string(&audit_entry_to_json(entry));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    writeln!(file, "{line}").map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

fn audit_tail(cfg: &Config, n: usize) -> Result<String, String> {
    let path = match cfg.audit_log.as_ref() {
        Some(p) => p,
        None => {
            // Sin audit log activo. Devolvemos respuesta válida y vacía,
            // no es un error — simplemente no hay nada que mostrar.
            let mut o = Json::obj();
            o.push(("ok".into(), Json::Bool(true)));
            o.push(("enabled".into(), Json::Bool(false)));
            o.push(("entries".into(), Json::Arr(vec![])));
            return Ok(json_to_string(&Json::Obj(o)));
        }
    };
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("leer {}: {e}", path.display())),
    };
    let lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();
    let take = n.min(lines.len());
    let start = lines.len() - take;
    let mut entries = Vec::with_capacity(take);
    for line in &lines[start..] {
        match json_parse(line) {
            Ok(j) => entries.push(j),
            Err(_) => continue, // línea corrupta — la saltamos sin romper
        }
    }
    let mut o = Json::obj();
    o.push(("ok".into(), Json::Bool(true)));
    o.push(("enabled".into(), Json::Bool(true)));
    o.push((
        "path".into(),
        Json::Str(path.display().to_string()),
    ));
    o.push(("entries".into(), Json::Arr(entries)));
    Ok(json_to_string(&Json::Obj(o)))
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
            audit_log: None,
        };
        let state = Mutex::new(RuntimeState::default());
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let reply = dispatch(req, &cfg, &state).unwrap();
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
            audit_log: None,
        };
        let state = Mutex::new(RuntimeState::default());
        let reply =
            dispatch(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#, &cfg, &state).unwrap();
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
            audit_log: None,
        };
        let state = Mutex::new(RuntimeState::default());
        let reply =
            dispatch(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#, &cfg, &state).unwrap();
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
            audit_log: None,
        };
        let state = Mutex::new(RuntimeState::default());
        let reply = dispatch(
            r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#,
            &cfg,
            &state,
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
            audit_log: None,
        };
        let state = Mutex::new(RuntimeState::default());
        let reply =
            dispatch(r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#, &cfg, &state).unwrap();
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
            audit_log: None,
        };
        let state = Mutex::new(RuntimeState::default());
        let reply = dispatch(
            r#"{"jsonrpc":"2.0","id":6,"method":"definitely/not/a/method"}"#,
            &cfg,
            &state,
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
            audit_log: None,
        };
        let state = Mutex::new(RuntimeState::default());
        let reply = dispatch(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &cfg,
            &state,
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

    #[test]
    fn cosine_identical_vectors_distance_zero() {
        let a = vec![1.0, 2.0, 3.0];
        let d = distance(Metric::Cosine, &a, &a).unwrap();
        assert!(d.abs() < 1e-9, "cosine de un vector consigo mismo debe ser ~0, fue {d}");
    }

    #[test]
    fn cosine_orthogonal_vectors_distance_one() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let d = distance(Metric::Cosine, &a, &b).unwrap();
        assert!((d - 1.0).abs() < 1e-9, "cosine de vectores ortogonales = 1, fue {d}");
    }

    #[test]
    fn euclidean_known_distance() {
        let a = vec![0.0, 0.0];
        let b = vec![3.0, 4.0];
        let d = distance(Metric::Euclidean, &a, &b).unwrap();
        assert!((d - 5.0).abs() < 1e-9, "euclidean (0,0)→(3,4) = 5, fue {d}");
    }

    #[test]
    fn dot_returns_negative_so_sort_picks_largest() {
        // dot(a,a)=14, dot(a,b)=0 → con negación, -14 < 0 → a sale primero
        let q = vec![1.0, 2.0, 3.0];
        let a = vec![1.0, 2.0, 3.0]; // dot 14
        let b = vec![3.0, -2.0, 1.0 / 3.0]; // dot ≈ 0
        let da = distance(Metric::Dot, &q, &a).unwrap();
        let db = distance(Metric::Dot, &q, &b).unwrap();
        assert!(da < db, "dot mayor → distancia menor (sort ascendente)");
    }

    #[test]
    fn distance_returns_none_on_dimension_mismatch() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        assert!(distance(Metric::Cosine, &a, &b).is_none());
    }

    #[test]
    fn cosine_zero_vector_returns_none() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 1.0];
        assert!(distance(Metric::Cosine, &a, &b).is_none());
    }

    #[test]
    fn push_top_k_keeps_smallest_distances() {
        let mk = |d: f64, id: i64| VectorMatch {
            distance: d,
            pk: Json::Num(id as f64),
            vector: Json::Arr(vec![]),
        };
        let mut top = Vec::new();
        for (d, id) in [(0.5, 1), (0.1, 2), (0.9, 3), (0.3, 4), (0.7, 5)] {
            push_top_k(&mut top, 3, mk(d, id));
        }
        assert_eq!(top.len(), 3);
        let mut dists: Vec<f64> = top.iter().map(|m| m.distance).collect();
        dists.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(dists, vec![0.1, 0.3, 0.5]);
    }

    #[test]
    fn safe_ident_accepts_valid_identifiers() {
        assert!(safe_ident("users").is_ok());
        assert!(safe_ident("user_id").is_ok());
        assert!(safe_ident("_private").is_ok());
        assert!(safe_ident("Col1").is_ok());
    }

    #[test]
    fn safe_ident_rejects_injection_attempts() {
        assert!(safe_ident("users; DROP TABLE x").is_err());
        assert!(safe_ident("users--").is_err());
        assert!(safe_ident("a b").is_err());
        assert!(safe_ident("1col").is_err());
        assert!(safe_ident("").is_err());
    }

    #[test]
    fn parse_metric_accepts_aliases() {
        assert_eq!(parse_metric("cosine").unwrap(), Metric::Cosine);
        assert_eq!(parse_metric("euclidean").unwrap(), Metric::Euclidean);
        assert_eq!(parse_metric("l2").unwrap(), Metric::Euclidean);
        assert_eq!(parse_metric("dot").unwrap(), Metric::Dot);
        assert_eq!(parse_metric("ip").unwrap(), Metric::Dot);
        assert!(parse_metric("manhattan").is_err());
    }

    #[test]
    fn tools_list_includes_vector_search() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
            audit_log: None,
        };
        let state = Mutex::new(RuntimeState::default());
        let reply = dispatch(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#,
            &cfg,
            &state,
        )
        .unwrap();
        assert!(reply.contains("gabysql_vector_search"));
        assert!(reply.contains("ADR-0011"));
    }

    #[test]
    fn parse_vector_json_validates_types() {
        let arr = json_parse("[1.0, 2.5, -3.0]").unwrap();
        let v = parse_vector_json(&arr).unwrap();
        assert_eq!(v, vec![1.0, 2.5, -3.0]);
        let bad = json_parse(r#"[1.0, "string"]"#).unwrap();
        assert!(parse_vector_json(&bad).is_err());
        let not_arr = json_parse("42").unwrap();
        assert!(parse_vector_json(&not_arr).is_err());
    }

    #[test]
    fn initialize_captures_client_info() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
            audit_log: None,
        };
        let state = Mutex::new(RuntimeState::default());
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "clientInfo":{"name":"claude-desktop","version":"1.2.3"}
        }}"#;
        let _ = dispatch(req, &cfg, &state).unwrap();
        let captured = state.lock().unwrap().client_info.clone().unwrap();
        assert_eq!(captured.name, "claude-desktop");
        assert_eq!(captured.version, "1.2.3");
    }

    #[test]
    fn audit_append_and_tail_roundtrip() {
        // Usamos un path único en temp para no chocar con paralelismo de tests.
        let mut path = std::env::temp_dir();
        path.push(format!(
            "gabysql-mcp-audit-{}-{}.jsonl",
            std::process::id(),
            now_unix()
        ));
        let _ = std::fs::remove_file(&path);
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
            audit_log: Some(path.clone()),
        };

        let entry = AuditEntry {
            ts_unix: 1730000000,
            tool: "gabysql_execute".into(),
            db: Some("rag.db".into()),
            sql: Some("INSERT INTO docs VALUES (1, '...')".into()),
            reason: Some("backfill inicial del corpus".into()),
            client: Some(ClientInfo {
                name: "claude-desktop".into(),
                version: "1.2.3".into(),
            }),
            ok: true,
            error: None,
        };
        audit_append(&path, &entry).unwrap();

        let raw = audit_tail(&cfg, 10).unwrap();
        let parsed = json_parse(&raw).unwrap();
        assert_eq!(parsed.get("ok"), Some(&Json::Bool(true)));
        assert_eq!(parsed.get("enabled"), Some(&Json::Bool(true)));
        let entries = match parsed.get("entries") {
            Some(Json::Arr(a)) => a.clone(),
            _ => panic!("entries no es array"),
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].get("reason").and_then(|j| j.as_str()),
            Some("backfill inicial del corpus")
        );
        assert_eq!(
            entries[0]
                .get("client")
                .and_then(|c| c.get("name"))
                .and_then(|j| j.as_str()),
            Some("claude-desktop")
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn audit_tail_returns_disabled_when_no_log_configured() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
            audit_log: None,
        };
        let raw = audit_tail(&cfg, 50).unwrap();
        let parsed = json_parse(&raw).unwrap();
        assert_eq!(parsed.get("enabled"), Some(&Json::Bool(false)));
    }

    #[test]
    fn tools_list_includes_audit_tail() {
        let cfg = Config {
            server_url: DEFAULT_SERVER.into(),
            token: None,
            read_only: false,
            show_help: false,
            audit_log: None,
        };
        let state = Mutex::new(RuntimeState::default());
        let reply =
            dispatch(r#"{"jsonrpc":"2.0","id":8,"method":"tools/list"}"#, &cfg, &state).unwrap();
        assert!(reply.contains("gabysql_audit_tail"));
        assert!(reply.contains("ADR-0012"));
    }

    #[test]
    fn audit_append_is_jsonl_one_entry_per_line() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "gabysql-mcp-audit-multi-{}-{}.jsonl",
            std::process::id(),
            now_unix()
        ));
        let _ = std::fs::remove_file(&path);

        for i in 0..3 {
            let e = AuditEntry {
                ts_unix: 1730000000 + i,
                tool: "gabysql_execute".into(),
                db: None,
                sql: Some(format!("INSERT INTO t VALUES ({i})")),
                reason: None,
                client: None,
                ok: true,
                error: None,
            };
            audit_append(&path, &e).unwrap();
        }
        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3);
        for line in &lines {
            // Cada línea debe ser JSON válido por sí misma.
            json_parse(line).unwrap();
        }
        let _ = std::fs::remove_file(&path);
    }
}
