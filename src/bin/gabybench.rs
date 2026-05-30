//! gabybench — benchmark profesional one-shot del motor gabysql.
//!
//! NO es el producto `gabybench` (spec en `docs/GABYBENCH_SPEC.md`).
//! Es una sesión de medición reproducible que popula 3 DBs sintéticas y mide
//! latencias de queries representativas. Pensado para correr en release y
//! producir números crudos en `bench/results.json` + reporte humano en stdout.
//!
//! Uso:
//!     cargo run --release --bin gabybench
//!     cargo run --release --bin gabybench -- setup     # solo crea DBs
//!     cargo run --release --bin gabybench -- run       # solo corre queries
//!     cargo run --release --bin gabybench -- all       # default

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gabysql::sql::{parse, Engine, Statement};
use gabysql::storage::Pager;
use gabysql::DbResult;

// ---------------------------------------------------------------------------
// Configuración global
// ---------------------------------------------------------------------------

const BENCH_DIR: &str = "bench";
const DBS_DIR: &str = "bench/dbs";
const RESULTS_JSON: &str = "bench/results.json";

const MICROBLOG_USERS: usize = 10_000;
const MICROBLOG_POSTS: usize = 40_000;
const EVENTS_ROWS: usize = 200_000;
const ORDERS_ROWS: usize = 20_000;
const LINES_ROWS: usize = 100_000;

const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

// ---------------------------------------------------------------------------
// PRNG determinístico — LCG (Numerical Recipes)
// ---------------------------------------------------------------------------

struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo)
    }
}

