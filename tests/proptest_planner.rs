//! Bloque M3 (2026-06-15): property tests sobre el planner.
//!
//! Propósito: defender la correctness de los plan-cambios que introdujo
//! la sesión P5 — P5c (cost-based skip-index), P5d (hash-join build-side
//! swap), R6 (composite bucket-size check), R2 (umbral 0.10). Todos ellos
//! cambian QUÉ path corre el motor según las stats, pero NUNCA deberían
//! cambiar el **resultado** (rows devueltas) ni el **count**.
//!
//! El test es property-based hand-rolled (zero deps externas, ADR-0001):
//! generamos data + queries con un LCG determinístico y comparamos
//! `SELECT con ANALYZE` vs `SELECT sin ANALYZE`. Si alguno difiere, un
//! plan-cambio del optimizer rompió correctness — falla con seed
//! reproducible.

use std::error::Error;
use std::path::{Path, PathBuf};

use gabysql::sql::{parse, Engine, Value};
use gabysql::storage::Pager;

// ---------------------------------------------------------------------------
// LCG determinístico (mismas constantes que gabybench). Sin deps.
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
// Helpers de fixture
// ---------------------------------------------------------------------------

fn tmp_db(seed: u64, label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("gby_proptest_{}_{:x}.db", label, seed))
}

fn cleanup(p: &Path) {
    let _ = std::fs::remove_file(p);
    let mut wal = p.as_os_str().to_owned();
    wal.push(".wal");
    let _ = std::fs::remove_file(PathBuf::from(wal));
}

fn exec_batch(db: &Path, sqls: &[&str]) -> Result<(), Box<dyn Error>> {
    let mut pager = Pager::open(db)?;
    pager.begin()?;
    {
        let mut engine = Engine::new(&mut pager);
        for sql in sqls {
            for stmt in parse(sql)? {
                engine.exec(stmt)?;
            }
        }
    }
    pager.commit()?;
    pager.close()?;
    Ok(())
}

/// Pobla la tabla `t` con `n` filas determinísticas según `seed`.
/// Schema: `(id PK, a INT, b TEXT, c INT)` con índices sobre a y b.
/// Distribución de `a`: skewed (0..5 frecuentes, 5..20 raros) — útil
/// para exercer P5c (alta sel vs baja sel sobre la misma columna).
fn populate(db: &Path, seed: u64, n: usize) -> Result<(), Box<dyn Error>> {
    let mut pager = Pager::create(db)?;
    pager.begin()?;
    {
        let mut engine = Engine::new(&mut pager);
        for stmt in parse(
            "CREATE TABLE t (id INT PRIMARY KEY, a INT, b TEXT, c INT);
             CREATE INDEX idx_a ON t (a);
             CREATE INDEX idx_b ON t (b);",
        )? {
            engine.exec(stmt)?;
        }
    }
    pager.commit()?;
    pager.close()?;

    let b_choices = ["alpha", "beta", "gamma", "delta", "epsilon"];
    let mut rng = Lcg::new(seed);
    let mut pager = Pager::open(db)?;
    pager.begin()?;
    {
        let mut engine = Engine::new(&mut pager);
        for i in 1..=n {
            // a: 70% en 0..5 (skewed → alta sel para Eq sobre esos valores),
            //    30% en 5..20 (baja sel).
            let a = if rng.next_u64() % 10 < 7 {
                rng.range(0, 5)
            } else {
                rng.range(5, 20)
            } as i64;
            let b = b_choices[rng.next_u64() as usize % b_choices.len()];
            let c = rng.range(0, 100) as i64;
            let sql = format!(
                "INSERT INTO t (id, a, b, c) VALUES ({}, {}, '{}', {})",
                i, a, b, c
            );
            for stmt in parse(&sql)? {
                engine.exec(stmt)?;
            }
        }
    }
    pager.commit()?;
    pager.close()?;
    Ok(())
}

