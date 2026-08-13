//! Bloque L (2026-08-13): log de sentencias y errores del **motor**.
//!
//! Complementa las dos capas de observabilidad que ya existían y que
//! dejaban un hueco:
//!
//! - [ADR-0012] audit log del gateway MCP (`--audit-log`): captura SQL +
//!   `reason` + identidad del agente, pero **sólo el tráfico MCP**.
//! - [ADR-0014] logs JSON del server (`-log-json`): una línea por
//!   request HTTP con method/path/status/latency a stdout, **sin el SQL
//!   ni el código `[GBY-NNNN]`**.
//!
//! Lo que faltaba es el equivalente a `log_statement` /
//! `log_min_error_statement` de PostgreSQL: el log de **la base**, no el
//! del transporte. Este módulo lo provee como un sink JSONL append-only
//! con rotación por tamaño, enganchado en un único punto —
//! `Engine::exec` — de modo que cubre a la vez el CLI, el server, el uso
//! embebido como librería y el `.msi` de escritorio.
//!
//! La rotación es la diferencia operativa contra ADR-0014, que la
//! descartó argumentando "stdout + `logrotate`". Ese argumento no aplica
//! acá: en el uso embebido y en el desktop no hay supervisor que rote
//! nada, y el archivo crecería sin techo.
//!
//! Cero deps externas ([ADR-0001](../docs/adr/0001-rust-zero-deps-core.md)).
//! Ver [ADR-0094](../docs/adr/0094-engine-statement-log.md).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::errors::{coded, codes};
use crate::DbResult;

/// Versión del shape de cada entrada JSONL. Va en el campo `v` de cada
/// línea desde el día uno — ADR-0012 documentó como consecuencia
/// negativa el no haberlo hecho en el audit log del gateway, y esa
/// deuda no se repite acá.
pub const LOG_SCHEMA_VERSION: u32 = 1;

/// Tamaño por defecto al que se rota (8 MiB). Suficiente para que un
/// operador abra el archivo con cualquier editor sin pelearse.
pub const DEFAULT_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Cantidad de archivos rotados que se conservan por defecto
/// (`.1`, `.2`, `.3`). Con el default de 8 MiB son ~32 MiB de techo
/// total para el log.
pub const DEFAULT_MAX_FILES: usize = 3;

/// Qué sentencias llegan al log. Espeja `log_statement` de PostgreSQL,
/// con `error` agregado abajo de todo porque el caso de uso más común
/// ("quiero ver qué falló") no debería costar el volumen de `all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Nada. Equivalente a no configurar log.
    None,
    /// Sólo sentencias que terminaron en error.
    Error,
    /// Errores + sentencias que mutan (DDL, DML, control transaccional).
    Mod,
    /// Todo, incluidos los SELECT.
    All,
}

impl LogLevel {
    /// Parsea el valor de `-log-level` / `GABYSQL_LOG_LEVEL`.
    /// Case-insensitive.
    pub fn parse(raw: &str) -> DbResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Ok(LogLevel::None),
            "error" => Ok(LogLevel::Error),
            "mod" => Ok(LogLevel::Mod),
            "all" => Ok(LogLevel::All),
            other => Err(coded(
                codes::LOG_INVALID_LEVEL,
                format!("nivel de log desconocido: '{other}' (válidos: none, error, mod, all)"),
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::None => "none",
            LogLevel::Error => "error",
            LogLevel::Mod => "mod",
            LogLevel::All => "all",
        }
    }

    /// ¿Una sentencia con estas características entra al log?
    fn admits(&self, mutating: bool, ok: bool) -> bool {
        match self {
            LogLevel::None => false,
            LogLevel::Error => !ok,
            LogLevel::Mod => !ok || mutating,
            LogLevel::All => true,
        }
    }
}

