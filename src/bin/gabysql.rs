use gabysql::sql::{parse, Engine, ResultSet, Statement, Value};
use gabysql::storage::Pager;
use gabysql::{DbError, DbResult};
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {}", err);
        std::process::exit(1);
    }
}

fn run() -> DbResult<()> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() < 2 {
        usage();
        std::process::exit(2);
    }

    match args[1].as_str() {
        "init" => {
            let (force, file) = match args.len() {
                3 => (false, &args[2]),
                4 if args[2] == "--force" => (true, &args[3]),
                _ => {
                    usage();
                    std::process::exit(2);
                }
            };
            let mut pager = if force {
                Pager::create_force(file)?
            } else {
                Pager::create(file)?
            };
            pager.close()?;
            println!("OK");
        }
        "info" => {
            if args.len() != 3 {
                usage();
                std::process::exit(2);
            }
            let mut pager = Pager::open(&args[2])?;
            let header = pager.header();
            println!(
                "pageSize={}  pageCount={}  catalogRoot={}",
                header.page_size, header.page_count, header.catalog_root_page
            );
            pager.close()?;
        }
        "exec" => {
            if args.len() < 4 {
                usage();
                std::process::exit(2);
            }
            let db = &args[2];
            let query = args[3..].join(" ");
            run_exec(PathBuf::from(db), &query)?;
        }
        "repl" => {
            if args.len() != 3 {
                usage();
                std::process::exit(2);
            }
            run_repl(PathBuf::from(&args[2]))?;
        }
        "backup" => run_backup_cmd(&args[2..], false)?,
        "restore" => run_backup_cmd(&args[2..], true)?,
        "verify" => {
            if args.len() != 3 {
                usage();
                std::process::exit(2);
            }
            let report = gabysql::backup::verify(Path::new(&args[2]))?;
            println!(
                "OK verify  path={}  pages={}  bytes={}",
                report.path.display(),
                report.pages,
                report.bytes
            );
        }
        _ => {
            usage();
            std::process::exit(2);
        }
    }

    Ok(())
}

/// Shared driver for `backup` and `restore` (both produce a verified
/// copy of `src` at `dst`). Accepts an optional leading `--force` to
/// allow overwriting an existing `dst`.
fn run_backup_cmd(rest: &[String], is_restore: bool) -> DbResult<()> {
    let mut force = false;
    let mut positional: Vec<&str> = Vec::new();
    for arg in rest {
        if arg == "--force" {
            force = true;
        } else {
            positional.push(arg);
        }
    }
    if positional.len() != 2 {
        usage();
        std::process::exit(2);
    }
    let src = Path::new(positional[0]);
    let dst = Path::new(positional[1]);
    let report = if is_restore {
        gabysql::backup::restore(src, dst, force)?
    } else {
        gabysql::backup::backup(src, dst, force)?
    };
    println!(
        "OK {}  src={}  dst={}  pages={}  bytes={}",
        if is_restore { "restore" } else { "backup" },
        report.src.display(),
        report.dst.display(),
        report.pages,
        report.bytes
    );
    Ok(())
}

fn run_exec(db_path: PathBuf, query: &str) -> DbResult<()> {
    let statements = parse(query)?;

    // CLI mode treats the directory containing db_path as the "multi-DB
    // directory" so CREATE/DROP/SHOW DATABASE work the same way they do
    // through the HTTP server.
    if statements.iter().any(is_db_level_stmt) {
        if statements.iter().any(|s| !is_db_level_stmt(s)) {
            return Err(DbError::new(
                "no se admite mezclar CREATE/DROP/SHOW DATABASE con sentencias de tabla",
            ));
        }
        let dir = db_path.parent().unwrap_or_else(|| Path::new("."));
        return run_db_level_cli(dir, statements);
    }

    let mut pager = Pager::open(db_path)?;
    pager.begin()?;
    let response = (|| -> DbResult<Vec<ResultSet>> {
        let mut engine = Engine::new(&mut pager);
        let mut results = Vec::new();
        for statement in statements {
            results.push(engine.exec(statement)?);
        }
        pager.commit()?;
        Ok(results)
    })();

    match response {
        Ok(results) => {
            for result in results {
                print_result(&result);
            }
            Ok(())
        }
        Err(err) => {
            let _ = pager.rollback();
            Err(err)
        }
    }
}

