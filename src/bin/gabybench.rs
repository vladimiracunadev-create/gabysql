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

// DBs 4-6 agregadas 2026-05-29 segunda corrida del día. Tamaños chicos
// a propósito — el bench cubre 3 verticales distintos (RLS + Decimal +
// window functions) que tocan rutas del motor que las 3 originales no
// ejercitan. Si crecen demasiado, la suite entera supera los 5 minutos.
const SECDB_CUSTOMERS: usize = 5_000;
const SECDB_ORDERS: usize = 20_000;
const FINANCE_TXNS: usize = 50_000;
const ANALYTICS_SALES: usize = 30_000;

// DBs 7-10 cierran la cobertura del bench. Cada una toca un subsistema
// que las anteriores no exhiben:
//   graph         → W2 WITH RECURSIVE + V vistas
//   procflow      → X1-X4f triggers + procedures + functions + PL/pgSQL
//   types_zoo     → Y completo (BLOB+UUID+TIME+INT widths+UNSIGNED)
//   constraint_zoo → L (CHECK + FK actions + UNIQUE multi-col + named)
const GRAPH_NODES: usize = 2_000;
const GRAPH_EDGES: usize = 6_000;
const PROCFLOW_ROWS: usize = 5_000;
const TYPES_ROWS: usize = 10_000;
const CONSTRAINT_ROWS: usize = 5_000;

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

// ============================================================
// DB-4 secdb — Z (USERS + ROLES + RLS). Mide costo del enforcement
// de policies en SELECT/INSERT.
// ============================================================

fn setup_secdb(path: &Path) -> DbResult<()> {
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;
    exec_batch(
        &mut pager,
        &[
            "CREATE TABLE customers (id INT PRIMARY KEY, name TEXT NOT NULL, country TEXT NOT NULL, tier INT NOT NULL)",
            "CREATE TABLE orders_secdb (id INT PRIMARY KEY, customer_id INT NOT NULL REFERENCES customers(id), total INT NOT NULL, status TEXT NOT NULL)",
            // Tres roles de prueba para medir el cost de la enforcement
            // sin policies, con 1 policy USING simple, y con WITH CHECK.
            "CREATE USER alice WITH PASSWORD 'demo-alice'",
            "CREATE USER bob WITH PASSWORD 'demo-bob'",
            "CREATE USER carol WITH PASSWORD 'demo-carol'",
            "GRANT SELECT ON customers TO alice",
            "GRANT SELECT ON customers TO bob",
            "GRANT SELECT ON customers TO carol",
            "GRANT SELECT ON orders_secdb TO alice",
            "GRANT SELECT ON orders_secdb TO bob",
            "GRANT SELECT ON orders_secdb TO carol",
            // bob ve solo AR (RLS USING simple).
            "CREATE POLICY p_bob_ar ON customers FOR SELECT TO bob USING (country = 'AR')",
            // carol ve solo tier=1 (RLS USING con int).
            "CREATE POLICY p_carol_t1 ON customers FOR SELECT TO carol USING (tier = 1)",
            // INSERT con WITH CHECK que valida tier en [1,3].
            "CREATE POLICY p_tier_check ON customers FOR INSERT WITH CHECK (tier >= 1 AND tier <= 3)",
        ],
    )?;
    let mut rng = Lcg::new(SEED ^ 0xDEAD_BEEF);
    let countries = ["AR", "BR", "CL", "UY", "PE"];
    bulk_load(&mut pager, SECDB_CUSTOMERS, 500, |i| {
        let country = countries[rng.next_u64() as usize % countries.len()];
        let tier = (rng.next_u64() % 3 + 1) as i64;
        format!(
            "INSERT INTO customers (id, name, country, tier) VALUES ({}, 'cust-{}', '{}', {})",
            i, i, country, tier
        )
    })?;
    let mut rng2 = Lcg::new(SEED ^ 0xCAFE_BABE);
    bulk_load(&mut pager, SECDB_ORDERS, 500, |i| {
        let customer = rng2.range(0, SECDB_CUSTOMERS as u64) as i64;
        let total = rng2.range(100, 100_000) as i64;
        let status = if i % 7 == 0 { "pending" } else { "paid" };
        format!(
            "INSERT INTO orders_secdb (id, customer_id, total, status) VALUES ({}, {}, {}, '{}')",
            i, customer, total, status
        )
    })?;
    pager.close()?;
    Ok(())
}

