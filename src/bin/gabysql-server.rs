use gabysql::server::{run, ServerConfig, DEFAULT_MAX_CONNECTIONS};
use gabysql::DbResult;
use std::env;
use std::path::PathBuf;

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

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-db" => db = args.next().map(PathBuf::from),
            "-dir" => dir = args.next().map(PathBuf::from),
            "-addr" => addr = args.next().unwrap_or_else(|| ":8080".to_string()),
            "-token" => token = args.next(),
            "-max-connections" => {
                let raw = args
                    .next()
                    .ok_or_else(|| gabysql::DbError::new("-max-connections requires a value"))?;
                max_connections = raw.parse::<usize>().map_err(|err| {
                    gabysql::DbError::new(format!("invalid -max-connections: {}", err))
                })?;
                if max_connections == 0 {
                    return Err(gabysql::DbError::new("-max-connections must be > 0"));
                }
            }
            "-log-json" => log_json = true,
            "-h" | "--help" => {
                usage();
                return Ok(());
            }
            _ => return Err(gabysql::DbError::new(format!("flag no soportada: {}", arg))),
        }
    }

    run(
        &addr,
        ServerConfig {
            single_db: db,
            dir,
            token,
            max_connections,
            log_json,
        },
    )
}

fn usage() {
    println!(
        "Uso:\n  gabysql-server -db demo.db -addr :8080\n  gabysql-server -dir ./dbs -addr :8080\n  gabysql-server -db demo.db -token secret\n  gabysql-server -dir ./dbs -max-connections 32\n  gabysql-server -db demo.db -log-json   (one JSON line per request on stdout)"
    );
}
