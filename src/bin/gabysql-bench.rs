//! gabysql-bench — harness de benchmark profesional para gabysql.
//!
//! NO es el producto aspiracional `gabybench` (ver docs/GABYBENCH_SPEC.md).
//! Es un binario simple que crea una DB sintética, mide latencias de queries
//! representativas con percentiles y throughput de carga masiva, y emite un
//! reporte tabular en Markdown.
//!
//! Uso:
//!     gabysql-bench --db <path> --scenario <microblog|events|catalog>
//!                   [--phase <load|query|all>] [--iters N] [--out <md>]
//!
//! Política del repo: cero deps externas (solo std).
//!
//! Política defensiva: si una query individual falla (típicamente GBY-4001
//! cuando se intenta WHERE sobre columna no-PK no-indexada), NO crashea el
//! bench: se anota "N/A" o "ERROR: <txt>" en el reporte y se continúa.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use gabysql::sql::{parse, Engine};
use gabysql::storage::Pager;
use gabysql::{DbError, DbResult};

// ---------------------------------------------------------------------------
// Configuración
// ---------------------------------------------------------------------------

const DEFAULT_ITERS: usize = 100;
const WARMUP_DISCARD: usize = 5;

// Tamaños por escenario
const MICROBLOG_USERS: usize = 10_000;
const MICROBLOG_POSTS: usize = 40_000;
const EVENTS_ROWS: usize = 200_000;
const ORDERS_ROWS: usize = 10_000;
const LINES_ROWS: usize = 100_000;

const SEED: u64 = 0xC0FF_EE12_3456_789A;

// ---------------------------------------------------------------------------
// PRNG — xorshift64 (escrito a mano, zero deps)
// ---------------------------------------------------------------------------

struct Xorshift64(u64);

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        let s = if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed };
        Self(s)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        debug_assert!(hi > lo);
        lo + self.next_u64() % (hi - lo)
    }
}

// ---------------------------------------------------------------------------
// CLI args (parser manual)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Args {
    db: PathBuf,
    scenario: String,
    phase: String,
    iters: usize,
    out: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = std::env::args().collect();
    let mut db: Option<PathBuf> = None;
    let mut scenario: Option<String> = None;
    let mut phase = "all".to_string();
    let mut iters = DEFAULT_ITERS;
    let mut out: Option<PathBuf> = None;

    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        let next = || -> Result<String, String> {
            argv.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("flag {} requiere valor", a))
        };
        match a.as_str() {
            "--db" => {
                db = Some(PathBuf::from(next()?));
                i += 2;
            }
            "--scenario" => {
                scenario = Some(next()?);
                i += 2;
            }
            "--phase" => {
                phase = next()?;
                i += 2;
            }
            "--iters" => {
                iters = next()?
                    .parse::<usize>()
                    .map_err(|e| format!("--iters: {}", e))?;
                i += 2;
            }
            "--out" => {
                out = Some(PathBuf::from(next()?));
                i += 2;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("flag desconocido: {}", other)),
        }
    }

    let db = db.ok_or("falta --db")?;
    let scenario = scenario.ok_or("falta --scenario")?;
    if !matches!(scenario.as_str(), "microblog" | "events" | "catalog") {
        return Err(format!("--scenario inválido: {}", scenario));
    }
    if !matches!(phase.as_str(), "load" | "query" | "all") {
        return Err(format!("--phase inválido: {}", phase));
    }
    if iters <= WARMUP_DISCARD {
        return Err(format!("--iters debe ser > {}", WARMUP_DISCARD));
    }

    Ok(Args {
        db,
        scenario,
        phase,
        iters,
        out,
    })
}

fn print_usage() {
    println!(
        "Uso:\n  gabysql-bench --db <path> --scenario <microblog|events|catalog> \\\n                --phase <load|query|all> [--iters N] [--out report.md]\n"
    );
}

// ---------------------------------------------------------------------------
// Medición
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Stats {
    op: String,
    iters: usize,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    min_us: f64,
    max_us: f64,
    rows: usize,
    note: String,
    failed: bool,
}