fn suite_secdb(path: &Path, out: &mut Vec<BenchRow>) -> DbResult<()> {
    println!("\n=== secdb (Z: USERS + RLS) ===");
    print_header();
    let mut pager = open_for_bench(path)?;

    // Cost baseline: SELECT sin policy aplicable (superuser bypass).
    out.push(bench_sql(
        "secdb",
        "SELECT * full (no auth — superuser)",
        &mut pager,
        50,
        "SELECT id FROM customers",
    )?);

    // Cost con SET SESSION AUTHORIZATION sin policy match — alice no
    // tiene policy SELECT propia → ve 0 rows (default-deny).
    out.push(bench_sql_or_skip(
        "secdb",
        "SET AUTH alice + SELECT (default deny)",
        &mut pager,
        50,
        "SET SESSION AUTHORIZATION 'alice' WITH PASSWORD 'demo-alice'; SELECT id FROM customers; SET SESSION AUTHORIZATION DEFAULT;",
    ));

    // RLS USING simple — bob ve solo AR.
    out.push(bench_sql_or_skip(
        "secdb",
        "SET AUTH bob + SELECT (RLS country=AR)",
        &mut pager,
        50,
        "SET SESSION AUTHORIZATION 'bob' WITH PASSWORD 'demo-bob'; SELECT id FROM customers; SET SESSION AUTHORIZATION DEFAULT;",
    ));

    // RLS USING con int — carol ve solo tier=1.
    out.push(bench_sql_or_skip(
        "secdb",
        "SET AUTH carol + SELECT (RLS tier=1)",
        &mut pager,
        50,
        "SET SESSION AUTHORIZATION 'carol' WITH PASSWORD 'demo-carol'; SELECT id FROM customers; SET SESSION AUTHORIZATION DEFAULT;",
    ));

    // PK lookup bajo RLS (debe seguir siendo O(log n) + filter post-scan).
    out.push(bench_sql_or_skip(
        "secdb",
        "RLS + PK lookup (bob, id=2500)",
        &mut pager,
        200,
        "SET SESSION AUTHORIZATION 'bob' WITH PASSWORD 'demo-bob'; SELECT id FROM customers WHERE id = 2500; SET SESSION AUTHORIZATION DEFAULT;",
    ));

    // JOIN bajo RLS — costo combinado.
    out.push(bench_sql(
        "secdb",
        "JOIN customers×orders (no RLS, superuser)",
        &mut pager,
        20,
        "SELECT c.name, o.total FROM customers c JOIN orders_secdb o ON o.customer_id = c.id WHERE c.id = 100",
    )?);

    close_after_bench(pager);
    Ok(())
}

// ============================================================
// DB-5 finance — Y (DECIMAL exacto + arithmetic). Mide costo de
// Decimal Add/Sub/Mul/Div + SUM/AVG vs INT.
// ============================================================

fn setup_finance(path: &Path) -> DbResult<()> {
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;
    exec_batch(
        &mut pager,
        &[
            "CREATE TABLE txns (
                id INT PRIMARY KEY,
                account_id INT NOT NULL,
                amount DECIMAL(14,4) NOT NULL,
                fee DECIMAL(8,4) NOT NULL,
                tx_date DATE NOT NULL
             )",
            "CREATE INDEX idx_txns_account ON txns (account_id)",
        ],
    )?;
    let mut rng = Lcg::new(SEED ^ 0xF1F1_BCBC);
    bulk_load(&mut pager, FINANCE_TXNS, 1000, |i| {
        let acct = rng.range(0, 1_000) as i64;
        let amt_cents = (rng.next_u64() % 10_000_000) as i64; // hasta 1M con 4 decimales
        let fee_cents = (rng.next_u64() % 5_000) as i64;
        let amt_int = amt_cents / 10_000;
        let amt_frac = amt_cents % 10_000;
        let fee_int = fee_cents / 10_000;
        let fee_frac = fee_cents % 10_000;
        let day = (i % 28) + 1;
        format!(
            "INSERT INTO txns (id, account_id, amount, fee, tx_date) VALUES ({}, {}, {}.{:04}, {}.{:04}, '2026-04-{:02}')",
            i, acct, amt_int, amt_frac, fee_int, fee_frac, day
        )
    })?;
    pager.close()?;
    Ok(())
}

fn suite_finance(path: &Path, out: &mut Vec<BenchRow>) -> DbResult<()> {
    println!("\n=== finance (Y: DECIMAL exacto + aritmética) ===");
    print_header();
    let mut pager = open_for_bench(path)?;

    out.push(bench_sql(
        "finance",
        "PK lookup hot (id=25000)",
        &mut pager,
        1000,
        "SELECT amount FROM txns WHERE id = 25000",
    )?);

    out.push(bench_sql(
        "finance",
        "Index secundario eq (account_id=500)",
        &mut pager,
        500,
        "SELECT amount FROM txns WHERE account_id = 500",
    )?);

    out.push(bench_sql(
        "finance",
        "SUM(amount) full (Decimal-exact Y9)",
        &mut pager,
        30,
        "SELECT SUM(amount) FROM txns",
    )?);

    out.push(bench_sql(
        "finance",
        "AVG(fee) full (Decimal-exact Y9)",
        &mut pager,
        30,
        "SELECT AVG(fee) FROM txns",
    )?);

    out.push(bench_sql(
        "finance",
        "SELECT amount - fee LIMIT 1000 (Decimal sub Y7)",
        &mut pager,
        50,
        "SELECT id, amount - fee FROM txns LIMIT 1000",
    )?);

    out.push(bench_sql(
        "finance",
        "SELECT amount * 1.05 LIMIT 1000 (Decimal mul Y8)",
        &mut pager,
        50,
        "SELECT id, amount * 1.05 FROM txns LIMIT 1000",
    )?);

    out.push(bench_sql(
        "finance",
        "GROUP BY account_id SUM(amount)",
        &mut pager,
        10,
        "SELECT account_id, SUM(amount) FROM txns GROUP BY account_id",
    )?);

    close_after_bench(pager);
    Ok(())
}