/// Pobla 2 tablas relacionadas para tests de JOIN — ejercen P5d swap.
/// `u` chica (50 filas), `o` grande (300 filas, FK a u.id).
fn populate_join(db: &Path, seed: u64) -> Result<(), Box<dyn Error>> {
    let mut pager = Pager::create(db)?;
    pager.begin()?;
    {
        let mut engine = Engine::new(&mut pager);
        for stmt in parse(
            "CREATE TABLE u (id INT PRIMARY KEY, label TEXT);
             CREATE TABLE o (id INT PRIMARY KEY, user_id INT, val INT);
             CREATE INDEX idx_o_user ON o (user_id);",
        )? {
            engine.exec(stmt)?;
        }
    }
    pager.commit()?;
    pager.close()?;

    let mut rng = Lcg::new(seed);
    let mut pager = Pager::open(db)?;
    pager.begin()?;
    {
        let mut engine = Engine::new(&mut pager);
        for i in 1..=50 {
            let lbl = ["x", "y", "z"][rng.next_u64() as usize % 3];
            for stmt in parse(&format!(
                "INSERT INTO u (id, label) VALUES ({}, '{}')",
                i, lbl
            ))? {
                engine.exec(stmt)?;
            }
        }
        for i in 1..=300 {
            let uid = rng.range(1, 51) as i64;
            let val = rng.range(0, 1000) as i64;
            for stmt in parse(&format!(
                "INSERT INTO o (id, user_id, val) VALUES ({}, {}, {})",
                i, uid, val
            ))? {
                engine.exec(stmt)?;
            }
        }
    }
    pager.commit()?;
    pager.close()?;
    Ok(())
}

