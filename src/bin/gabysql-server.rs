use gabysql::server::{run, ServerConfig};
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

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-db" => db = args.next().map(PathBuf::from),
            "-dir" => dir = args.next().map(PathBuf::from),
            "-addr" => addr = args.next().unwrap_or_else(|| ":8080".to_string()),
            "-token" => token = args.next(),
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
        },
    )
}

fn usage() {
    println!(
        "Uso:\n  gabysql-server -db demo.db -addr :8080\n  gabysql-server -dir ./dbs -addr :8080\n  gabysql-server -db demo.db -token secret"
    );
}