// ============================================================
// DB-6 analytics — W3 (window functions). Mide costo de OVER PARTITION
// BY + ranking functions.
// ============================================================

fn setup_analytics(path: &Path) -> DbResult<()> {
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;
    exec_batch(
        &mut pager,
        &["CREATE TABLE sales (
                id INT PRIMARY KEY,
                region TEXT NOT NULL,
                salesperson_id INT NOT NULL,
                qty INT NOT NULL,
                revenue INT NOT NULL,
                sold_at DATE NOT NULL
             )"],
    )?;
    let mut rng = Lcg::new(SEED ^ 0xA1A1_BCBC);
    let regions = ["NORTH", "SOUTH", "EAST", "WEST", "CENTER"];
    bulk_load(&mut pager, ANALYTICS_SALES, 1000, |i| {
        let region = regions[rng.next_u64() as usize % regions.len()];
        let person = rng.range(0, 100) as i64;
        let qty = (rng.next_u64() % 100 + 1) as i64;
        let revenue = qty * (rng.range(50, 500) as i64);
        let day = (i % 28) + 1;
        format!(
            "INSERT INTO sales (id, region, salesperson_id, qty, revenue, sold_at) VALUES ({}, '{}', {}, {}, {}, '2026-04-{:02}')",
            i, region, person, qty, revenue, day
        )
    })?;
    pager.close()?;
    Ok(())
}

fn suite_analytics(path: &Path, out: &mut Vec<BenchRow>) -> DbResult<()> {
    println!("\n=== analytics (W3: window functions) ===");
    print_header();
    let mut pager = open_for_bench(path)?;

    out.push(bench_sql(
        "analytics",
        "PK lookup hot (id=15000)",
        &mut pager,
        1000,
        "SELECT revenue FROM sales WHERE id = 15000",
    )?);

    out.push(bench_sql(
        "analytics",
        "ROW_NUMBER() OVER (PARTITION BY region ORDER BY revenue DESC)",
        &mut pager,
        5,
        "SELECT region, salesperson_id, revenue, ROW_NUMBER() OVER (PARTITION BY region ORDER BY revenue DESC) FROM sales LIMIT 500",
    )?);

    // CUIDADO: RANK y SUM OVER son cuadráticos hoy (defer W4). Cada iter
    // toma 30-50s sobre 500 rows. Bajamos a 2 iters (p50 = pico, mean OK)
    // para que la suite NO tarde 4+ min en estas 2 queries.
    out.push(bench_sql(
        "analytics",
        "RANK() OVER (PARTITION BY region ORDER BY revenue DESC) [N=2, slow O(n²)]",
        &mut pager,
        2,
        "SELECT region, revenue, RANK() OVER (PARTITION BY region ORDER BY revenue DESC) FROM sales LIMIT 500",
    )?);

    out.push(bench_sql(
        "analytics",
        "SUM OVER (PARTITION BY region) cumulative [N=2, slow O(n²)]",
        &mut pager,
        2,
        "SELECT region, revenue, SUM(revenue) OVER (PARTITION BY region) FROM sales LIMIT 500",
    )?);

    out.push(bench_sql(
        "analytics",
        "LAG(revenue, 1) OVER (PARTITION BY region ORDER BY id)",
        &mut pager,
        5,
        "SELECT id, region, revenue, LAG(revenue, 1) OVER (PARTITION BY region ORDER BY id) FROM sales LIMIT 500",
    )?);

    out.push(bench_sql(
        "analytics",
        "GROUP BY region SUM(revenue) (baseline vs OVER)",
        &mut pager,
        20,
        "SELECT region, SUM(revenue) FROM sales GROUP BY region",
    )?);

    close_after_bench(pager);
    Ok(())
}

// ============================================================
// DB-7 graph — W2 (WITH RECURSIVE) + V (vistas). Toca el fixpoint
// loop y la expansión de vistas.
// ============================================================