fn percentile_us(sorted_ns: &[u128], p: f64) -> f64 {
    if sorted_ns.is_empty() {
        return 0.0;
    }
    let idx = ((sorted_ns.len() as f64 - 1.0) * p).round() as usize;
    sorted_ns[idx.min(sorted_ns.len() - 1)] as f64 / 1000.0
}

fn summarize(op: &str, samples_ns: Vec<u128>, rows: usize, note: &str) -> Stats {
    let mut s = samples_ns.clone();
    s.sort_unstable();
    Stats {
        op: op.to_string(),
        iters: s.len(),
        p50_us: percentile_us(&s, 0.50),
        p95_us: percentile_us(&s, 0.95),
        p99_us: percentile_us(&s, 0.99),
        min_us: *s.first().unwrap_or(&0) as f64 / 1000.0,
        max_us: *s.last().unwrap_or(&0) as f64 / 1000.0,
        rows,
        note: note.to_string(),
        failed: false,
    }
}

fn err_stat(op: &str, note: &str, err: &DbError) -> Stats {
    let truncated = {
        let s = err.to_string();
        if s.len() > 120 { format!("{}…", &s[..120]) } else { s }
    };
    Stats {
        op: op.to_string(),
        iters: 0,
        p50_us: 0.0,
        p95_us: 0.0,
        p99_us: 0.0,
        min_us: 0.0,
        max_us: 0.0,
        rows: 0,
        note: format!("{} — ERROR: {}", note, truncated),
        failed: true,
    }
}

/// Mide `iters` iteraciones de `f`. Captura errores: si la primera iteración
/// falla, devuelve un Stats marcado como ERROR. Si falla a mitad, registra
/// nota y devuelve los samples válidos. NUNCA propaga el error.
fn bench_safe<F>(op: &str, pager: &mut Pager, iters: usize, note: &str, mut f: F) -> Stats
where
    F: FnMut(&mut Engine, usize) -> DbResult<usize>,
{
    let mut samples = Vec::with_capacity(iters);
    let mut last_rows = 0;
    let mut failure: Option<DbError> = None;
    let mut failed_at: usize = usize::MAX;
    {
        let mut engine = Engine::new(pager);
        for i in 0..iters {
            let t0 = Instant::now();
            match f(&mut engine, i) {
                Ok(rows) => {
                    last_rows = rows;
                    let elapsed = t0.elapsed().as_nanos();
                    if i >= WARMUP_DISCARD {
                        samples.push(elapsed);
                    }
                }
                Err(e) => {
                    failure = Some(e);
                    failed_at = i;
                    break;
                }
            }
        }
    }
    match (failure, samples.is_empty()) {
        (Some(e), true) => err_stat(op, note, &e),
        (Some(e), false) => {
            let mut s = summarize(op, samples, last_rows, note);
            s.note = format!("{} — PARCIAL: falló en iter {} ({})", note, failed_at, e);
            s.failed = true;
            s
        }
        (None, _) => summarize(op, samples, last_rows, note),
    }
}

// ---------------------------------------------------------------------------
// Resultado de load
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct LoadStat {
    label: String,
    rows: usize,
    elapsed_s: f64,
    throughput: f64, // rows/sec
}

// ---------------------------------------------------------------------------
// Helpers de exec
// ---------------------------------------------------------------------------

fn exec_sql_rows(engine: &mut Engine, sql: &str) -> DbResult<usize> {
    let stmts = parse(sql)?;
    let mut total = 0;
    for s in stmts {
        total += engine.exec(s)?.rows.len();
    }
    Ok(total)
}

