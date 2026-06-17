//! Bloque M4 (2026-06-15): stress test del parser SQL.
//!
//! Propósito: detectar panics, unwraps fallidos, loops infinitos en el
//! parser ante inputs random. Es la línea "1 hora limpia de fuzz" del
//! README — `cargo fuzz` real necesita libFuzzer (Linux + nightly), que
//! NO está disponible en el entorno típico del autor (Windows + GNU sin
//! MSVC). Solución pragmática: generador hand-rolled determinístico +
//! `panic::catch_unwind` para atrapar fallas.
//!
//! Cero deps externas (alinea ADR-0001).
//!
//! ## Uso
//!
//! Marcado `#[ignore]` — no corre en CI per-commit (correr 60s × 800
//! tests es absurdo). Para verificar manualmente:
//!
//! ```bash
//! # Default 60 segundos (sanity check)
//! cargo test --target x86_64-pc-windows-gnu --test fuzz_parser -- \
//!     --ignored --nocapture
//!
//! # 1 hora limpia (evidencia para README)
//! GABYSQL_FUZZ_PARSER_SECS=3600 cargo test --target x86_64-pc-windows-gnu \
//!     --test fuzz_parser -- --ignored --nocapture
//! ```
//!
//! Cada panic encontrado se imprime con su seed reproducible. Re-correr
//! con `GABYSQL_FUZZ_PARSER_SEED=0x...` (no implementado todavía) sería
//! la próxima mejora.

use std::panic;
use std::time::{Duration, Instant};

use gabysql::sql::parse;

// ---------------------------------------------------------------------------
// LCG determinístico — mismas constantes que gabybench / proptest_planner.
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
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u64() as usize) % items.len()]
    }
}

// ---------------------------------------------------------------------------
// Vocabulario SQL — keywords, operadores, puntuación.
// ---------------------------------------------------------------------------

const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "JOIN",
    "ON",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "TRUE",
    "FALSE",
    "GROUP",
    "BY",
    "HAVING",
    "ORDER",
    "LIMIT",
    "OFFSET",
    "AS",
    "DISTINCT",
    "CREATE",
    "TABLE",
    "DROP",
    "ALTER",
    "ADD",
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "INDEX",
    "UNIQUE",
    "CONSTRAINT",
    "CHECK",
    "DEFAULT",
    "INT",
    "TEXT",
    "FLOAT",
    "BOOL",
    "DATE",
    "TIME",
    "DATETIME",
    "DECIMAL",
    "VARCHAR",
    "BLOB",
    "UUID",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "EXPLAIN",
    "ANALYZE",
    "IN",
    "BETWEEN",
    "LIKE",
    "IS",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "USING",
    "NATURAL",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "WITH",
    "RECURSIVE",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "CAST",
    "ASC",
    "DESC",
    "EXISTS",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "RETURNING",
    "ON",
    "CONFLICT",
    "DO",
    "NOTHING",
    "ROW",
    "CTE",
    "VIEW",
];

const OPERATORS: &[&str] = &[
    "=", "<>", "!=", "<", ">", "<=", ">=", "+", "-", "*", "/", "%", "||",
];

const PUNCT: &[&str] = &[",", ";", "(", ")", ".", "[", "]"];

// ---------------------------------------------------------------------------
// Generadores.
// ---------------------------------------------------------------------------

fn random_ident(rng: &mut Lcg) -> String {
    let len = rng.range(1, 12) as usize;
    let mut s = String::with_capacity(len);
    for _ in 0..len {
        let c = (b'a' + (rng.next_u64() % 26) as u8) as char;
        s.push(c);
    }
    s
}

fn random_literal(rng: &mut Lcg) -> String {
    match rng.range(0, 5) {
        0 => format!("{}", rng.next_u64() as i64),
        1 => format!("{}.{}", rng.range(0, 1000), rng.range(0, 1000)),
        2 => format!("'{}'", random_ident(rng)),
        3 => "NULL".to_string(),
        _ => format!("-{}", rng.range(0, 100_000)),
    }
}

fn random_token(rng: &mut Lcg) -> String {
    match rng.range(0, 100) {
        0..=40 => rng.pick(KEYWORDS).to_string(),
        41..=55 => random_ident(rng),
        56..=75 => random_literal(rng),
        76..=87 => rng.pick(OPERATORS).to_string(),
        _ => rng.pick(PUNCT).to_string(),
    }
}