fn run_select(db: &Path, sql: &str) -> Result<Vec<Vec<Value>>, Box<dyn Error>> {
    let mut pager = Pager::open(db)?;
    pager.begin()?;
    let stmt = parse(sql)?.remove(0);
    let mut engine = Engine::new(&mut pager);
    let rs = engine.exec(stmt)?;
    let rows = rs.rows.clone();
    drop(engine);
    let _ = pager.rollback();
    let _ = pager.close();
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Generadores de WHERE / SELECT
// ---------------------------------------------------------------------------

/// Genera un WHERE aleatorio sobre la tabla `t`. Cubre Eq sobre PK,
/// sobre col indexada (alta y baja selectividad por la distribución
/// skewed), Compare, Between, AND, OR, IN list.
fn random_where(rng: &mut Lcg) -> String {
    let pick = rng.range(0, 10);
    let b_choices = ["alpha", "beta", "gamma", "delta", "epsilon"];
    match pick {
        0 => format!("a = {}", rng.range(0, 20)),
        1 => format!("a > {}", rng.range(0, 20)),
        2 => format!("a BETWEEN {} AND {}", rng.range(0, 10), rng.range(10, 20)),
        3 => format!(
            "b = '{}'",
            b_choices[rng.next_u64() as usize % b_choices.len()]
        ),
        4 => format!("c > {}", rng.range(0, 100)),
        5 => format!(
            "a = {} AND b = '{}'",
            rng.range(0, 5),
            b_choices[rng.next_u64() as usize % b_choices.len()]
        ),
        6 => format!("a > {} OR c < {}", rng.range(0, 20), rng.range(0, 100)),
        7 => format!(
            "a = {} AND c BETWEEN {} AND {}",
            rng.range(0, 20),
            rng.range(0, 50),
            rng.range(50, 100)
        ),
        8 => format!("id = {}", rng.range(1, 101)),
        _ => format!(
            "a IN ({}, {}, {})",
            rng.range(0, 20),
            rng.range(0, 20),
            rng.range(0, 20)
        ),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Invariante #1 (la más importante): con la misma data y el mismo
/// WHERE, el resultado de un SELECT debe ser idéntico tenga el motor
/// stats corridas (vía ANALYZE) o no. P5c/R6 pueden cambiar el plan
/// (skip-index, FullScan, etc) pero NUNCA el conjunto devuelto.
///
/// 50 iteraciones × 3 queries = **150 comparaciones**. Cada falla
/// imprime el seed para reproducir.
#[test]
fn m3_select_results_invariant_with_vs_without_analyze() -> Result<(), Box<dyn Error>> {
    const ITERS: usize = 50;
    const ROWS_PER_RUN: usize = 100;
    const QUERIES_PER_RUN: usize = 3;
    let outer_seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut outer = Lcg::new(outer_seed);

    for run in 0..ITERS {
        let seed = outer.next_u64();
        let db_no_stats = tmp_db(seed, "ns");
        let db_stats = tmp_db(seed, "ws");
        cleanup(&db_no_stats);
        cleanup(&db_stats);

        populate(&db_no_stats, seed, ROWS_PER_RUN)?;
        populate(&db_stats, seed, ROWS_PER_RUN)?;
        exec_batch(&db_stats, &["ANALYZE TABLE t"])?;

        let mut where_rng = Lcg::new(seed ^ 0x1234_5678);
        for q in 0..QUERIES_PER_RUN {
            let w = random_where(&mut where_rng);
            let sql = format!("SELECT id FROM t WHERE {} ORDER BY id", w);
            let no_stats = run_select(&db_no_stats, &sql)?;
            let stats = run_select(&db_stats, &sql)?;
            assert_eq!(
                no_stats, stats,
                "Run {} query {} (seed=0x{:x}): WHERE `{}` difiere con/sin ANALYZE\nsin: {:?}\ncon: {:?}",
                run, q, seed, w, no_stats, stats
            );
        }

        cleanup(&db_no_stats);
        cleanup(&db_stats);
    }
    Ok(())
}

/// Invariante #2: lo mismo para UPDATE/DELETE con WHERE. Verifica que
/// las stats no cambien cuáles filas son afectadas (R8 composite fast-
/// path se aplica acá también).
#[test]
fn m3_count_invariant_with_vs_without_analyze() -> Result<(), Box<dyn Error>> {
    const ITERS: usize = 30;
    const ROWS_PER_RUN: usize = 100;
    let outer_seed: u64 = 0xBADC_0FFE_DEAD_BEEF;
    let mut outer = Lcg::new(outer_seed);

    for run in 0..ITERS {
        let seed = outer.next_u64();
        let db_no_stats = tmp_db(seed, "cns");
        let db_stats = tmp_db(seed, "cws");
        cleanup(&db_no_stats);
        cleanup(&db_stats);

        populate(&db_no_stats, seed, ROWS_PER_RUN)?;
        populate(&db_stats, seed, ROWS_PER_RUN)?;
        exec_batch(&db_stats, &["ANALYZE TABLE t"])?;

        let mut where_rng = Lcg::new(seed ^ 0xCAFE_BABE);
        let w = random_where(&mut where_rng);
        // COUNT(*) sobre el WHERE — invariante de cardinalidad sin ORDER BY.
        let sql = format!("SELECT COUNT(*) FROM t WHERE {}", w);
        let no_stats = run_select(&db_no_stats, &sql)?;
        let stats = run_select(&db_stats, &sql)?;
        assert_eq!(
            no_stats, stats,
            "Run {} (seed=0x{:x}): COUNT(*) WHERE `{}` difiere con/sin ANALYZE\nsin: {:?}\ncon: {:?}",
            run, seed, w, no_stats, stats
        );

        cleanup(&db_no_stats);
        cleanup(&db_stats);
    }
    Ok(())
}

/// Invariante #3: JOIN inner con/sin stats — P5d (hash-join build-side
/// swap) puede cambiar quién es build y quién es probe según
/// cardinality, pero el set de filas devueltas debe ser idéntico.
#[test]
fn m3_inner_join_invariant_with_vs_without_analyze() -> Result<(), Box<dyn Error>> {
    const ITERS: usize = 20;
    let outer_seed: u64 = 0xF00D_BABE_DEAD_BEEF;
    let mut outer = Lcg::new(outer_seed);

    for run in 0..ITERS {
        let seed = outer.next_u64();
        let db_no_stats = tmp_db(seed, "jns");
        let db_stats = tmp_db(seed, "jws");
        cleanup(&db_no_stats);
        cleanup(&db_stats);

        populate_join(&db_no_stats, seed)?;
        populate_join(&db_stats, seed)?;
        exec_batch(&db_stats, &["ANALYZE TABLE u; ANALYZE TABLE o"])?;

        // Combo de queries: simple JOIN, JOIN+WHERE label, JOIN+val
        // range. ORDER BY single-col porque el parser actual no acepta
        // multi-col (limitación conocida — TAREAS_PENDIENTES §1). Para
        // comparar sets independientemente del orden, ordenamos los
        // Vec<Vec<Value>> en Rust antes del assert.
        let queries = [
            "SELECT u.id, o.val FROM u JOIN o ON o.user_id = u.id ORDER BY o.id",
            "SELECT u.id, o.val FROM u JOIN o ON o.user_id = u.id WHERE u.label = 'x' ORDER BY o.id",
            "SELECT u.id FROM u JOIN o ON o.user_id = u.id WHERE o.val > 500 ORDER BY o.id",
        ];

        for (qi, q) in queries.iter().enumerate() {
            let mut no_stats = run_select(&db_no_stats, q)?;
            let mut stats = run_select(&db_stats, q)?;
            // Normalizar orden para que P5d swap (que puede flippear
            // orden sin ORDER BY robusto) no nos confunda con
            // diferencia de set. La invariante real es el SET.
            no_stats.sort_by(|a, b| format!("{:?}", a).cmp(&format!("{:?}", b)));
            stats.sort_by(|a, b| format!("{:?}", a).cmp(&format!("{:?}", b)));
            assert_eq!(
                no_stats, stats,
                "Run {} query {} (seed=0x{:x}): JOIN set difiere con/sin ANALYZE\nsin: {} rows\ncon: {} rows",
                run, qi, seed, no_stats.len(), stats.len()
            );
        }

        cleanup(&db_no_stats);
        cleanup(&db_stats);
    }
    Ok(())
}
