use gabysql::sql::{parse, Engine, ResultSet, Value};
use gabysql::storage::Pager;
use gabysql::DbResult;
use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

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
        _ => {
            usage();
            std::process::exit(2);
        }
    }

    Ok(())
}

fn run_exec(db_path: PathBuf, query: &str) -> DbResult<()> {
    let mut pager = Pager::open(db_path)?;
    pager.begin()?;
    let response = (|| -> DbResult<Vec<ResultSet>> {
        let statements = parse(query)?;
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
    }
}

fn usage() {
    println!(
        "Uso:\n  gabysql init [--force] <file.db>\n  gabysql info <file.db>\n  gabysql exec <file.db> \"<SQL...>\"\n  gabysql repl <file.db>\n\n  init refuses to overwrite an existing file; pass --force to replace it.\n\nSQL soportado:\n  CREATE TABLE users (id INT PRIMARY KEY, name TEXT, active BOOL);\n  INSERT INTO users (id,name,active) VALUES (1,'Ana',TRUE);\n  SELECT * FROM users;\n  SELECT id,name FROM users WHERE id BETWEEN 1 AND 10 LIMIT 5 OFFSET 0;"
    );
}