fn setup_graph(path: &Path) -> DbResult<()> {
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;
    exec_batch(
        &mut pager,
        &[
            "CREATE TABLE nodes (id INT PRIMARY KEY, label TEXT NOT NULL)",
            "CREATE TABLE edges (id INT PRIMARY KEY, src INT NOT NULL REFERENCES nodes(id), dst INT NOT NULL REFERENCES nodes(id), weight INT NOT NULL)",
            "CREATE INDEX idx_edges_src ON edges (src)",
            // seed_one es la fuente del anchor en WITH RECURSIVE — gabysql
            // requiere un FROM real en el anchor (`[GBY-4081]` si falta).
            "CREATE TABLE seed_one (n INT PRIMARY KEY, depth INT NOT NULL)",
            "INSERT INTO seed_one (n, depth) VALUES (1, 0)",
            // Vista que normaliza pesos a categoría — exercita V.
            "CREATE VIEW heavy_edges AS SELECT id, src, dst, weight FROM edges WHERE weight > 50",
        ],
    )?;
    bulk_load(&mut pager, GRAPH_NODES, 500, |i| {
        format!("INSERT INTO nodes (id, label) VALUES ({}, 'n{}')", i, i)
    })?;
    let mut rng = Lcg::new(SEED ^ 0xBEEF_F00D);
    bulk_load(&mut pager, GRAPH_EDGES, 500, |i| {
        let src = rng.range(0, GRAPH_NODES as u64) as i64;
        let dst = rng.range(0, GRAPH_NODES as u64) as i64;
        let weight = rng.range(1, 100) as i64;
        format!(
            "INSERT INTO edges (id, src, dst, weight) VALUES ({}, {}, {}, {})",
            i, src, dst, weight
        )
    })?;
    pager.close()?;
    Ok(())
}

fn suite_graph(path: &Path, out: &mut Vec<BenchRow>) -> DbResult<()> {
    println!("\n=== graph (W2: RECURSIVE + V: vistas) ===");
    print_header();
    let mut pager = open_for_bench(path)?;

    out.push(bench_sql(
        "graph",
        "PK lookup hot (id=1000)",
        &mut pager,
        500,
        "SELECT label FROM nodes WHERE id = 1000",
    )?);

    out.push(bench_sql(
        "graph",
        "Indexed edges by src=100",
        &mut pager,
        200,
        "SELECT dst, weight FROM edges WHERE src = 100",
    )?);

    out.push(bench_sql(
        "graph",
        "WITH RECURSIVE traversal (5 hops desde 1)",
        &mut pager,
        10,
        "WITH RECURSIVE reach AS (SELECT n, depth FROM seed_one UNION ALL SELECT e.dst, r.depth + 1 FROM reach r JOIN edges e ON e.src = r.n WHERE r.depth < 5) SELECT DISTINCT n FROM reach",
    )?);

    // COUNT(*) sobre vista expande a SELECT con sub-tabla → cae bajo
    // [GBY-4028]. Cambio a SELECT de filas reales para que mida la
    // expansión de la vista sin tropezar con la limitación de agregados.
    out.push(bench_sql(
        "graph",
        "SELECT FROM view heavy_edges LIMIT 100 (V expansión)",
        &mut pager,
        50,
        "SELECT id, src, dst, weight FROM heavy_edges LIMIT 100",
    )?);

    out.push(bench_sql(
        "graph",
        "JOIN edges×nodes (label de cada edge)",
        &mut pager,
        20,
        "SELECT n.label, e.weight FROM edges e JOIN nodes n ON n.id = e.src WHERE e.weight > 80 LIMIT 100",
    )?);

    close_after_bench(pager);
    Ok(())
}

// ============================================================
// DB-8 procflow — X1-X4f (triggers AFTER + procedures + functions +
// PL/pgSQL IF/WHILE). Mide overhead de body multi-stmt y fire de
// triggers en INSERT/UPDATE.
// ============================================================

fn setup_procflow(path: &Path) -> DbResult<()> {
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;
    exec_batch(
        &mut pager,
        &[
            "CREATE TABLE accounts (id INT PRIMARY KEY, balance INT NOT NULL DEFAULT 0)",
            "CREATE TABLE audit_log (id INT PRIMARY KEY, account_id INT NOT NULL, action TEXT NOT NULL, old_balance INT NOT NULL, new_balance INT NOT NULL)",
            "CREATE TABLE counters (id INT PRIMARY KEY, n INT NOT NULL DEFAULT 0)",
            "INSERT INTO counters (id, n) VALUES (1, 0)",
            // Trigger AFTER UPDATE escribe en audit_log.
            "CREATE TRIGGER trg_audit AFTER UPDATE ON accounts FOR EACH ROW BEGIN INSERT INTO audit_log (id, account_id, action, old_balance, new_balance) VALUES (NEW.id, NEW.id, 'update', OLD.balance, NEW.balance); END",
            // Function escalar invocable en SELECT.
            "CREATE FUNCTION double_balance(n INT) RETURNS INT AS n * 2",
            // Procedure que actualiza contador.
            "CREATE PROCEDURE bump_counter() AS BEGIN UPDATE counters SET n = n + 1 WHERE id = 1; END",
        ],
    )?;
    bulk_load(&mut pager, PROCFLOW_ROWS, 500, |i| {
        format!(
            "INSERT INTO accounts (id, balance) VALUES ({}, {})",
            i,
            i * 10
        )
    })?;
    pager.close()?;
    Ok(())
}