/// Una sentencia ejecutada, lista para serializar a JSONL.
///
/// Los campos son prestados: el logger serializa y escribe sin quedarse
/// con nada, así que el caller no paga clones en el camino caliente.
#[derive(Debug)]
pub struct LogRecord<'a> {
    /// Forma de la sentencia — `"INSERT"`, `"CREATE TABLE"`, etc.
    pub kind: &'static str,
    /// Si la sentencia muta el estado de la DB. Decide el corte de
    /// `LogLevel::Mod`.
    pub mutating: bool,
    /// Texto SQL de origen del batch que contiene esta sentencia.
    /// `None` si el caller no lo aportó (uso embebido que llama a
    /// `Engine::exec` con un `Statement` ya construido).
    pub sql: Option<&'a str>,
    /// Índice 0-based de la sentencia dentro del batch de `sql`. Un
    /// `parse()` puede devolver varios `Statement` para un solo texto;
    /// este campo desambigua cuál de ellos generó la entrada.
    pub stmt_index: usize,
    pub ok: bool,
    /// Mensaje de error completo, tal cual lo produjo el motor.
    pub error: Option<&'a str>,
    /// Filas del `ResultSet` devuelto (0 si hubo error).
    pub rows: usize,
    pub duration_us: u64,
}

/// Estado mutable del sink, detrás del `Mutex`.
#[derive(Debug)]
struct Sink {
    /// `None` mientras se rota (Windows no deja renombrar un archivo con
    /// un handle abierto) o si el reopen falló.
    file: Option<File>,
    /// Bytes escritos al archivo actual. Se siembra con el tamaño real
    /// al abrir, para que un reinicio no reinicie el conteo de rotación.
    size: u64,
}

/// Sink JSONL append-only con rotación por tamaño.
#[derive(Debug)]
pub struct DbLogger {
    path: PathBuf,
    level: LogLevel,
    /// `0` desactiva la rotación (el archivo crece sin techo).
    max_bytes: u64,
    max_files: usize,
    sink: Mutex<Sink>,
}

impl DbLogger {
    /// Abre (o crea) el archivo de log en modo append.
    ///
    /// Este es el **único** error del subsistema que aborta: si la ruta
    /// no se puede abrir al arrancar, es config rota y el operador
    /// necesita enterarse ya. Una vez abierto, todo fallo posterior de
    /// escritura es best-effort (ver [`DbLogger::log`]).
    pub fn open(
        path: impl Into<PathBuf>,
        level: LogLevel,
        max_bytes: u64,
        max_files: usize,
    ) -> DbResult<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    coded(
                        codes::LOG_OPEN_FAILED,
                        format!(
                            "no se pudo crear el directorio del log '{}': {e}",
                            parent.display()
                        ),
                    )
                })?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| {
                coded(
                    codes::LOG_OPEN_FAILED,
                    format!("no se pudo abrir el log '{}': {e}", path.display()),
                )
            })?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path,
            level,
            max_bytes,
            max_files,
            sink: Mutex::new(Sink {
                file: Some(file),
                size,
            }),
        })
    }

    pub fn level(&self) -> LogLevel {
        self.level
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Filtro barato que el motor consulta **antes** de armar el
    /// `LogRecord`. Evita construir el registro para sentencias que el
    /// nivel va a descartar igual.
    pub fn admits(&self, mutating: bool, ok: bool) -> bool {
        self.level.admits(mutating, ok)
    }

    /// Anexa una entrada. Best-effort por diseño: si el filesystem
    /// falla, el error va a stderr y la sentencia que se estaba
    /// logueando sigue su curso normal.
    ///
    /// La alternativa (propagar el error al caller) implicaría que un
    /// disco lleno tumba escrituras que el motor ya aplicó — peor
    /// operacionalmente que perder una línea de log. Mismo criterio que
    /// ADR-0012.
    pub fn log(&self, record: &LogRecord<'_>) {
        if !self.level.admits(record.mutating, record.ok) {
            return;
        }
        let line = render_entry(record);
        if let Err(e) = self.append(&line) {
            eprintln!(
                "gabysql: append al log '{}' falló: {e}",
                self.path.display()
            );
        }
    }

    fn append(&self, line: &str) -> std::io::Result<()> {
        let mut sink = match self.sink.lock() {
            Ok(guard) => guard,
            // Mutex envenenado: otro thread paniqueó con el lock tomado.
            // El log no es razón para tumbar el proceso, así que
            // recuperamos el guard y seguimos.
            Err(poisoned) => poisoned.into_inner(),
        };
        let bytes = line.len() as u64;
        if self.max_bytes > 0 && sink.size > 0 && sink.size + bytes > self.max_bytes {
            self.rotate(&mut sink)?;
        }
        // Si el reopen de una rotación previa falló, reintentamos acá.
        if sink.file.is_none() {
            sink.file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
            sink.size = 0;
        }
        if let Some(file) = sink.file.as_mut() {
            file.write_all(line.as_bytes())?;
            sink.size += bytes;
        }
        Ok(())
    }

    /// `foo.log.2` → `foo.log.3`, `foo.log.1` → `foo.log.2`,
    /// `foo.log` → `foo.log.1`, y reabre `foo.log` vacío. El más viejo
    /// se descarta.
    ///
    /// El handle se cierra **antes** de renombrar: Windows rechaza el
    /// rename de un archivo con handles abiertos.
    fn rotate(&self, sink: &mut Sink) -> std::io::Result<()> {
        sink.file = None;
        if self.max_files == 0 {
            // Sin archivos históricos: se trunca y se sigue.
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)?;
            sink.file = Some(file);
            sink.size = 0;
            return Ok(());
        }
        let rotated = |n: usize| -> PathBuf {
            let mut s = self.path.clone().into_os_string();
            s.push(format!(".{n}"));
            PathBuf::from(s)
        };
        let oldest = rotated(self.max_files);
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }
        for n in (1..self.max_files).rev() {
            let from = rotated(n);
            if from.exists() {
                fs::rename(&from, rotated(n + 1))?;
            }
        }
        fs::rename(&self.path, rotated(1))?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        sink.file = Some(file);
        sink.size = 0;
        Ok(())
    }
}