fn is_db_level_stmt(s: &Statement) -> bool {
    matches!(
        s,
        Statement::CreateDatabase(_) | Statement::DropDatabase(_) | Statement::ShowDatabases
    )
}

fn run_db_level_cli(dir: &Path, stmts: Vec<Statement>) -> DbResult<()> {
    fs::create_dir_all(dir)?;
    for stmt in stmts {
        match stmt {
            Statement::CreateDatabase(s) => {
                let path = dir.join(format!("{}.db", normalize_db_ident(&s.name)?));
                if path.exists() {
                    if s.if_not_exists {
                        println!("OK (ya existía: {})", path.display());
                        continue;
                    }
                    return Err(DbError::new(format!(
                        "base de datos '{}' ya existe",
                        path.display()
                    )));
                }
                let mut pager = Pager::create(&path)?;
                pager.close()?;
                println!("OK ({})", path.display());
            }
            Statement::DropDatabase(s) => {
                let path = dir.join(format!("{}.db", normalize_db_ident(&s.name)?));
                let mut wal = path.clone().into_os_string();
                wal.push(".wal");
                let wal = PathBuf::from(wal);
                if !path.exists() {
                    if s.if_exists {
                        println!("OK (no existía: {})", path.display());
                        continue;
                    }
                    return Err(DbError::new(format!(
                        "base de datos '{}' no existe",
                        path.display()
                    )));
                }
                fs::remove_file(&path)?;
                let _ = fs::remove_file(&wal);
                println!("OK ({} eliminada)", path.display());
            }
            Statement::ShowDatabases => {
                let mut entries: Vec<String> = fs::read_dir(dir)?
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        name.strip_suffix(".db").map(|s| s.to_string())
                    })
                    .collect();
                entries.sort();
                println!("database");
                for name in entries {
                    println!("{}", name);
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn normalize_db_ident(value: &str) -> DbResult<String> {
    let trimmed = value.trim();
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        || trimmed.is_empty()
    {
        return Err(DbError::new(
            "nombre de DB inválido: solo [A-Za-z0-9_-], no vacío",
        ));
    }
    Ok(trimmed.to_string())
}

fn run_repl(db_path: PathBuf) -> DbResult<()> {
    println!("gabysql repl. termina sentencias con ';'. Ctrl+C para salir.");
    let stdin = io::stdin();
    let mut buffer = String::new();
    loop {
        print!("> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        buffer.push_str(&line);
        if line.contains(';') {
            run_exec(db_path.clone(), &buffer)?;
            buffer.clear();
        }
    }
    Ok(())
}

fn print_result(result: &ResultSet) {
    if let Some(message) = &result.message {
        if result.columns.is_empty() {
            println!("{}", message);
            return;
        }
    }
    if !result.columns.is_empty() {
        println!("{}", result.columns.join("\t"));
    }
    for row in &result.rows {
        let parts = row.iter().map(value_to_string).collect::<Vec<_>>();
        println!("{}", parts.join("\t"));
    }
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Integer(number) => number.to_string(),
        Value::Float(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::String(text) => text.clone(),
        // Bloque Y4: BLOB se renderiza como hex con prefijo 0x.
        Value::Bytes(b) => {
            let mut s = String::with_capacity(2 + b.len() * 2);
            s.push_str("0x");
            for byte in b {
                s.push_str(&format!("{:02x}", byte));
            }
            s
        }
    }
}

fn usage() {
    println!(
        "Uso:\n  gabysql init [--force] <file.db>\n  gabysql info <file.db>\n  gabysql exec <file.db> \"<SQL...>\"\n  gabysql repl <file.db>\n  gabysql backup [--force] <src.db> <dst.db>\n  gabysql restore [--force] <src.db> <dst.db>\n  gabysql verify <file.db>\n\n  init refuses to overwrite an existing file; pass --force to replace it.\n  backup/restore verify every CRC32 page trailer on read and re-open the destination to confirm end-to-end integrity.\n  verify walks every page of a closed DB and reports the page count.\n\nSQL soportado:\n  CREATE TABLE users (id INT PRIMARY KEY, name TEXT, active BOOL);\n  INSERT INTO users (id,name,active) VALUES (1,'Ana',TRUE);\n  SELECT * FROM users;\n  SELECT id,name FROM users WHERE id BETWEEN 1 AND 10 LIMIT 5 OFFSET 0;"
    );
}