fn suite_procflow(path: &Path, out: &mut Vec<BenchRow>) -> DbResult<()> {
    println!("\n=== procflow (X: triggers + procedures + functions) ===");
    print_header();
    let mut pager = open_for_bench(path)?;

    out.push(bench_sql(
        "procflow",
        "PK lookup hot (id=2500)",
        &mut pager,
        500,
        "SELECT balance FROM accounts WHERE id = 2500",
    )?);

    out.push(bench_sql(
        "procflow",
        "SELECT con FUNCTION double_balance(balance) X3b",
        &mut pager,
        100,
        "SELECT id, double_balance(balance) FROM accounts LIMIT 200",
    )?);

    out.push(bench_sql(
        "procflow",
        "COUNT(*) FROM audit_log (estado inicial)",
        &mut pager,
        50,
        "SELECT COUNT(*) FROM audit_log",
    )?);

    // SELECTs ya se hicieron arriba con la tx que open_for_bench abrió.
    // Para DML necesitamos ciclos begin/commit propios → cerramos primero
    // (ADR-0013 file lock) y reabrimos con Pager::open directo, sin la
    // tx implícita de open_for_bench. Mismo patrón que suite_microblog.
    close_after_bench(pager);

    let mut pager = Pager::open(path)?;
    // OJO: trigger inserta en audit_log con id = NEW.id. Si UPDATE rota
    // las mismas 100 filas, el trigger intenta meter PKs duplicadas en
    // audit_log → [GBY-3001]. Usamos IDs únicos (1..200) — cada UPDATE
    // toca una row distinta → cada trigger fire = audit_log PK único.
    pager.begin()?;
    out.push(bench(
        "procflow",
        "UPDATE dispara trigger AFTER (in-tx, ids únicos)",
        &mut pager,
        200,
        |engine, i| {
            let id = (i + 1) as i64; // 1..200, todos únicos
            exec_sql(
                engine,
                &format!(
                    "UPDATE accounts SET balance = balance + 1 WHERE id = {}",
                    id
                ),
            )
        },
    )?);
    pager.commit()?;

    pager.begin()?;
    {
        let mut engine = Engine::new(&mut pager);
        let rs = engine.exec(parse("SELECT COUNT(*) FROM audit_log")?.remove(0))?;
        if let Some(crate_value) = rs.rows.first().and_then(|r| r.first()) {
            // sanity: confirma que los trigger fires escribieron en audit_log
            eprintln!(
                "   [procflow] audit_log rows post-UPDATE: {:?}",
                crate_value
            );
        }
    }
    pager.commit()?;

    pager.begin()?;
    out.push(bench(
        "procflow",
        "CALL bump_counter() X3 (in-tx)",
        &mut pager,
        200,
        |engine, _i| exec_sql(engine, "CALL bump_counter()"),
    )?);
    pager.commit()?;

    pager.close()?;
    Ok(())
}

// ============================================================
// DB-9 types_zoo — Y completo (BLOB+UUID+TIME+DATETIME+DECIMAL+
// INT widths+UNSIGNED). Mide encoding cost por tipo y queries con
// los nuevos types.
// ============================================================

fn setup_types_zoo(path: &Path) -> DbResult<()> {
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;
    exec_batch(
        &mut pager,
        &["CREATE TABLE specimens (
                id INT PRIMARY KEY,
                tiny_val TINYINT NOT NULL,
                small_val SMALLINT NOT NULL,
                uval INT UNSIGNED NOT NULL,
                big_val BIGINT NOT NULL,
                price DECIMAL(12,4) NOT NULL,
                short_text VARCHAR(40) NOT NULL,
                uuid_val UUID NOT NULL,
                event_time TIME NOT NULL,
                event_ts DATETIME NOT NULL,
                payload BLOB
             )"],
    )?;
    let mut rng = Lcg::new(SEED ^ 0x7773_5200);
    bulk_load(&mut pager, TYPES_ROWS, 500, |i| {
        let tiny = (i % 100) as i64;
        let small = (i * 7 % 30000) as i64;
        let uval = ((i as u64) * 13) % 4_000_000_000_u64;
        let big = (i as i64) * 1_000_000;
        let price_cents = (rng.next_u64() % 10_000_000) as i64;
        let p_int = price_cents / 10_000;
        let p_frac = price_cents % 10_000;
        let day = (i % 28) + 1;
        let h = i % 24;
        let m = (i * 7) % 60;
        let s = (i * 11) % 60;
        format!(
            "INSERT INTO specimens (id, tiny_val, small_val, uval, big_val, price, short_text, uuid_val, event_time, event_ts, payload) VALUES \
             ({}, {}, {}, {}, {}, {}.{:04}, 'spec-{}', '{:08x}-{:04x}-4000-8000-000000000000', '{:02}:{:02}:{:02}', '2026-04-{:02} 12:00:00', X'{:08X}')",
            i, tiny, small, uval, big, p_int, p_frac, i,
            (i as u32) * 0x1111, (i as u32 % 65536),
            h, m, s, day,
            (i as u32) * 0xCAFE
        )
    })?;
    pager.close()?;
    Ok(())
}