fn exec_batch(pager: &mut Pager, sqls: &[&str]) -> DbResult<()> {
    pager.begin()?;
    let r = (|| -> DbResult<()> {
        let mut engine = Engine::new(pager);
        for sql in sqls {
            for stmt in parse(sql)? {
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
    Ok(())
}

/// Best-effort: no propaga errores, devuelve true si OK.
fn try_exec_batch(pager: &mut Pager, sqls: &[&str]) -> bool {
    exec_batch(pager, sqls).is_ok()
}

/// Bulk load — N inserts por commit para mantener el WAL acotado.
fn bulk_load<F>(
    pager: &mut Pager,
    total: usize,
    batch: usize,
    mut gen: F,
) -> DbResult<f64>
where
    F: FnMut(usize) -> String,
{
    let t0 = Instant::now();
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
    Ok(t0.elapsed().as_secs_f64())
}

// ---------------------------------------------------------------------------
// Escenario microblog
// ---------------------------------------------------------------------------

fn setup_microblog(pager: &mut Pager) -> DbResult<Vec<LoadStat>> {
    exec_batch(
        pager,
        &[
            "CREATE TABLE users (id INT PRIMARY KEY, nombre TEXT NOT NULL, email TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL)",
            "CREATE TABLE posts (id INT PRIMARY KEY, user_id INT NOT NULL REFERENCES users(id) ON DELETE CASCADE, titulo TEXT NOT NULL, body TEXT NOT NULL, created_at TEXT NOT NULL, likes INT NOT NULL DEFAULT 0)",
        ],
    )?;

    let nombres = ["Ana", "Luis", "Maria", "Pedro", "Juan", "Sofia", "Diego", "Lucia"];
    let mut rng = Xorshift64::new(SEED);
    let users_elapsed = bulk_load(pager, MICROBLOG_USERS, 500, |i| {
        let nombre = nombres[(rng.next_u64() as usize) % nombres.len()];
        format!(
            "INSERT INTO users (id, nombre, email, created_at) VALUES ({}, '{}', 'u{}@bench.dev', '2026-01-01 00:00:00')",
            i, nombre, i
        )
    })?;

    let mut rng2 = Xorshift64::new(SEED ^ 0xDEAD_BEEF);
    let posts_elapsed = bulk_load(pager, MICROBLOG_POSTS, 500, |i| {
        let r = rng2.next_u64() as f64 / u64::MAX as f64;
        let skewed = (r * r * MICROBLOG_USERS as f64) as i64;
        let user_id = skewed.min(MICROBLOG_USERS as i64 - 1);
        let likes = (rng2.next_u64() % 200) as i64;
        format!(
            "INSERT INTO posts (id, user_id, titulo, body, created_at, likes) VALUES ({}, {}, 'titulo {}', 'body del post {}', '2026-02-01 12:00:00', {})",
            i, user_id, i, i, likes
        )
    })?;

    // No creamos el índice acá — lo crea queries_microblog para poder medir
    // Q2 sin idx primero (defensivo) y luego con idx.

    Ok(vec![
        LoadStat {
            label: "INSERT users".into(),
            rows: MICROBLOG_USERS,
            elapsed_s: users_elapsed,
            throughput: MICROBLOG_USERS as f64 / users_elapsed,
        },
        LoadStat {
            label: "INSERT posts".into(),
            rows: MICROBLOG_POSTS,
            elapsed_s: posts_elapsed,
            throughput: MICROBLOG_POSTS as f64 / posts_elapsed,
        },
    ])
}

fn queries_microblog(pager: &mut Pager, iters: usize) -> DbResult<Vec<Stats>> {
    let mut out = Vec::new();

    // Asegurar estado: drop del idx por si existe de una corrida previa
    let _ = exec_batch(pager, &["DROP INDEX idx_posts_user"]);

    pager.begin()?;

    // Q1 — PK lookup random
    out.push(bench_safe("Q1 PK lookup (users.id)", pager, iters, "1000 ids dispersos", |engine, i| {
        let mut rng = Xorshift64::new(SEED ^ i as u64 ^ 0xA1);
        let id = (rng.next_u64() % MICROBLOG_USERS as u64) as i64;
        exec_sql_rows(engine, &format!("SELECT * FROM users WHERE id = {}", id))
    }));

    // Q2a — sin índice (esperamos GBY-4001 → N/A)
    out.push(bench_safe("Q2a Indexed eq posts.user_id (sin idx)", pager, iters / 5, "pre CREATE INDEX", |engine, i| {
        let mut rng = Xorshift64::new(SEED ^ i as u64 ^ 0xB2);
        let uid = (rng.next_u64() % MICROBLOG_USERS as u64) as i64;
        exec_sql_rows(engine, &format!("SELECT * FROM posts WHERE user_id = {}", uid))
    }));

    let _ = pager.commit();

    // Crear índice
    if !try_exec_batch(pager, &["CREATE INDEX idx_posts_user ON posts(user_id)"]) {
        // Si falla, las queries siguientes lo verán
        eprintln!("warn: CREATE INDEX idx_posts_user falló");
    }
    pager.begin()?;

    // Q2b — con índice
    out.push(bench_safe("Q2b Indexed eq posts.user_id (con idx)", pager, iters, "idx_posts_user activo", |engine, i| {
        let mut rng = Xorshift64::new(SEED ^ i as u64 ^ 0xB2);
        let uid = (rng.next_u64() % MICROBLOG_USERS as u64) as i64;
        exec_sql_rows(engine, &format!("SELECT * FROM posts WHERE user_id = {}", uid))
    }));

    // Q3 — range scan PK
    out.push(bench_safe("Q3 Range scan PK (posts.id BETWEEN)", pager, iters, "rango 100", |engine, i| {
        let mut rng = Xorshift64::new(SEED ^ i as u64 ^ 0xC3);
        let a = (rng.next_u64() % (MICROBLOG_POSTS as u64 - 100)) as i64;
        exec_sql_rows(engine, &format!("SELECT * FROM posts WHERE id BETWEEN {} AND {}", a, a + 99))
    }));

    // Q4 — JOIN index-loop
    out.push(bench_safe("Q4 JOIN posts×users (BETWEEN 1..100)", pager, iters, "join via idx PK", |engine, _| {
        exec_sql_rows(engine,
            "SELECT u.nombre, p.titulo FROM posts p JOIN users u ON p.user_id = u.id WHERE p.id BETWEEN 1 AND 100")
    }));

    // Q5 — Aggregate (likes no tiene idx → puede fallar 4001)
    out.push(bench_safe("Q5 Aggregate COUNT(*) WHERE likes>50", pager, iters / 2, "full scan posts", |engine, _| {
        exec_sql_rows(engine, "SELECT COUNT(*) FROM posts WHERE likes > 50")
    }));

    let _ = pager.commit();

    // Q6 — UPDATE (necesita commit por iter para reflejar costo real)
    let upd_iters = iters;
    let mut samples = Vec::with_capacity(upd_iters);
    let mut upd_fail: Option<DbError> = None;
    let mut upd_fail_at: usize = usize::MAX;
    for i in 0..upd_iters {
        let mut rng = Xorshift64::new(SEED ^ i as u64 ^ 0xD4);
        let id = (rng.next_u64() % MICROBLOG_POSTS as u64) as i64;
        if pager.begin().is_err() {
            upd_fail = Some(DbError::new("begin failed"));
            upd_fail_at = i;
            break;
        }
        let t1 = Instant::now();
        let r = (|| -> DbResult<()> {
            let mut engine = Engine::new(pager);
            for stmt in parse(&format!(
                "UPDATE posts SET likes = likes + 1 WHERE id = {}",
                id
            ))? {
                engine.exec(stmt)?;
            }
            Ok(())
        })();
        let dur = t1.elapsed().as_nanos();
        match r {
            Ok(()) => {
                if pager.commit().is_err() {
                    upd_fail = Some(DbError::new("commit failed"));
                    upd_fail_at = i;
                    break;
                }
                if i >= WARMUP_DISCARD {
                    samples.push(dur);
                }
            }
            Err(e) => {
                let _ = pager.rollback();
                upd_fail = Some(e);
                upd_fail_at = i;
                break;
            }
        }
    }
    let upd_note = "tx por iter (incluye fsync)";
    let upd_stat = match (upd_fail, samples.is_empty()) {
        (Some(e), true) => err_stat("Q6 UPDATE posts.likes (auto-commit)", upd_note, &e),
        (Some(e), false) => {
            let mut s = summarize("Q6 UPDATE posts.likes (auto-commit)", samples, 1, upd_note);
            s.note = format!("{} — PARCIAL: falló en iter {} ({})", upd_note, upd_fail_at, e);
            s.failed = true;
            s
        }
        (None, _) => summarize("Q6 UPDATE posts.likes (auto-commit)", samples, 1, upd_note),
    };
    out.push(upd_stat);

    Ok(out)
}

// ---------------------------------------------------------------------------
// Escenario events
// ---------------------------------------------------------------------------

fn setup_events(pager: &mut Pager) -> DbResult<Vec<LoadStat>> {
    exec_batch(
        pager,
        &[
            "CREATE TABLE events (id INT PRIMARY KEY, event_type TEXT NOT NULL, user_id INT NOT NULL, payload TEXT NOT NULL, ts TEXT NOT NULL, latency_ms INT NOT NULL)",
        ],
    )?;

    fn pick(r: u64) -> &'static str {
        match r % 100 {
            0..=49 => "login",
            50..=69 => "view",
            70..=84 => "click",
            85..=92 => "logout",
            93..=96 => "error",
            97..=98 => "purchase",
            99 => "admin_action",
            _ => "signup",
        }
    }

    let mut rng = Xorshift64::new(SEED ^ 0xEE11_2233);
    let elapsed = bulk_load(pager, EVENTS_ROWS, 1000, |i| {
        let r = rng.next_u64();
        let etype = pick(r);
        let uid = (rng.next_u64() % 5000) as i64;
        let lat = (rng.next_u64() % 3000) as i64;
        // pad_len acotado: con 200K rows el idx_events_type empuja entradas
        // grandes a páginas individuales y supera page size. 0..149 es seguro.
        let pad_len = (rng.next_u64() % 150) as usize;
        let payload = "x".repeat(pad_len);
        format!(
            "INSERT INTO events (id, event_type, user_id, payload, ts, latency_ms) VALUES ({}, '{}', {}, '{}', '2026-03-01 00:00:00', {})",
            i, etype, uid, payload, lat
        )
    })?;

    // Idx solo en event_type — latency_ms y LENGTH(payload) intencionalmente no
    // indexados para medir el rechazo defensivo de gabysql (GBY-4001).
    // Best-effort: con 200K rows + alta cardinalidad de payloads el b+tree puede
    // rechazar entradas grandes (límite de página); si falla, Q3/Q4 caerán a N/A
    // automáticamente via bench_safe.
    let _ = try_exec_batch(pager, &["CREATE INDEX idx_events_type ON events(event_type)"]);

    Ok(vec![LoadStat {
        label: "INSERT events".into(),
        rows: EVENTS_ROWS,
        elapsed_s: elapsed,
        throughput: EVENTS_ROWS as f64 / elapsed,
    }])
}

fn queries_events(pager: &mut Pager, iters: usize) -> DbResult<Vec<Stats>> {
    let mut out = Vec::new();
    pager.begin()?;

    let heavy_iters = (iters / 10).max(10);
    let mid_iters = (iters / 5).max(20);

    // Q1 — WHERE latency_ms > 1000 (no indexada → esperado 4001)
    out.push(bench_safe("Q1 Full scan COUNT WHERE latency>1000", pager, heavy_iters, "scan 200K (latency_ms no idx)", |engine, _| {
        exec_sql_rows(engine, "SELECT COUNT(*) FROM events WHERE latency_ms > 1000")
    }));

    out.push(bench_safe("Q2 GROUP BY event_type", pager, heavy_iters, "agg + hash group", |engine, _| {
        exec_sql_rows(engine, "SELECT event_type, COUNT(*), AVG(latency_ms) FROM events GROUP BY event_type")
    }));

    out.push(bench_safe("Q3 Indexed lookup type='login' LIMIT 100", pager, mid_iters, "tipo frecuente ~50%", |engine, _| {
        exec_sql_rows(engine, "SELECT * FROM events WHERE event_type = 'login' LIMIT 100")
    }));

    out.push(bench_safe("Q4 Indexed lookup type='admin_action' LIMIT 100", pager, mid_iters, "tipo raro ~1%", |engine, _| {
        exec_sql_rows(engine, "SELECT * FROM events WHERE event_type = 'admin_action' LIMIT 100")
    }));

    // Q5 — LENGTH(payload) > 100 (scalar en WHERE no indexable)
    out.push(bench_safe("Q5 Scalar func WHERE LENGTH(payload)>100", pager, heavy_iters, "G2 scalar in WHERE", |engine, _| {
        exec_sql_rows(engine, "SELECT COUNT(*) FROM events WHERE LENGTH(payload) > 100")
    }));

    out.push(bench_safe("Q6 INTERSECT (latency>500 ∩ user_id<100)", pager, heavy_iters, "I set op", |engine, _| {
        exec_sql_rows(engine,
            "SELECT event_type FROM events WHERE latency_ms > 500 INTERSECT SELECT event_type FROM events WHERE user_id < 100")
    }));

    out.push(bench_safe("Q7 Scalar subquery in SELECT LIMIT 10", pager, mid_iters, "H scalar subquery", |engine, _| {
        exec_sql_rows(engine, "SELECT id, (SELECT COUNT(*) FROM events) AS total FROM events LIMIT 10")
    }));

    out.push(bench_safe("Q8 Derived table GROUP BY filter cnt>1000", pager, heavy_iters, "H derived table", |engine, _| {
        exec_sql_rows(engine,
            "SELECT t.event_type, t.cnt FROM (SELECT event_type, COUNT(*) AS cnt FROM events GROUP BY event_type) AS t WHERE t.cnt > 1000")
    }));

    let _ = pager.commit();
    Ok(out)
}

// ---------------------------------------------------------------------------
// Escenario catalog (K2 — PK + índices compuestos)
// ---------------------------------------------------------------------------

fn setup_catalog(pager: &mut Pager) -> DbResult<Vec<LoadStat>> {
    exec_batch(
        pager,
        &[
            "CREATE TABLE orders (id INT PRIMARY KEY, customer_id INT NOT NULL, total INT NOT NULL, created_at TEXT NOT NULL)",
            "CREATE TABLE order_lines (order_id INT NOT NULL, line_no INT NOT NULL, sku TEXT NOT NULL, qty INT NOT NULL, price INT NOT NULL, PRIMARY KEY (order_id, line_no))",
        ],
    )?;

    let mut rng = Xorshift64::new(SEED ^ 0xCAFE_1111);
    let orders_elapsed = bulk_load(pager, ORDERS_ROWS, 500, |i| {
        let c = rng.range(1, 2000) as i64;
        let t = rng.range(100, 100_000) as i64;
        format!(
            "INSERT INTO orders (id, customer_id, total, created_at) VALUES ({}, {}, {}, '2026-04-01 09:00:00')",
            i, c, t
        )
    })?;

    let mut rng2 = Xorshift64::new(SEED ^ 0xDADA_2222);
    let lines_elapsed = bulk_load(pager, LINES_ROWS, 1000, |i| {
        let order_id = (i / 10) as i64;
        let line_no = (i % 10) as i64;
        let qty = (rng2.next_u64() % 20 + 1) as i64;
        let price = (rng2.next_u64() % 5000 + 10) as i64;
        let sku = rng2.next_u64() % 1000;
        format!(
            "INSERT INTO order_lines (order_id, line_no, sku, qty, price) VALUES ({}, {}, 'SKU-{:04}', {}, {})",
            order_id, line_no, sku, qty, price
        )
    })?;

    let _ = try_exec_batch(pager, &["CREATE INDEX idx_lines_order_sku ON order_lines (order_id, line_no)"]);

    Ok(vec![
        LoadStat {
            label: "INSERT orders".into(),
            rows: ORDERS_ROWS,
            elapsed_s: orders_elapsed,
            throughput: ORDERS_ROWS as f64 / orders_elapsed,
        },
        LoadStat {
            label: "INSERT order_lines (PK compuesta)".into(),
            rows: LINES_ROWS,
            elapsed_s: lines_elapsed,
            throughput: LINES_ROWS as f64 / lines_elapsed,
        },
    ])
}

fn queries_catalog(pager: &mut Pager, iters: usize) -> DbResult<Vec<Stats>> {
    let mut out = Vec::new();
    pager.begin()?;

    out.push(bench_safe("Q1 Composite PK full (order_id AND line_no)", pager, iters, "lookup exacto", |engine, i| {
        let mut rng = Xorshift64::new(SEED ^ i as u64 ^ 0xE1);
        let oid = (rng.next_u64() % ORDERS_ROWS as u64) as i64;
        let ln = (rng.next_u64() % 10) as i64;
        exec_sql_rows(engine, &format!("SELECT * FROM order_lines WHERE order_id = {} AND line_no = {}", oid, ln))
    }));

    out.push(bench_safe("Q2 Composite PK partial (order_id only)", pager, iters / 5, "fallback a scan", |engine, i| {
        let mut rng = Xorshift64::new(SEED ^ i as u64 ^ 0xE2);
        let oid = (rng.next_u64() % ORDERS_ROWS as u64) as i64;
        exec_sql_rows(engine, &format!("SELECT * FROM order_lines WHERE order_id = {}", oid))
    }));

    out.push(bench_safe("Q3 JOIN orders×order_lines (id BETWEEN 1..10)", pager, iters, "join + composite", |engine, _| {
        exec_sql_rows(engine,
            "SELECT o.id, l.sku, l.qty FROM orders o JOIN order_lines l ON o.id = l.order_id WHERE o.id BETWEEN 1 AND 10")
    }));

    out.push(bench_safe("Q4 Aggregate SUM(qty*price) GROUP LIMIT 100", pager, (iters / 10).max(5), "agg 100K rows", |engine, _| {
        exec_sql_rows(engine, "SELECT order_id, SUM(qty * price) AS total FROM order_lines GROUP BY order_id LIMIT 100")
    }));

    let _ = pager.commit();
    Ok(out)
}

// ---------------------------------------------------------------------------
// Reporte Markdown
// ---------------------------------------------------------------------------

fn fmt_us_opt(v: f64, failed: bool) -> String {
    if failed {
        "—".to_string()
    } else if v < 1000.0 {
        format!("{:.2}", v)
    } else if v < 1_000_000.0 {
        format!("{:.1}", v)
    } else {
        format!("{:.0}", v)
    }
}

fn render_stats_table(rows: &[Stats]) -> String {
    let mut s = String::new();
    s.push_str("| Operación | N | P50 (µs) | P95 (µs) | P99 (µs) | Min (µs) | Max (µs) | Filas | Notas |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---|\n");
    for r in rows {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.op,
            r.iters,
            fmt_us_opt(r.p50_us, r.failed),
            fmt_us_opt(r.p95_us, r.failed),
            fmt_us_opt(r.p99_us, r.failed),
            fmt_us_opt(r.min_us, r.failed),
            fmt_us_opt(r.max_us, r.failed),
            r.rows,
            r.note,
        ));
    }
    s
}

fn render_load_table(loads: &[LoadStat]) -> String {
    let mut s = String::new();
    s.push_str("| Operación | Filas | Tiempo (s) | Throughput (rows/s) |\n");
    s.push_str("|---|---:|---:|---:|\n");
    for l in loads {
        s.push_str(&format!(
            "| {} | {} | {:.3} | {:.0} |\n",
            l.label, l.rows, l.elapsed_s, l.throughput
        ));
    }
    s
}

// ---------------------------------------------------------------------------
// Metadata del bench (hardware, commit, toolchain, fecha)
// ---------------------------------------------------------------------------

fn capture_cmd(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "desconocido".to_string())
}