// ---------------------------------------------------------------------------
// Helpers de medición
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct BenchRow {
    suite: String,
    name: String,
    iters: usize,
    p50_ns: u128,
    p95_ns: u128,
    p99_ns: u128,
    mean_ns: u128,
    stddev_ns: u128,
    total_ms: f64,
    rows_returned: usize,
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn summarize(suite: &str, name: &str, samples: Vec<u128>, rows: usize) -> BenchRow {
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let n = sorted.len();
    let total: u128 = sorted.iter().sum();
    let mean = if n > 0 { total / n as u128 } else { 0 };
    let var: f64 = sorted
        .iter()
        .map(|&v| {
            let d = v as f64 - mean as f64;
            d * d
        })
        .sum::<f64>()
        / n.max(1) as f64;
    let stddev = var.sqrt() as u128;
    BenchRow {
        suite: suite.to_string(),
        name: name.to_string(),
        iters: n,
        p50_ns: percentile(&sorted, 0.50),
        p95_ns: percentile(&sorted, 0.95),
        p99_ns: percentile(&sorted, 0.99),
        mean_ns: mean,
        stddev_ns: stddev,
        total_ms: total as f64 / 1_000_000.0,
        rows_returned: rows,
    }
}

fn fmt_ns(ns: u128) -> String {
    if ns < 10_000 {
        format!("{} ns", ns)
    } else if ns < 10_000_000 {
        format!("{:.2} µs", ns as f64 / 1_000.0)
    } else {
        format!("{:.2} ms", ns as f64 / 1_000_000.0)
    }
}

fn print_header() {
    println!(
        "{:<42} {:>8} {:>12} {:>12} {:>12} {:>12} {:>10} {:>8}",
        "query", "N", "p50", "p95", "p99", "mean", "total(ms)", "rows"
    );
    println!("{}", "-".repeat(120));
}

fn print_row(r: &BenchRow) {
    println!(
        "{:<42} {:>8} {:>12} {:>12} {:>12} {:>12} {:>10.2} {:>8}",
        truncate(&r.name, 42),
        r.iters,
        fmt_ns(r.p50_ns),
        fmt_ns(r.p95_ns),
        fmt_ns(r.p99_ns),
        fmt_ns(r.mean_ns),
        r.total_ms,
        r.rows_returned,
    );
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() > n {
        format!("{}…", &s[..n - 1])
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Wrappers exec — cada uno abre/usa una tx ya activa
// ---------------------------------------------------------------------------

/// Ejecuta una sentencia SQL ya parseada y devuelve filas retornadas.
fn exec_stmt(engine: &mut Engine, stmt: Statement) -> DbResult<usize> {
    let rs = engine.exec(stmt)?;
    Ok(rs.rows.len())
}

/// Parsea + ejecuta. Útil cuando el SQL cambia entre iteraciones.
fn exec_sql(engine: &mut Engine, sql: &str) -> DbResult<usize> {
    let stmts = parse(sql)?;
    let mut total = 0;
    for s in stmts {
        total += exec_stmt(engine, s)?;
    }
    Ok(total)
}

/// Mide N iteraciones de una closure que ejecuta la query.
/// La closure recibe el iter index para queries con parámetro variable.
fn bench<F>(
    suite: &str,
    name: &str,
    pager: &mut Pager,
    iters: usize,
    mut f: F,
) -> DbResult<BenchRow>
where
    F: FnMut(&mut Engine, usize) -> DbResult<usize>,
{
    // Warmup. Para que la suite DML (INSERT con id = i + K) no colisione
    // con el main loop, usamos un offset gigante (1_000_000) en el
    // índice de warmup. Las queries SELECT ignoran `i` así que no las
    // afecta; las DML inserts caen en un rango disjunto al de iters.
    let warmup = (iters / 10).clamp(5, 50);
    {
        let mut engine = Engine::new(pager);
        for i in 0..warmup {
            let _ = f(&mut engine, i + 1_000_000)?;
        }
    }

    let mut samples = Vec::with_capacity(iters);
    let mut last_rows = 0;
    {
        let mut engine = Engine::new(pager);
        for i in 0..iters {
            let t0 = Instant::now();
            last_rows = f(&mut engine, i)?;
            samples.push(t0.elapsed().as_nanos());
        }
    }

    let row = summarize(suite, name, samples, last_rows);
    print_row(&row);
    Ok(row)
}

/// Helper para queries con SQL fijo (string).
fn bench_sql(
    suite: &str,
    name: &str,
    pager: &mut Pager,
    iters: usize,
    sql: &'static str,
) -> DbResult<BenchRow> {
    // Pre-parse una vez fuera del hot loop — para no medir el parser.
    // Igual lo re-parseamos por iter porque Statement no es Clone en general;
    // si lo fuera podríamos clonar. En la práctica el parse de un SELECT
    // chico es <5µs y se mide *junto* a la ejecución, lo cual es honesto:
    // el usuario final paga ambos costos.
    bench(suite, name, pager, iters, |engine, _| exec_sql(engine, sql))
}

/// Variante que NO aborta el bench si la query falla (hueco conocido del
/// motor — e.g. agregados sobre JOIN, [GBY-4028]). En vez de cortar
/// abruptamente la suite, deja una BenchRow marcada con `total_ms=-1` y
/// el código de error en `rows_returned` (-1 también) y sigue.
fn bench_sql_or_skip(
    suite: &str,
    name: &str,
    pager: &mut Pager,
    iters: usize,
    sql: &'static str,
) -> BenchRow {
    match bench_sql(suite, name, pager, iters, sql) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("   ⚠ SKIP '{}': {}", name, e);
            BenchRow {
                suite: suite.to_string(),
                name: format!("{} [SKIPPED: motor no soporta]", name),
                iters: 0,
                p50_ns: 0,
                p95_ns: 0,
                p99_ns: 0,
                mean_ns: 0,
                stddev_ns: 0,
                total_ms: -1.0,
                rows_returned: 0,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Setup de las 3 DBs
// ---------------------------------------------------------------------------

fn rm_if_exists(p: &Path) {
    let _ = fs::remove_file(p);
    let mut wal = p.as_os_str().to_owned();
    wal.push(".wal");
    let _ = fs::remove_file(PathBuf::from(wal));
}

fn db_size_bytes(p: &Path) -> u64 {
    fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Ejecuta varias sentencias en una sola transacción.
fn exec_batch(pager: &mut Pager, sqls: &[&str]) -> DbResult<()> {
    pager.begin()?;
    let res = (|| -> DbResult<()> {
        let mut engine = Engine::new(pager);
        for sql in sqls {
            for stmt in parse(sql)? {
                engine.exec(stmt)?;
            }
        }
        Ok(())
    })();
    match res {
        Ok(()) => pager.commit()?,
        Err(e) => {
            let _ = pager.rollback();
            return Err(e);
        }
    }
    Ok(())
}

/// Carga masiva mediante batches de N INSERTs por commit, para no hacer
/// crecer el WAL desmesuradamente.
fn bulk_load<F>(pager: &mut Pager, total: usize, batch: usize, mut gen: F) -> DbResult<()>
where
    F: FnMut(usize) -> String,
{
    let mut i = 0;
    while i < total {
        let end = (i + batch).min(total);
        pager.begin()?;
        let r = (|| -> DbResult<()> {
            let mut engine = Engine::new(pager);
            for j in i..end {
                let sql = gen(j);
                for stmt in parse(&sql)? {
                    engine.exec(stmt)?;
                }
            }
            Ok(())
        })();
        match r {
            Ok(()) => pager.commit()?,
            Err(e) => {
                let _ = pager.rollback();
                return Err(e);
            }
        }
        i = end;
    }
    Ok(())
}

fn escape_text(s: &str) -> String {
    s.replace('\'', "''")
}

// ---- DB-1 microblog --------------------------------------------------------

fn setup_microblog(path: &Path) -> DbResult<()> {
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;

    exec_batch(
        &mut pager,
        &[
            "CREATE TABLE users (id INT PRIMARY KEY, email TEXT NOT NULL UNIQUE, nombre TEXT NOT NULL, created_at DATETIME NOT NULL)",
            "CREATE TABLE posts (id INT PRIMARY KEY, user_id INT NOT NULL REFERENCES users(id) ON DELETE CASCADE, titulo TEXT NOT NULL, cuerpo TEXT NOT NULL, likes INT NOT NULL DEFAULT 0, created_at DATETIME NOT NULL)",
        ],
    )?;

    let mut rng = Lcg::new(SEED);
    let nombres = [
        "Ana", "Luis", "Maria", "Pedro", "Juan", "Sofia", "Diego", "Lucia",
    ];
    bulk_load(&mut pager, MICROBLOG_USERS, 500, |i| {
        let nombre = nombres[rng.next_u64() as usize % nombres.len()];
        format!(
            "INSERT INTO users (id, email, nombre, created_at) VALUES ({}, 'u{}@bench.dev', '{}', '2025-01-01 00:00:00')",
            i, i, nombre
        )
    })?;

    let mut rng = Lcg::new(SEED ^ 0xDEAD_BEEF);
    bulk_load(&mut pager, MICROBLOG_POSTS, 500, |i| {
        let user_id = rng.range(0, MICROBLOG_USERS as u64) as i64;
        let likes = rng.range(0, 1000) as i64;
        format!(
            "INSERT INTO posts (id, user_id, titulo, cuerpo, likes, created_at) VALUES ({}, {}, 'titulo {}', 'cuerpo del post numero {}', {}, '2025-02-01 12:00:00')",
            i, user_id, i, i, likes
        )
    })?;

    exec_batch(
        &mut pager,
        &[
            "CREATE INDEX idx_posts_user ON posts(user_id)",
            "CREATE INDEX idx_posts_likes ON posts(likes)",
        ],
    )?;

    pager.close()?;
    Ok(())
}

// ---- DB-2 events -----------------------------------------------------------

fn setup_events(path: &Path) -> DbResult<()> {
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;

    exec_batch(
        &mut pager,
        &["CREATE TABLE events (id INT PRIMARY KEY, kind TEXT NOT NULL, valor INT NOT NULL, ts DATETIME NOT NULL, payload TEXT NOT NULL)"],
    )?;

    // Distribución de kind: view 50%, click 25%, login 10%, logout 7%, error 4%, purchase 3%, signup 1%
    // Implementado con buckets sobre [0, 100).
    fn pick_kind(r: u64) -> &'static str {
        let p = r % 100;
        match p {
            0..=49 => "view",
            50..=74 => "click",
            75..=84 => "login",
            85..=91 => "logout",
            92..=95 => "error",
            96..=98 => "purchase",
            _ => "signup",
        }
    }

    let mut rng = Lcg::new(SEED ^ 0xEEEE_1111);
    bulk_load(&mut pager, EVENTS_ROWS, 1000, |i| {
        let r = rng.next_u64();
        let kind = pick_kind(r);
        let valor = (rng.next_u64() % 100_000) as i64;
        format!(
            "INSERT INTO events (id, kind, valor, ts, payload) VALUES ({}, '{}', {}, '2025-03-01 00:00:00', 'payload-{}')",
            i, kind, valor, i
        )
    })?;

    exec_batch(
        &mut pager,
        &["CREATE INDEX idx_events_valor ON events(valor)"],
    )?;

    pager.close()?;
    Ok(())
}

// ---- DB-3 orders_lines -----------------------------------------------------

fn setup_orders_lines(path: &Path) -> DbResult<()> {
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;

    exec_batch(
        &mut pager,
        &[
            "CREATE TABLE orders (id INT PRIMARY KEY, cliente_id INT NOT NULL, total INT NOT NULL, fecha DATETIME NOT NULL)",
            "CREATE TABLE lines (order_id INT NOT NULL, line_no INT NOT NULL, sku TEXT NOT NULL, qty INT NOT NULL, precio INT NOT NULL, PRIMARY KEY (order_id, line_no))",
        ],
    )?;

    let mut rng = Lcg::new(SEED ^ 0xABCD_1234);
    bulk_load(&mut pager, ORDERS_ROWS, 500, |i| {
        let cliente = rng.range(0, 1000) as i64;
        let total = rng.range(100, 100_000) as i64;
        format!(
            "INSERT INTO orders (id, cliente_id, total, fecha) VALUES ({}, {}, {}, '2025-04-01 09:00:00')",
            i, cliente, total
        )
    })?;

    // 5 líneas por orden: order_id = i / 5, line_no = i % 5
    let mut rng2 = Lcg::new(SEED ^ 0xCAFE_F00D);
    bulk_load(&mut pager, LINES_ROWS, 500, |i| {
        let order_id = (i / 5) as i64;
        let line_no = (i % 5) as i64;
        let qty = (rng2.next_u64() % 20 + 1) as i64;
        let precio = (rng2.next_u64() % 5000 + 10) as i64;
        let sku_n = rng2.next_u64() % 1000;
        format!(
            "INSERT INTO lines (order_id, line_no, sku, qty, precio) VALUES ({}, {}, 'SKU-{:04}', {}, {})",
            order_id, line_no, sku_n, qty, precio
        )
    })?;

    exec_batch(
        &mut pager,
        &["CREATE INDEX idx_lines_qty_precio ON lines(qty, precio)"],
    )?;

    pager.close()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Suites de queries
// ---------------------------------------------------------------------------

fn open_for_bench(path: &Path) -> DbResult<Pager> {
    // Una sola tx larga para todo el bench. No commit — los SELECTs no
    // mutan, y al final se rollback. Esto evita el costo de fsync per query.
    let mut pager = Pager::open(path)?;
    pager.begin()?;
    Ok(pager)
}

fn close_after_bench(mut pager: Pager) {
    let _ = pager.rollback();
    let _ = pager.close();
}

fn suite_microblog(path: &Path, out: &mut Vec<BenchRow>) -> DbResult<()> {
    println!("\n=== microblog ===");
    print_header();
    let mut pager = open_for_bench(path)?;

    out.push(bench_sql(
        "microblog",
        "PK lookup hot (id=5000)",
        &mut pager,
        1000,
        "SELECT * FROM users WHERE id = 5000",
    )?);

    out.push(bench(
        "microblog",
        "PK lookup cold (random id)",
        &mut pager,
        500,
        |engine, i| {
            let mut rng = Lcg::new(SEED ^ i as u64 ^ 0x1111);
            let id = (rng.next_u64() % MICROBLOG_USERS as u64) as i64;
            exec_sql(engine, &format!("SELECT * FROM users WHERE id = {}", id))
        },
    )?);

    out.push(bench_sql(
        "microblog",
        "UNIQUE TEXT lookup (email)",
        &mut pager,
        1000,
        "SELECT * FROM users WHERE email = 'u7777@bench.dev'",
    )?);

    out.push(bench_sql(
        "microblog",
        "Index secundario eq (user_id=100)",
        &mut pager,
        500,
        "SELECT * FROM posts WHERE user_id = 100",
    )?);

    out.push(bench_sql(
        "microblog",
        "Index ordered range (likes 50..100)",
        &mut pager,
        200,
        "SELECT id FROM posts WHERE likes BETWEEN 50 AND 100",
    )?);

    out.push(bench_sql(
        "microblog",
        "Full scan TEXT (nombre LIKE A%)",
        &mut pager,
        100,
        "SELECT id FROM users WHERE nombre LIKE 'A%'",
    )?);

    // [GBY-4028] vigente: agregados sobre SELECT con JOIN no se soportan.
    // Lo medimos con skip-graceful para no abortar la suite.
    out.push(bench_sql_or_skip("microblog", "JOIN+COUNT (u.id=7)", &mut pager, 200,
        "SELECT u.nombre, COUNT(*) FROM users u JOIN posts p ON p.user_id = u.id WHERE u.id = 7 GROUP BY u.nombre"));

    out.push(bench_sql(
        "microblog",
        "Aggregate global posts",
        &mut pager,
        50,
        "SELECT COUNT(*), AVG(likes) FROM posts",
    )?);

    out.push(bench_sql(
        "microblog",
        "GROUP BY user_id",
        &mut pager,
        20,
        "SELECT user_id, COUNT(*) FROM posts GROUP BY user_id",
    )?);

    close_after_bench(pager);

    // DML — bracket completo en UNA tx (la label original decía "auto-tx" pero
    // el bench no implementaba commit per-iter y caía con
    // "requiere transacción activa"). Hoy medimos cost in-tx amortizado, no
    // cost de fsync per-statement. Honesto: el flush real es de Fase 4.
    {
        let mut pager = Pager::open(path)?;
        pager.begin()?;
        {
            let mut engine = Engine::new(&mut pager);
            engine.exec(
                parse("CREATE TABLE tmp_ins (id INT PRIMARY KEY, v INT NOT NULL)")?.remove(0),
            )?;
        }
        pager.commit()?;

        pager.begin()?;

        out.push(bench(
            "microblog",
            "INSERT single (in-tx)",
            &mut pager,
            500,
            |engine, i| {
                exec_sql(
                    engine,
                    &format!(
                        "INSERT INTO tmp_ins (id, v) VALUES ({}, {})",
                        i + 100_000,
                        i
                    ),
                )
            },
        )?);

        out.push(bench(
            "microblog",
            "UPDATE por PK (in-tx)",
            &mut pager,
            500,
            |engine, i| {
                let id = (i % 500) + 100_000;
                exec_sql(
                    engine,
                    &format!("UPDATE tmp_ins SET v = {} WHERE id = {}", i, id),
                )
            },
        )?);

        out.push(bench(
            "microblog",
            "DELETE por PK (in-tx)",
            &mut pager,
            500,
            |engine, i| {
                let id = i + 100_000;
                exec_sql(engine, &format!("DELETE FROM tmp_ins WHERE id = {}", id))
            },
        )?);

        pager.commit()?;

        // cleanup
        pager.begin()?;
        {
            let mut engine = Engine::new(&mut pager);
            engine.exec(parse("DROP TABLE tmp_ins")?.remove(0))?;
        }
        pager.commit()?;
        pager.close()?;
    }

    Ok(())
}

fn suite_events(path: &Path, out: &mut Vec<BenchRow>) -> DbResult<()> {
    println!("\n=== events ===");
    print_header();
    let mut pager = open_for_bench(path)?;

    out.push(bench_sql(
        "events",
        "Full scan eq kind='view' (no idx)",
        &mut pager,
        20,
        "SELECT COUNT(*) FROM events WHERE kind = 'view'",
    )?);

    out.push(bench_sql(
        "events",
        "Indexed eq valor=12345",
        &mut pager,
        500,
        "SELECT * FROM events WHERE valor = 12345",
    )?);

    out.push(bench_sql(
        "events",
        "Indexed range valor 1000..2000",
        &mut pager,
        200,
        "SELECT id FROM events WHERE valor BETWEEN 1000 AND 2000",
    )?);

    out.push(bench_sql(
        "events",
        "Indexed range large 10k..90k",
        &mut pager,
        50,
        "SELECT id FROM events WHERE valor BETWEEN 10000 AND 90000",
    )?);

    out.push(bench_sql(
        "events",
        "Aggregate COUNT(*) full",
        &mut pager,
        30,
        "SELECT COUNT(*) FROM events",
    )?);

    out.push(bench_sql(
        "events",
        "GROUP BY kind (low-card)",
        &mut pager,
        20,
        "SELECT kind, COUNT(*) FROM events GROUP BY kind",
    )?);

    out.push(bench_sql(
        "events",
        "DISTINCT kind",
        &mut pager,
        20,
        "SELECT DISTINCT kind FROM events",
    )?);

    // SELECT bare sin FROM (`SELECT (subq)`) no se soporta por el parser
    // hoy — defer a un bloque futuro. Skip-graceful para no abortar la suite.
    out.push(bench_sql_or_skip(
        "events",
        "Subquery escalar COUNT(view) bare-SELECT",
        &mut pager,
        20,
        "SELECT (SELECT COUNT(*) FROM events WHERE kind = 'view')",
    ));

    out.push(bench_sql(
        "events",
        "UNION two valor ranges",
        &mut pager,
        30,
        "SELECT id FROM events WHERE valor > 99000 UNION SELECT id FROM events WHERE valor < 100",
    )?);

    out.push(bench_sql(
        "events",
        "SELECT con Expr (UPPER, *2) LIMIT 100",
        &mut pager,
        200,
        "SELECT id, UPPER(kind), valor * 2 FROM events LIMIT 100",
    )?);

    close_after_bench(pager);
    Ok(())
}

fn suite_orders_lines(path: &Path, out: &mut Vec<BenchRow>) -> DbResult<()> {
    println!("\n=== orders_lines (PK compuesta) ===");
    print_header();
    let mut pager = open_for_bench(path)?;

    out.push(bench_sql(
        "orders_lines",
        "PK compuesta full (order_id+line_no)",
        &mut pager,
        1000,
        "SELECT * FROM lines WHERE order_id = 5000 AND line_no = 2",
    )?);

    out.push(bench_sql(
        "orders_lines",
        "PK compuesta partial (order_id only)",
        &mut pager,
        100,
        "SELECT * FROM lines WHERE order_id = 5000",
    )?);

    out.push(bench_sql(
        "orders_lines",
        "Composite index lookup qty+precio",
        &mut pager,
        200,
        "SELECT order_id FROM lines WHERE qty = 5 AND precio = 100",
    )?);

    out.push(bench_sql(
        "orders_lines",
        "JOIN orders×lines on order_id=7",
        &mut pager,
        100,
        "SELECT o.id, l.sku FROM orders o JOIN lines l ON l.order_id = o.id WHERE o.id = 7",
    )?);

    out.push(bench_sql(
        "orders_lines",
        "Aggregate SUM(qty*precio) GROUP",
        &mut pager,
        5,
        "SELECT order_id, SUM(qty * precio) FROM lines GROUP BY order_id LIMIT 10",
    )?);

    // [GBY-4002] BETWEEN solo se soporta sobre PK o columna INT con índice
    // ordenado. `qty` no tiene índice. Skip-graceful.
    out.push(bench_sql_or_skip(
        "orders_lines",
        "Composite range qty BETWEEN (no idx)",
        &mut pager,
        50,
        "SELECT order_id FROM lines WHERE qty BETWEEN 1 AND 5",
    ));

    close_after_bench(pager);

    // CTAS, DROP COLUMN, INSERT 1000 — operaciones one-shot
    {
        let mut pager = Pager::open(path)?;
        // CTAS — medimos como un timing único
        let ctas_sql = "CREATE TABLE lines_summary AS SELECT order_id, SUM(qty * precio) total FROM lines GROUP BY order_id";
        pager.begin()?;
        let t0 = Instant::now();
        {
            let mut engine = Engine::new(&mut pager);
            // si ya existe, drop
            let _ = engine.exec(parse("DROP TABLE lines_summary")?.remove(0));
        }
        pager.commit()?;
        pager.begin()?;
        let t1 = Instant::now();
        {
            let mut engine = Engine::new(&mut pager);
            engine.exec(parse(ctas_sql)?.remove(0))?;
        }
        let ctas_ns = t1.elapsed().as_nanos();
        pager.commit()?;
        let row = BenchRow {
            suite: "orders_lines".into(),
            name: "CTAS lines_summary (one-shot)".into(),
            iters: 1,
            p50_ns: ctas_ns,
            p95_ns: ctas_ns,
            p99_ns: ctas_ns,
            mean_ns: ctas_ns,
            stddev_ns: 0,
            total_ms: ctas_ns as f64 / 1_000_000.0,
            rows_returned: 0,
        };
        let _ = t0; // silence unused
        print_row(&row);
        out.push(row);

        // DROP COLUMN sobre orders (tabla mediana 20k filas)
        pager.begin()?;
        let t0 = Instant::now();
        {
            let mut engine = Engine::new(&mut pager);
            engine.exec(parse("ALTER TABLE orders DROP COLUMN fecha")?.remove(0))?;
        }
        let drop_ns = t0.elapsed().as_nanos();
        pager.commit()?;
        let row = BenchRow {
            suite: "orders_lines".into(),
            name: "DROP COLUMN fecha (orders 20k)".into(),
            iters: 1,
            p50_ns: drop_ns,
            p95_ns: drop_ns,
            p99_ns: drop_ns,
            mean_ns: drop_ns,
            stddev_ns: 0,
            total_ms: drop_ns as f64 / 1_000_000.0,
            rows_returned: 0,
        };
        print_row(&row);
        out.push(row);

        // INSERT 1000 líneas en tabla aux con PK compuesta
        pager.begin()?;
        {
            let mut engine = Engine::new(&mut pager);
            let _ = engine.exec(parse("DROP TABLE lines_aux")?.remove(0));
            engine.exec(parse("CREATE TABLE lines_aux (a INT NOT NULL, b INT NOT NULL, v INT NOT NULL, PRIMARY KEY (a, b))")?.remove(0))?;
        }
        pager.commit()?;

        // El bench (que sigue) hace INSERTs sin abrir tx propia — el bench
        // global necesita una tx activa para los new_page() internos.
        pager.begin()?;
        let row = bench(
            "orders_lines",
            "INSERT PK compuesta (in-tx)",
            &mut pager,
            500,
            |engine, i| {
                exec_sql(
                    engine,
                    &format!(
                        "INSERT INTO lines_aux (a, b, v) VALUES ({}, {}, {})",
                        i,
                        i * 2,
                        i
                    ),
                )
            },
        )?;
        pager.commit()?;
        out.push(row);

        pager.begin()?;
        {
            let mut engine = Engine::new(&mut pager);
            let _ = engine.exec(parse("DROP TABLE lines_aux")?.remove(0));
        }
        pager.commit()?;
        pager.close()?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// JSON output (sin deps externas)
// ---------------------------------------------------------------------------

fn dump_json(path: &Path, rows: &[BenchRow]) -> std::io::Result<()> {
    let mut s = String::from("{\n  \"results\": [\n");
    for (i, r) in rows.iter().enumerate() {
        s.push_str("    {");
        s.push_str(&format!("\"suite\":\"{}\",", r.suite));
        s.push_str(&format!("\"name\":\"{}\",", r.name.replace('"', "\\\"")));
        s.push_str(&format!("\"iters\":{},", r.iters));
        s.push_str(&format!("\"p50_ns\":{},", r.p50_ns));
        s.push_str(&format!("\"p95_ns\":{},", r.p95_ns));
        s.push_str(&format!("\"p99_ns\":{},", r.p99_ns));
        s.push_str(&format!("\"mean_ns\":{},", r.mean_ns));
        s.push_str(&format!("\"stddev_ns\":{},", r.stddev_ns));
        s.push_str(&format!("\"total_ms\":{:.4},", r.total_ms));
        s.push_str(&format!("\"rows_returned\":{}", r.rows_returned));
        s.push('}');
        if i + 1 < rows.len() {
            s.push(',');
        }
        s.push('\n');
    }
    s.push_str("  ]\n}\n");
    fs::write(path, s)
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> DbResult<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("all");

    fs::create_dir_all(BENCH_DIR)?;
    fs::create_dir_all(DBS_DIR)?;

    let p1 = PathBuf::from(format!("{}/microblog.db", DBS_DIR));
    let p2 = PathBuf::from(format!("{}/events.db", DBS_DIR));
    let p3 = PathBuf::from(format!("{}/orders_lines.db", DBS_DIR));

    if matches!(mode, "all" | "setup") {
        println!(
            "== setup microblog (users={}, posts={}) ==",
            MICROBLOG_USERS, MICROBLOG_POSTS
        );
        let t0 = Instant::now();
        setup_microblog(&p1)?;
        println!(
            "   ok en {:.2}s  size={} bytes",
            t0.elapsed().as_secs_f64(),
            db_size_bytes(&p1)
        );

        println!("== setup events  (rows={}) ==", EVENTS_ROWS);
        let t0 = Instant::now();
        setup_events(&p2)?;
        println!(
            "   ok en {:.2}s  size={} bytes",
            t0.elapsed().as_secs_f64(),
            db_size_bytes(&p2)
        );

        println!(
            "== setup orders_lines (orders={}, lines={}) ==",
            ORDERS_ROWS, LINES_ROWS
        );
        let t0 = Instant::now();
        setup_orders_lines(&p3)?;
        println!(
            "   ok en {:.2}s  size={} bytes",
            t0.elapsed().as_secs_f64(),
            db_size_bytes(&p3)
        );
    }

    if matches!(mode, "all" | "run") {
        let mut all_rows: Vec<BenchRow> = Vec::new();
        suite_microblog(&p1, &mut all_rows)?;
        suite_events(&p2, &mut all_rows)?;
        suite_orders_lines(&p3, &mut all_rows)?;

        dump_json(Path::new(RESULTS_JSON), &all_rows)?;
        println!("\nresultados crudos en {}", RESULTS_JSON);
        println!("tamaños finales:");
        println!("  microblog.db    = {} bytes", db_size_bytes(&p1));
        println!("  events.db       = {} bytes", db_size_bytes(&p2));
        println!("  orders_lines.db = {} bytes", db_size_bytes(&p3));
    }

    Ok(())
}

// Silence unused warning for escape_text (kept as utility for future).
#[allow(dead_code)]
fn _keep_escape() -> fn(&str) -> String {
    escape_text
}