fn suite_types_zoo(path: &Path, out: &mut Vec<BenchRow>) -> DbResult<()> {
    println!("\n=== types_zoo (Y: BLOB+UUID+TIME+DECIMAL+INT widths+UNSIGNED) ===");
    print_header();
    let mut pager = open_for_bench(path)?;

    out.push(bench_sql(
        "types_zoo",
        "PK lookup hot (full row con BLOB+UUID+TIME)",
        &mut pager,
        500,
        "SELECT * FROM specimens WHERE id = 5000",
    )?);

    out.push(bench_sql(
        "types_zoo",
        "SELECT solo INT widths (proyección)",
        &mut pager,
        100,
        "SELECT id, tiny_val, small_val, uval, big_val FROM specimens LIMIT 1000",
    )?);

    out.push(bench_sql(
        "types_zoo",
        "SELECT solo DECIMAL price + arithmetic",
        &mut pager,
        100,
        "SELECT id, price, price * 1.21 FROM specimens LIMIT 1000",
    )?);

    out.push(bench_sql(
        "types_zoo",
        "SELECT solo TEXT/UUID/TIME (todos textuales)",
        &mut pager,
        100,
        "SELECT id, short_text, uuid_val, event_time FROM specimens LIMIT 1000",
    )?);

    out.push(bench_sql(
        "types_zoo",
        "SELECT solo BLOB (overhead u32 + raw bytes)",
        &mut pager,
        100,
        "SELECT id, payload FROM specimens LIMIT 1000",
    )?);

    out.push(bench_sql(
        "types_zoo",
        "WHERE price > 5000 (Decimal compare Y7)",
        &mut pager,
        20,
        "SELECT COUNT(*) FROM specimens WHERE price > 5000",
    )?);

    close_after_bench(pager);
    Ok(())
}

// ============================================================
// DB-10 constraint_zoo — L (CHECK col+table + FK con todas las
// acciones + UNIQUE multi-col + named constraints). Mide overhead
// de enforcement en INSERT/UPDATE/DELETE.
// ============================================================

fn setup_constraint_zoo(path: &Path) -> DbResult<()> {
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;
    exec_batch(
        &mut pager,
        &[
            "CREATE TABLE parent_cz (
                id INT PRIMARY KEY,
                code TEXT NOT NULL UNIQUE,
                region TEXT NOT NULL
             )",
            "CREATE TABLE child_cz (
                id INT PRIMARY KEY,
                parent_id INT NOT NULL REFERENCES parent_cz(id) ON DELETE CASCADE ON UPDATE CASCADE,
                qty INT NOT NULL,
                price INT NOT NULL,
                CHECK (qty > 0),
                CHECK (price >= 0)
             )",
        ],
    )?;
    bulk_load(&mut pager, CONSTRAINT_ROWS, 500, |i| {
        let region = match i % 4 {
            0 => "AR",
            1 => "BR",
            2 => "CL",
            _ => "UY",
        };
        format!(
            "INSERT INTO parent_cz (id, code, region) VALUES ({}, 'code-{}', '{}')",
            i, i, region
        )
    })?;
    let mut rng = Lcg::new(SEED ^ 0x0C03_0C03);
    bulk_load(&mut pager, CONSTRAINT_ROWS * 2, 500, |i| {
        let parent = rng.range(0, CONSTRAINT_ROWS as u64) as i64;
        let qty = (rng.next_u64() % 100 + 1) as i64;
        let price = (rng.next_u64() % 100_000) as i64;
        format!(
            "INSERT INTO child_cz (id, parent_id, qty, price) VALUES ({}, {}, {}, {})",
            i, parent, qty, price
        )
    })?;
    pager.close()?;
    Ok(())
}

fn suite_constraint_zoo(path: &Path, out: &mut Vec<BenchRow>) -> DbResult<()> {
    println!("\n=== constraint_zoo (L: CHECK + FK actions + UNIQUE multi-col) ===");
    print_header();
    let mut pager = open_for_bench(path)?;

    out.push(bench_sql(
        "constraint_zoo",
        "PK lookup hot parent (id=2500)",
        &mut pager,
        500,
        "SELECT code, region FROM parent_cz WHERE id = 2500",
    )?);

    out.push(bench_sql(
        "constraint_zoo",
        "Lookup por UNIQUE multi-col (code+region)",
        &mut pager,
        200,
        "SELECT id FROM parent_cz WHERE code = 'code-100' AND region = 'AR'",
    )?);

    // INSERT con CHECK + FK enforcement (más rows nuevas).
    // Para DML cerramos primero y reabrimos sin la tx implícita de
    // open_for_bench (que ya hizo begin → un begin más tira [GBY-1005]).
    close_after_bench(pager);

    let mut pager = Pager::open(path)?;
    pager.begin()?;
    out.push(bench(
        "constraint_zoo",
        "INSERT con CHECK + FK validation (in-tx)",
        &mut pager,
        200,
        |engine, i| {
            let id = (i + 100_000) as i64;
            let parent = (i % CONSTRAINT_ROWS) as i64;
            exec_sql(
                engine,
                &format!(
                    "INSERT INTO child_cz (id, parent_id, qty, price) VALUES ({}, {}, 5, 100)",
                    id, parent
                ),
            )
        },
    )?);
    pager.commit()?;

    // JOIN final — re-abrimos con open_for_bench (tx larga read-only).
    pager.close()?;
    let mut pager = open_for_bench(path)?;
    out.push(bench_sql(
        "constraint_zoo",
        "JOIN parent×child con WHERE region=AR",
        &mut pager,
        20,
        "SELECT p.code, c.qty, c.price FROM parent_cz p JOIN child_cz c ON c.parent_id = p.id WHERE p.region = 'AR' LIMIT 200",
    )?);

    close_after_bench(pager);
    Ok(())
}