fn render_metadata() -> String {
    let now = capture_cmd("powershell", &["-NoProfile", "-Command", "Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz'"]);
    let commit = capture_cmd("git", &["rev-parse", "--short", "HEAD"]);
    let branch = capture_cmd("git", &["rev-parse", "--abbrev-ref", "HEAD"]);
    let rustc = capture_cmd("rustc", &["--version"]);
    let os = capture_cmd("powershell", &["-NoProfile", "-Command", "(Get-CimInstance Win32_OperatingSystem).Caption + ' ' + (Get-CimInstance Win32_OperatingSystem).Version"]);
    let cpu = capture_cmd("powershell", &["-NoProfile", "-Command", "(Get-CimInstance Win32_Processor).Name"]);
    let cores = capture_cmd("powershell", &["-NoProfile", "-Command", "(Get-CimInstance Win32_Processor).NumberOfLogicalProcessors"]);
    let ram_gb = capture_cmd("powershell", &["-NoProfile", "-Command", "[math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1)"]);

    let mut s = String::new();
    s.push_str("# Benchmark gabysql\n\n");
    s.push_str("## Metadata\n\n");
    s.push_str(&format!("- **Fecha**: {}\n", now));
    s.push_str(&format!("- **Commit**: `{}` (branch `{}`)\n", commit, branch));
    s.push_str(&format!("- **Toolchain**: {}\n", rustc));
    s.push_str(&format!("- **OS**: {}\n", os));
    s.push_str(&format!("- **CPU**: {} ({} threads lógicos)\n", cpu, cores));
    s.push_str(&format!("- **RAM**: {} GB\n", ram_gb));
    s.push_str("- **Build profile**: release (LTO según Cargo.toml)\n");
    s.push_str(&format!("- **Warmup descartado**: {} iters por operación\n", WARMUP_DISCARD));
    s.push('\n');
    s
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {}", e);
            print_usage();
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&args) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

