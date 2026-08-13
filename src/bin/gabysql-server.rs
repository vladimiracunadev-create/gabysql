use gabysql::dblog;
use gabysql::server::{run, ServerConfig, DEFAULT_MAX_CONNECTIONS};
use gabysql::DbResult;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    if let Err(err) = real_main() {
        eprintln!("error: {}", err);
        std::process::exit(1);
    }
}

fn real_main() -> DbResult<()> {
    let mut db = None;
    let mut dir = None;
    let mut addr = ":8080".to_string();
    let mut token = None;
    let mut max_connections = DEFAULT_MAX_CONNECTIONS;
    let mut log_json = false;
    let mut log_file: Option<PathBuf> = None;
    let mut log_level: Option<String> = None;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-db" => db = args.next().map(PathBuf::from),
            "-dir" => dir = args.next().map(PathBuf::from),
            "-addr" => addr = args.next().unwrap_or_else(|| ":8080".to_string()),
            "-token" => token = args.next(),
            "-max-connections" => {
                let raw = args.next().ok_or_else(|| {
                    gabysql::DbError::new(
                        "-max-connections requiere un valor (ej: -max-connections 32)",
                    )
                })?;
                max_connections = raw.parse::<usize>().map_err(|err| {
                    gabysql::DbError::new(format!(
                        "-max-connections inválido: no es un entero ({}); recibí '{}'",
                        err, raw
                    ))
                })?;
                if max_connections == 0 {
                    return Err(gabysql::DbError::new(
                        "-max-connections debe ser > 0; recibí 0",
                    ));
                }
            }
            "-log-json" => log_json = true,
            "-log-file" => {
                log_file = Some(args.next().map(PathBuf::from).ok_or_else(|| {
                    gabysql::DbError::new("-log-file requiere una ruta (ej: -log-file gabysql.log)")
                })?);
            }
            "-log-level" => {
                log_level = Some(args.next().ok_or_else(|| {
                    gabysql::DbError::new(
                        "-log-level requiere un valor (none|error|mod|all); ej: -log-level mod",
                    )
                })?);
            }
            "-h" | "--help" => {
                usage();
                return Ok(());
            }
            _ => return Err(gabysql::DbError::new(format!("flag no soportada: {}", arg))),
        }
    }

    // Bloque L: log de sentencias del motor. Sin -log-file ni
    // GABYSQL_LOG_FILE queda en None y el motor no escribe nada.
    let logger = dblog::from_env_or_flags(log_file, log_level)?.map(Arc::new);
    if let Some(l) = logger.as_ref() {
        eprintln!(
            "gabysql-server: log de sentencias en {} (level={})",
            l.path().display(),
            l.level().as_str()
        );
    }

    run(
        &addr,
        ServerConfig {
            single_db: db,
            dir,
            token,
            max_connections,
            log_json,
            logger,
        },
    )
}

fn usage() {
    println!(
        "Uso:\n  \
         gabysql-server -db demo.db -addr :8080\n  \
         gabysql-server -dir ./dbs -addr :8080\n  \
         gabysql-server -db demo.db -token secret\n  \
         gabysql-server -dir ./dbs -max-connections 32\n  \
         gabysql-server -db demo.db -log-json   (una línea JSON por request en stdout)\n  \
         gabysql-server -db demo.db -log-file gabysql.log -log-level mod\n\
         \n\
         Log de sentencias del motor (ADR-0094):\n  \
         -log-file P      Archivo JSONL append-only con rotación por tamaño.\n  \
         -log-level L     none | error (default) | mod | all.\n\
         \n  \
         Env equivalentes: GABYSQL_LOG_FILE, GABYSQL_LOG_LEVEL,\n  \
         GABYSQL_LOG_MAX_BYTES (default 8 MiB), GABYSQL_LOG_MAX_FILES (default 3)."
    );
}