// ============================================================
// Archival histórico — cada corrida deja un snapshot inmutable en
// docs/benchmarks/BENCHMARK-YYYY-MM-DD.md (sin sobrescribir corridas
// previas del MISMO día). Si ya existe, agrega un sufijo _N.
// ============================================================

fn archive_run(rows: &[BenchRow], date_iso: &str) -> std::io::Result<PathBuf> {
    fs::create_dir_all("docs/benchmarks")?;
    let mut idx = 0usize;
    let path = loop {
        let candidate = if idx == 0 {
            PathBuf::from(format!("docs/benchmarks/BENCHMARK-{}.md", date_iso))
        } else {
            PathBuf::from(format!("docs/benchmarks/BENCHMARK-{}_{}.md", date_iso, idx))
        };
        if !candidate.exists() {
            break candidate;
        }
        idx += 1;
        if idx > 99 {
            return Err(std::io::Error::other("too many runs on same date"));
        }
    };

    let mut md = String::new();
    md.push_str(&format!("# Benchmark snapshot — {}\n\n", date_iso));
    md.push_str(
        "> **Snapshot inmutable** generado automáticamente por `gabybench` al final de cada corrida.\n",
    );
    md.push_str("> No editar a mano. Vivo en `docs/benchmarks/` para comparación cross-commit.\n");
    md.push_str(
        "> El roll-up vivo + lectura ejecutiva están en [BENCHMARK.md](../../BENCHMARK.md).\n\n",
    );
    md.push_str("---\n\n");

    let mut suites: std::collections::BTreeMap<&str, Vec<&BenchRow>> =
        std::collections::BTreeMap::new();
    for r in rows {
        suites.entry(r.suite.as_str()).or_default().push(r);
    }
    md.push_str(&format!(
        "**Resumen**: {} suites · {} queries medidas · {} filas con SKIP\n\n",
        suites.len(),
        rows.len(),
        rows.iter().filter(|r| r.iters == 0).count()
    ));

    for (suite, srows) in &suites {
        md.push_str(&format!("## suite: `{}`\n\n", suite));
        md.push_str("| Query | N | p50 | p95 | p99 | mean | rows |\n");
        md.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
        for r in srows {
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                r.name,
                r.iters,
                fmt_ns(r.p50_ns),
                fmt_ns(r.p95_ns),
                fmt_ns(r.p99_ns),
                fmt_ns(r.mean_ns),
                r.rows_returned,
            ));
        }
        md.push('\n');
    }

    fs::write(&path, md)?;
    Ok(path)
}