fn run(args: &Args) -> DbResult<()> {
    println!("=== gabysql-bench ===");
    println!(
        "scenario={}  phase={}  iters={}  db={}",
        args.scenario,
        args.phase,
        args.iters,
        args.db.display()
    );

    if let Some(parent) = args.db.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let do_load = matches!(args.phase.as_str(), "load" | "all");
    let do_query = matches!(args.phase.as_str(), "query" | "all");

    let mut load_stats: Vec<LoadStat> = Vec::new();
    let mut query_stats: Vec<Stats> = Vec::new();

    if do_load {
        let mut pager = Pager::create_force(&args.db)?;
        let t0 = Instant::now();
        load_stats = match args.scenario.as_str() {
            "microblog" => setup_microblog(&mut pager)?,
            "events" => setup_events(&mut pager)?,
            "catalog" => setup_catalog(&mut pager)?,
            _ => unreachable!(),
        };
        let dt = t0.elapsed().as_secs_f64();
        pager.close()?;

        println!("\n--- Fase LOAD ({:.2}s total) ---", dt);
        for l in &load_stats {
            println!(
                "  {:<40} rows={:>7}  t={:>6.2}s  thr={:>9.0} rows/s",
                l.label, l.rows, l.elapsed_s, l.throughput
            );
        }
    }

    if do_query {
        let mut pager = Pager::open(&args.db)?;
        let t0 = Instant::now();
        // queries_* ahora NUNCA propaga errores de queries individuales
        query_stats = match args.scenario.as_str() {
            "microblog" => queries_microblog(&mut pager, args.iters)?,
            "events" => queries_events(&mut pager, args.iters)?,
            "catalog" => queries_catalog(&mut pager, args.iters)?,
            _ => unreachable!(),
        };
        let dt = t0.elapsed().as_secs_f64();
        let _ = pager.rollback();
        pager.close()?;

        println!("\n--- Fase QUERY ({:.2}s total) ---", dt);
        println!("{}", render_stats_table(&query_stats));
    }

    if let Some(out_path) = &args.out {
        let is_first = !out_path.exists();
        let mut md = String::new();
        if is_first {
            md.push_str(&render_metadata());
        }
        md.push_str(&format!("\n## Escenario: {}\n\n", args.scenario));
        md.push_str(&format!("- DB: `{}`\n", args.db.display()));
        md.push_str(&format!("- iters: {} (warmup descartado: {})\n", args.iters, WARMUP_DISCARD));
        md.push_str(&format!("- phase: {}\n\n", args.phase));
        if !load_stats.is_empty() {
            md.push_str("### Carga\n\n");
            md.push_str(&render_load_table(&load_stats));
            md.push('\n');
        }
        if !query_stats.is_empty() {
            md.push_str("### Queries\n\n");
            md.push_str(&render_stats_table(&query_stats));
            // Resumen de fallos del escenario
            let failures: Vec<&Stats> = query_stats.iter().filter(|s| s.failed).collect();
            if !failures.is_empty() {
                md.push_str("\n**Queries con problema (N/A o ERROR):**\n\n");
                for f in failures {
                    md.push_str(&format!("- `{}` → {}\n", f.op, f.note));
                }
            }
        }
        append_or_create(out_path, &md)?;
        println!("\nReporte {} en: {}", if is_first { "creado" } else { "appendeado" }, out_path.display());
    }

    Ok(())
}

fn append_or_create(path: &Path, content: &str) -> DbResult<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(content.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}