/// Serializa una entrada a una línea JSONL (incluye el `\n` final).
fn render_entry(r: &LogRecord<'_>) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut out = String::with_capacity(160);
    out.push_str(&format!(
        "{{\"v\":{},\"ts_unix\":{},\"kind\":{},\"mutating\":{},\"stmt_index\":{},\
         \"ok\":{},\"rows\":{},\"duration_us\":{}",
        LOG_SCHEMA_VERSION,
        ts,
        json_escape(r.kind),
        r.mutating,
        r.stmt_index,
        r.ok,
        r.rows,
        r.duration_us
    ));
    if let Some(sql) = r.sql {
        out.push_str(&format!(",\"sql\":{}", json_escape(sql)));
    }
    if let Some(err) = r.error {
        if let Some(code) = extract_code(err) {
            out.push_str(&format!(",\"code\":{code}"));
        }
        out.push_str(&format!(",\"error\":{}", json_escape(err)));
    }
    out.push_str("}\n");
    out
}

/// Extrae el `NNNN` de un mensaje que arranca con `[GBY-NNNN] `.
/// Devuelve `None` para errores sin código (los que todavía usan
/// `DbError::new` pelado).
pub fn extract_code(message: &str) -> Option<u32> {
    let rest = message.strip_prefix("[GBY-")?;
    let (digits, _) = rest.split_once(']')?;
    digits.parse::<u32>().ok()
}