fn generate_query(rng: &mut Lcg) -> String {
    let n_tokens = rng.range(2, 60) as usize;
    let mut q = String::with_capacity(n_tokens * 8);
    for i in 0..n_tokens {
        if i > 0 {
            q.push(' ');
        }
        q.push_str(&random_token(rng));
    }
    q
}

/// Mutador adversarial: toma un query base y le inyecta bytes random
/// en posiciones random. Ataca buffer parsing, decoding UTF-8, etc.
fn mutate_bytes(rng: &mut Lcg, q: &str) -> String {
    let mut bytes: Vec<u8> = q.bytes().collect();
    let n_mutations = rng.range(1, 5) as usize;
    for _ in 0..n_mutations {
        if bytes.is_empty() {
            break;
        }
        let pos = (rng.next_u64() as usize) % bytes.len();
        let action = rng.range(0, 4);
        match action {
            0 => {
                bytes[pos] = (rng.next_u64() & 0xFF) as u8;
            }
            1 => {
                bytes.insert(pos, (rng.next_u64() & 0xFF) as u8);
            }
            _ => {
                bytes.remove(pos);
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// El test.
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn m4_fuzz_parser_no_panic() {
    let duration_secs: u64 = std::env::var("GABYSQL_FUZZ_PARSER_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let duration = Duration::from_secs(duration_secs);
    println!(
        "== M4 fuzz_parser_no_panic: corriendo {}s ==",
        duration_secs
    );

    // Suprimir el output de panics durante el run — los capturamos en
    // catch_unwind. Sin esto, cada panic mostraría su stack trace en
    // stderr y el output de la corrida de 1h sería incontrolable.
    panic::set_hook(Box::new(|_| {}));

    let mut rng = Lcg::new(0x9E37_79B9_7F4A_7C15);
    let start = Instant::now();
    let mut iters = 0u64;
    let mut parse_ok = 0u64;
    let mut parse_err = 0u64;
    let mut panics: Vec<(u64, String)> = Vec::new();
    let mut last_progress = Instant::now();

    while start.elapsed() < duration {
        let seed = rng.next_u64();
        let mut local = Lcg::new(seed);

        // 70% queries pseudo-estructuradas, 30% bytes mutados (adversarial).
        let q = if local.range(0, 10) < 7 {
            generate_query(&mut local)
        } else {
            let base = generate_query(&mut local);
            mutate_bytes(&mut local, &base)
        };

        let q_clone = q.clone();
        let result = panic::catch_unwind(|| parse(&q_clone));
        match result {
            Ok(Ok(_)) => parse_ok += 1,
            Ok(Err(_)) => parse_err += 1,
            Err(_) => {
                panics.push((seed, q.clone()));
                if panics.len() >= 20 {
                    println!("Cortando: 20 panics distintos encontrados.");
                    break;
                }
            }
        }
        iters += 1;

        // Progress cada 5 segundos para que el operador sepa que está vivo.
        if last_progress.elapsed() >= Duration::from_secs(5) {
            println!(
                "[{:5}s] iters={} parse_ok={} parse_err={} panics={}",
                start.elapsed().as_secs(),
                iters,
                parse_ok,
                parse_err,
                panics.len()
            );
            last_progress = Instant::now();
        }
    }

    let _ = panic::take_hook();

    println!("== resultado final ==");
    println!("duración:      {} s", start.elapsed().as_secs());
    println!("iters totales: {}", iters);
    println!(
        "iters/seg:     {}",
        iters / start.elapsed().as_secs().max(1)
    );
    println!("parse OK:      {}", parse_ok);
    println!("parse error:   {} (esperado — input random)", parse_err);
    println!("PANICS:        {}", panics.len());
    for (seed, q) in &panics {
        let preview = if q.len() > 200 { &q[..200] } else { q.as_str() };
        println!("  seed=0x{:016x} query: {}", seed, preview);
    }

    assert!(
        panics.is_empty(),
        "{} panic(s) detectado(s) en parser durante {} s — ver salida arriba",
        panics.len(),
        duration_secs
    );
}