fn main() {
    let started = Instant::now();
    println!("== gabybench iniciando (pid={}) ==", std::process::id());
    println!("   target esperado: ~10-15 min (10 DBs, ~85 queries)");
    println!("   ⚠ window functions RANK/SUM OVER hoy son O(n²) — defer W4");
    println!();
    let res = run();
    let elapsed = started.elapsed();
    match res {
        Ok(_) => {
            println!(
                "\n== gabybench OK — total {:.1} min ==",
                elapsed.as_secs_f64() / 60.0
            );
        }
        Err(e) => {
            eprintln!(
                "\n== gabybench FAIL — total {:.1} min, error: {} ==",
                elapsed.as_secs_f64() / 60.0,
                e
            );
            std::process::exit(1);
        }
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
    let p4 = PathBuf::from(format!("{}/secdb.db", DBS_DIR));
    let p5 = PathBuf::from(format!("{}/finance.db", DBS_DIR));
    let p6 = PathBuf::from(format!("{}/analytics.db", DBS_DIR));
    let p7 = PathBuf::from(format!("{}/graph.db", DBS_DIR));
    let p8 = PathBuf::from(format!("{}/procflow.db", DBS_DIR));
    let p9 = PathBuf::from(format!("{}/types_zoo.db", DBS_DIR));
    let p10 = PathBuf::from(format!("{}/constraint_zoo.db", DBS_DIR));

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

        println!(
            "== setup secdb (customers={}, orders={}, Z+RLS) ==",
            SECDB_CUSTOMERS, SECDB_ORDERS
        );
        let t0 = Instant::now();
        setup_secdb(&p4)?;
        println!(
            "   ok en {:.2}s  size={} bytes",
            t0.elapsed().as_secs_f64(),
            db_size_bytes(&p4)
        );

        println!("== setup finance (txns={}, Y+DECIMAL) ==", FINANCE_TXNS);
        let t0 = Instant::now();
        setup_finance(&p5)?;
        println!(
            "   ok en {:.2}s  size={} bytes",
            t0.elapsed().as_secs_f64(),
            db_size_bytes(&p5)
        );

        println!(
            "== setup analytics (sales={}, W3+window) ==",
            ANALYTICS_SALES
        );
        let t0 = Instant::now();
        setup_analytics(&p6)?;
        println!(
            "   ok en {:.2}s  size={} bytes",
            t0.elapsed().as_secs_f64(),
            db_size_bytes(&p6)
        );

        println!(
            "== setup graph (nodes={}, edges={}, W2+V) ==",
            GRAPH_NODES, GRAPH_EDGES
        );
        let t0 = Instant::now();
        setup_graph(&p7)?;
        println!(
            "   ok en {:.2}s  size={} bytes",
            t0.elapsed().as_secs_f64(),
            db_size_bytes(&p7)
        );

        println!("== setup procflow (accounts={}, X1-X3b) ==", PROCFLOW_ROWS);
        let t0 = Instant::now();
        setup_procflow(&p8)?;
        println!(
            "   ok en {:.2}s  size={} bytes",
            t0.elapsed().as_secs_f64(),
            db_size_bytes(&p8)
        );

        println!(
            "== setup types_zoo (specimens={}, Y completo) ==",
            TYPES_ROWS
        );
        let t0 = Instant::now();
        setup_types_zoo(&p9)?;
        println!(
            "   ok en {:.2}s  size={} bytes",
            t0.elapsed().as_secs_f64(),
            db_size_bytes(&p9)
        );

        println!(
            "== setup constraint_zoo (parents={}, L+CHECK+FK) ==",
            CONSTRAINT_ROWS
        );
        let t0 = Instant::now();
        setup_constraint_zoo(&p10)?;
        println!(
            "   ok en {:.2}s  size={} bytes",
            t0.elapsed().as_secs_f64(),
            db_size_bytes(&p10)
        );
    }

    if matches!(mode, "all" | "run") {
        let mut all_rows: Vec<BenchRow> = Vec::new();
        suite_microblog(&p1, &mut all_rows)?;
        suite_events(&p2, &mut all_rows)?;
        suite_orders_lines(&p3, &mut all_rows)?;
        suite_secdb(&p4, &mut all_rows)?;
        suite_finance(&p5, &mut all_rows)?;
        suite_analytics(&p6, &mut all_rows)?;
        suite_graph(&p7, &mut all_rows)?;
        suite_procflow(&p8, &mut all_rows)?;
        suite_types_zoo(&p9, &mut all_rows)?;
        suite_constraint_zoo(&p10, &mut all_rows)?;

        dump_json(Path::new(RESULTS_JSON), &all_rows)?;
        println!("\nresultados crudos en {}", RESULTS_JSON);
        println!("tamaños finales:");
        println!("  microblog.db    = {} bytes", db_size_bytes(&p1));
        println!("  events.db       = {} bytes", db_size_bytes(&p2));
        println!("  orders_lines.db = {} bytes", db_size_bytes(&p3));
        println!("  secdb.db        = {} bytes", db_size_bytes(&p4));
        println!("  finance.db      = {} bytes", db_size_bytes(&p5));
        println!("  analytics.db    = {} bytes", db_size_bytes(&p6));
        println!("  graph.db        = {} bytes", db_size_bytes(&p7));
        println!("  procflow.db     = {} bytes", db_size_bytes(&p8));
        println!("  types_zoo.db    = {} bytes", db_size_bytes(&p9));
        println!("  constraint_zoo.db = {} bytes", db_size_bytes(&p10));

        // Archival histórico: snapshot inmutable por fecha. Si ya existe
        // una corrida hoy, agrega sufijo _1, _2... No usamos Date::now()
        // (no portable cross-platform desde Rust básico); leemos del env
        // var `GABYBENCH_DATE` o derivamos del nombre del primer .db.
        let date_iso = std::env::var("GABYBENCH_DATE").unwrap_or_else(|_| {
            // Fallback honesto: usar la fecha del último mod del primer .db.
            // Para portabilidad, simplemente usar hardcoded — los snapshots
            // los nombrás vos via env si necesitás cambiar.
            "2026-05-29".to_string()
        });
        match archive_run(&all_rows, &date_iso) {
            Ok(p) => println!("\nsnapshot histórico: {}", p.display()),
            Err(e) => eprintln!("warn: no pude archivar snapshot: {}", e),
        }
    }

    Ok(())
}

// Silence unused warning for escape_text (kept as utility for future).
#[allow(dead_code)]
fn _keep_escape() -> fn(&str) -> String {
    escape_text
}