/// Escapa un `&str` como string JSON, con las comillas incluidas.
///
/// Canónico para el proyecto: `server.rs` delega acá en vez de mantener
/// su propia copia.
pub fn json_escape(value: &str) -> String {
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

/// Helper para binarios: resuelve la config del log desde flags ya
/// parseados con fallback a env (`GABYSQL_LOG_FILE`, `GABYSQL_LOG_LEVEL`).
/// Devuelve `None` si no hay log configurado por ninguna vía.
pub fn from_env_or_flags(
    file_flag: Option<PathBuf>,
    level_flag: Option<String>,
) -> DbResult<Option<DbLogger>> {
    let path = file_flag.or_else(|| {
        std::env::var("GABYSQL_LOG_FILE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
    });
    let Some(path) = path else {
        return Ok(None);
    };
    let level_raw = level_flag
        .or_else(|| {
            std::env::var("GABYSQL_LOG_LEVEL")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "error".to_string());
    let level = LogLevel::parse(&level_raw)?;
    if level == LogLevel::None {
        return Ok(None);
    }
    let max_bytes = env_u64("GABYSQL_LOG_MAX_BYTES").unwrap_or(DEFAULT_MAX_BYTES);
    let max_files = env_u64("GABYSQL_LOG_MAX_FILES").map_or(DEFAULT_MAX_FILES, |v| v as usize);
    Ok(Some(DbLogger::open(path, level, max_bytes, max_files)?))
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("gabysql-dblog-test-{name}"));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn read(path: &Path) -> String {
        let mut s = String::new();
        File::open(path).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    fn rec<'a>(
        kind: &'static str,
        mutating: bool,
        ok: bool,
        err: Option<&'a str>,
    ) -> LogRecord<'a> {
        LogRecord {
            kind,
            mutating,
            sql: Some("SELECT 1"),
            stmt_index: 0,
            ok,
            error: err,
            rows: 0,
            duration_us: 7,
        }
    }

    #[test]
    fn level_parse_accepts_the_four_names_case_insensitive() {
        assert_eq!(LogLevel::parse("NONE").unwrap(), LogLevel::None);
        assert_eq!(LogLevel::parse(" error ").unwrap(), LogLevel::Error);
        assert_eq!(LogLevel::parse("Mod").unwrap(), LogLevel::Mod);
        assert_eq!(LogLevel::parse("all").unwrap(), LogLevel::All);
    }

    #[test]
    fn level_parse_rejects_unknown_with_coded_error() {
        let err = LogLevel::parse("verbose").unwrap_err();
        assert!(err.to_string().starts_with("[GBY-6002]"), "{err}");
    }

    #[test]
    fn level_admits_matches_the_documented_matrix() {
        // (level, mutating, ok) -> admits
        assert!(!LogLevel::None.admits(true, false));
        assert!(LogLevel::Error.admits(false, false));
        assert!(!LogLevel::Error.admits(true, true));
        assert!(LogLevel::Mod.admits(true, true));
        assert!(!LogLevel::Mod.admits(false, true));
        assert!(LogLevel::Mod.admits(false, false));
        assert!(LogLevel::All.admits(false, true));
    }

    #[test]
    fn error_level_writes_only_failures() {
        let dir = tmp("error-level");
        let path = dir.join("gabysql.log");
        let logger = DbLogger::open(&path, LogLevel::Error, 0, 3).unwrap();
        logger.log(&rec("SELECT", false, true, None));
        logger.log(&rec("INSERT", true, false, Some("[GBY-3001] pk duplicada")));
        let body = read(&path);
        assert_eq!(body.lines().count(), 1, "body={body}");
        assert!(body.contains("\"code\":3001"), "{body}");
        assert!(body.contains("\"ok\":false"), "{body}");
        // La única línea es la del fallo: el SELECT exitoso no entró.
        // (Se chequea por `kind`, no por el texto SQL — el helper `rec`
        // usa el mismo `sql` para todos los registros.)
        assert!(body.contains("\"kind\":\"INSERT\""), "{body}");
        assert!(!body.contains("\"kind\":\"SELECT\""), "{body}");
    }

    #[test]
    fn mod_level_writes_mutations_and_errors_but_not_reads() {
        let dir = tmp("mod-level");
        let path = dir.join("gabysql.log");
        let logger = DbLogger::open(&path, LogLevel::Mod, 0, 3).unwrap();
        logger.log(&rec("SELECT", false, true, None));
        logger.log(&rec("INSERT", true, true, None));
        logger.log(&rec(
            "SELECT",
            false,
            false,
            Some("[GBY-2001] tabla no existe"),
        ));
        let body = read(&path);
        assert_eq!(body.lines().count(), 2, "body={body}");
    }

    #[test]
    fn every_line_is_standalone_valid_jsonl() {
        let dir = tmp("jsonl");
        let path = dir.join("gabysql.log");
        let logger = DbLogger::open(&path, LogLevel::All, 0, 3).unwrap();
        for _ in 0..5 {
            logger.log(&rec("SELECT", false, true, None));
        }
        let body = read(&path);
        assert_eq!(body.lines().count(), 5);
        for line in body.lines() {
            assert!(
                line.starts_with('{') && line.ends_with('}'),
                "línea: {line}"
            );
            assert!(
                line.contains(&format!("\"v\":{LOG_SCHEMA_VERSION}")),
                "{line}"
            );
        }
    }

    #[test]
    fn sql_with_quotes_and_newlines_stays_on_one_line() {
        let dir = tmp("escape");
        let path = dir.join("gabysql.log");
        let logger = DbLogger::open(&path, LogLevel::All, 0, 3).unwrap();
        let mut r = rec("INSERT", true, true, None);
        let sql = "INSERT INTO t VALUES ('a\"b',\n'c\\d')";
        r.sql = Some(sql);
        logger.log(&r);
        let body = read(&path);
        assert_eq!(body.lines().count(), 1, "body={body}");
        assert!(body.contains("\\n"), "{body}");
        assert!(body.contains("\\\""), "{body}");
    }

    #[test]
    fn rotation_caps_the_file_and_keeps_history() {
        let dir = tmp("rotation");
        let path = dir.join("gabysql.log");
        // max_bytes chico para forzar varias rotaciones con pocas líneas.
        let logger = DbLogger::open(&path, LogLevel::All, 300, 2).unwrap();
        for _ in 0..40 {
            logger.log(&rec("SELECT", false, true, None));
        }
        assert!(path.exists());
        assert!(fs::metadata(&path).unwrap().len() <= 300 + 200);
        let r1 = dir.join("gabysql.log.1");
        let r2 = dir.join("gabysql.log.2");
        assert!(r1.exists(), "falta el rotado .1");
        assert!(r2.exists(), "falta el rotado .2");
        // max_files=2 → nunca aparece un .3.
        assert!(!dir.join("gabysql.log.3").exists());
    }

    #[test]
    fn reopen_seeds_size_from_disk_so_rotation_survives_restart() {
        let dir = tmp("reopen");
        let path = dir.join("gabysql.log");
        {
            let logger = DbLogger::open(&path, LogLevel::All, 0, 3).unwrap();
            for _ in 0..10 {
                logger.log(&rec("SELECT", false, true, None));
            }
        }
        let first_len = fs::metadata(&path).unwrap().len();
        assert!(first_len > 0);
        // Reabrir con un cap por debajo de lo ya escrito: la primera
        // escritura debe rotar, no seguir creciendo.
        let logger = DbLogger::open(&path, LogLevel::All, first_len, 3).unwrap();
        logger.log(&rec("SELECT", false, true, None));
        assert!(dir.join("gabysql.log.1").exists());
        assert!(fs::metadata(&path).unwrap().len() < first_len);
    }

    #[test]
    fn open_creates_missing_parent_directories() {
        let dir = tmp("mkdir");
        let path = dir.join("nested").join("deep").join("gabysql.log");
        let logger = DbLogger::open(&path, LogLevel::All, 0, 3).unwrap();
        logger.log(&rec("SELECT", false, true, None));
        assert!(path.exists());
    }

    #[test]
    fn extract_code_reads_the_gby_prefix_only() {
        assert_eq!(extract_code("[GBY-3001] pk duplicada"), Some(3001));
        assert_eq!(extract_code("[GBY-0042] ejemplo"), Some(42));
        assert_eq!(extract_code("error sin código"), None);
        assert_eq!(extract_code("[GBY-abcd] roto"), None);
    }

    #[test]
    fn json_escape_handles_control_chars() {
        assert_eq!(json_escape("a\u{1}b"), "\"a\\u0001b\"");
        assert_eq!(json_escape("tab\there"), "\"tab\\there\"");
    }
}
