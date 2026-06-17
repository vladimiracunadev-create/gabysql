//! Property tests sobre el Pager / capa de transacciones.
//!
//! Propósito: defender la correctness de
//! `begin`/`commit`/`rollback`/`insert`/`update`/`delete` ante secuencias
//! random — el segundo nivel de la red de seguridad después de M3
//! (planner). M3 verifica que el optimizer no rompe resultados; este
//! pack verifica que el storage layer no corrompe disco.
//!
//! Invariantes verificadas:
//!
//! 1. **Idempotencia de commit**: tras `COMMIT`, las filas insertadas
//!    son visibles en una sesión nueva (reopen).
//! 2. **Rollback descarta**: tras `ROLLBACK`, las filas insertadas en la
//!    tx NO son visibles en una sesión nueva.
//! 3. **Integridad post-mortem**: tras cualquier secuencia válida,
//!    `INTEGRITY CHECK` no reporta findings.
//!
//! Como M3, zero deps externas (ADR-0001) y LCG determinístico (cada
//! falla imprime seed reproducible).

use std::error::Error;
use std::path::{Path, PathBuf};

use gabysql::sql::{parse, Engine, Value};
use gabysql::storage::Pager;

// ---------------------------------------------------------------------------
// LCG determinístico (mismas constantes que gabybench / proptest_planner /
// fuzz_parser).
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
// Helpers.
// ---------------------------------------------------------------------------

fn tmp_db(seed: u64, label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("gby_pager_proptest_{}_{:x}.db", label, seed))
}

fn cleanup(p: &Path) {
    let _ = std::fs::remove_file(p);
    let mut wal = p.as_os_str().to_owned();
    wal.push(".wal");
    let _ = std::fs::remove_file(PathBuf::from(wal));
}

fn create_table(db: &Path) -> Result<(), Box<dyn Error>> {
    let mut pager = Pager::create(db)?;
    pager.begin()?;
    {
        let mut engine = Engine::new(&mut pager);
        for stmt in parse("CREATE TABLE t (id INT PRIMARY KEY, v INT)")? {
            engine.exec(stmt)?;
        }
    }
    pager.commit()?;
    pager.close()?;
    Ok(())
}

fn read_all_ids(db: &Path) -> Result<Vec<i64>, Box<dyn Error>> {
    let mut pager = Pager::open(db)?;
    pager.begin()?;
    let mut engine = Engine::new(&mut pager);
    let rs = engine.exec(parse("SELECT id FROM t ORDER BY id")?.remove(0))?;
    let ids: Vec<i64> = rs
        .rows
        .iter()
        .map(|r| match r[0] {
            Value::Integer(n) => n,
            _ => panic!("id no es INT"),
        })
        .collect();
    drop(engine);
    let _ = pager.rollback();
    let _ = pager.close();
    Ok(ids)
}

fn integrity_check_clean(db: &Path) -> Result<bool, Box<dyn Error>> {
    let mut pager = Pager::open(db)?;
    pager.begin()?;
    let mut engine = Engine::new(&mut pager);
    let rs = engine.exec(parse("INTEGRITY CHECK")?.remove(0))?;
    // Si rs.rows está vacío o el mensaje empieza con "OK", está limpio.
    let clean = rs.rows.is_empty()
        || rs
            .message
            .as_deref()
            .map(|m| m.contains("OK") || m.contains("no findings"))
            .unwrap_or(false);
    drop(engine);
    let _ = pager.rollback();
    let _ = pager.close();
    Ok(clean)
}

// ---------------------------------------------------------------------------
// Generadores de secuencias.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Insert(i64, i64), // (id, v)
    UpdateById(i64),  // SET v=v+1 WHERE id=...
    DeleteById(i64),  // DELETE WHERE id=...
}

/// Genera una secuencia de ops random. Los IDs vienen de un pool acotado
/// (para que update/delete tengan chance de matchear inserts previos).
fn gen_ops(rng: &mut Lcg, len: usize) -> Vec<Op> {
    let mut ops = Vec::with_capacity(len);
    for _ in 0..len {
        let id = rng.range(0, 30) as i64;
        let pick = rng.range(0, 10);
        ops.push(match pick {
            0..=5 => Op::Insert(id, rng.range(0, 1000) as i64),
            6..=7 => Op::UpdateById(id),
            _ => Op::DeleteById(id),
        });
    }
    ops
}

/// Aplica una sola op. Errores (PK duplicada, fila no existe → 0 filas
/// post-ANSI fix) NO son fallos del test — son comportamiento esperado
/// del motor ante input random. Solo nos importa que NO panic + que el
/// estado final sea consistente.
fn apply_op(engine: &mut Engine, op: &Op) -> Result<(), Box<dyn Error>> {
    let sql = match op {
        Op::Insert(id, v) => format!("INSERT INTO t (id, v) VALUES ({}, {})", id, v),
        Op::UpdateById(id) => format!("UPDATE t SET v = v + 1 WHERE id = {}", id),
        Op::DeleteById(id) => format!("DELETE FROM t WHERE id = {}", id),
    };
    // Ignoramos errores del engine — son comportamiento esperado para
    // input random (PK dup, NOT NULL, etc.). Solo nos importa que no
    // panic.
    let _ = engine.exec(parse(&sql)?.remove(0));
    Ok(())
}

/// Modelo en Rust de qué debería pasar — replica la semántica del
/// engine para que podamos comparar. Acepta los mismos `Op` y mantiene
/// un HashMap<id, v>. Errores (PK dup, fila no existe) se ignoran como
/// el engine post-ANSI fix.
fn apply_to_model(model: &mut std::collections::BTreeMap<i64, i64>, op: &Op) {
    match op {
        Op::Insert(id, v) => {
            // PK dup → engine rechaza, model también.
            model.entry(*id).or_insert(*v);
        }
        Op::UpdateById(id) => {
            if let Some(v) = model.get_mut(id) {
                *v += 1;
            }
            // No existe → 0 filas (post-ANSI fix).
        }
        Op::DeleteById(id) => {
            model.remove(id);
            // No existe → 0 filas (post-ANSI fix).
        }
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// Invariante #1: tras commit + reopen, las filas insertadas son
/// visibles y matchean el modelo computado en Rust.
#[test]
fn pager_commit_visibility_invariant() -> Result<(), Box<dyn Error>> {
    const ITERS: usize = 40;
    const OPS_PER_RUN: usize = 50;
    let outer_seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut outer = Lcg::new(outer_seed);

    for run in 0..ITERS {
        let seed = outer.next_u64();
        let db = tmp_db(seed, "commit");
        cleanup(&db);
        create_table(&db)?;

        let mut model: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
        let ops = gen_ops(&mut Lcg::new(seed), OPS_PER_RUN);

        {
            let mut pager = Pager::open(&db)?;
            pager.begin()?;
            {
                let mut engine = Engine::new(&mut pager);
                for op in &ops {
                    apply_op(&mut engine, op)?;
                    apply_to_model(&mut model, op);
                }
            }
            pager.commit()?;
            pager.close()?;
        }

        let actual_ids = read_all_ids(&db)?;
        let expected_ids: Vec<i64> = model.keys().copied().collect();
        assert_eq!(
            actual_ids, expected_ids,
            "Run {} (seed=0x{:x}): post-commit difiere del modelo\nesperado: {:?}\nactual: {:?}\nops: {:?}",
            run, seed, expected_ids, actual_ids, ops
        );

        assert!(
            integrity_check_clean(&db)?,
            "Run {} (seed=0x{:x}): INTEGRITY CHECK encontró findings",
            run,
            seed
        );

        cleanup(&db);
    }
    Ok(())
}

/// Invariante #2: rollback descarta todo lo escrito en la tx — el
/// reopen ve EXACTAMENTE el estado pre-tx.
#[test]
fn pager_rollback_discards_invariant() -> Result<(), Box<dyn Error>> {
    const ITERS: usize = 30;
    const PRE_OPS: usize = 20;
    const ROLLBACK_OPS: usize = 30;
    let outer_seed: u64 = 0xBADC_0FFE_DEAD_BEEF;
    let mut outer = Lcg::new(outer_seed);

    for run in 0..ITERS {
        let seed = outer.next_u64();
        let db = tmp_db(seed, "rollback");
        cleanup(&db);
        create_table(&db)?;

        // Fase 1: commit con PRE_OPS ops — esto debe sobrevivir.
        let pre_ops = gen_ops(&mut Lcg::new(seed), PRE_OPS);
        let mut model: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
        {
            let mut pager = Pager::open(&db)?;
            pager.begin()?;
            {
                let mut engine = Engine::new(&mut pager);
                for op in &pre_ops {
                    apply_op(&mut engine, op)?;
                    apply_to_model(&mut model, op);
                }
            }
            pager.commit()?;
            pager.close()?;
        }
        let snapshot_pre: Vec<i64> = model.keys().copied().collect();

        // Fase 2: tx con ROLLBACK_OPS ops, luego ROLLBACK — NADA de
        // esto debe sobrevivir.
        let rollback_ops = gen_ops(&mut Lcg::new(seed ^ 0xDEAD), ROLLBACK_OPS);
        {
            let mut pager = Pager::open(&db)?;
            pager.begin()?;
            {
                let mut engine = Engine::new(&mut pager);
                for op in &rollback_ops {
                    apply_op(&mut engine, op)?;
                }
            }
            pager.rollback()?;
            pager.close()?;
        }

        let actual = read_all_ids(&db)?;
        assert_eq!(
            actual, snapshot_pre,
            "Run {} (seed=0x{:x}): rollback NO descartó cambios\nsnapshot_pre: {:?}\nactual: {:?}\nrollback_ops: {:?}",
            run, seed, snapshot_pre, actual, rollback_ops
        );

        assert!(
            integrity_check_clean(&db)?,
            "Run {} (seed=0x{:x}): INTEGRITY CHECK encontró findings post-rollback",
            run,
            seed
        );

        cleanup(&db);
    }
    Ok(())
}

/// Invariante #3: chain de N transacciones (commits y rollbacks
/// intercalados) preserva integridad. El modelo Rust solo aplica las
/// committed; las rolled-back se descartan.
#[test]
fn pager_chained_tx_integrity_invariant() -> Result<(), Box<dyn Error>> {
    const ITERS: usize = 20;
    const TX_PER_RUN: usize = 8;
    const OPS_PER_TX: usize = 10;
    let outer_seed: u64 = 0xF00D_BABE_C0DE_DEAD;
    let mut outer = Lcg::new(outer_seed);

    for run in 0..ITERS {
        let seed = outer.next_u64();
        let db = tmp_db(seed, "chain");
        cleanup(&db);
        create_table(&db)?;

        let mut tx_rng = Lcg::new(seed);
        let mut model: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();

        for tx_i in 0..TX_PER_RUN {
            let ops = gen_ops(&mut tx_rng, OPS_PER_TX);
            // 70% commit, 30% rollback.
            let do_commit = tx_rng.range(0, 10) < 7;

            let mut pager = Pager::open(&db)?;
            pager.begin()?;
            {
                let mut engine = Engine::new(&mut pager);
                for op in &ops {
                    apply_op(&mut engine, op)?;
                }
            }
            if do_commit {
                pager.commit()?;
                for op in &ops {
                    apply_to_model(&mut model, op);
                }
            } else {
                pager.rollback()?;
            }
            pager.close()?;

            // Verificación intermedia.
            let actual = read_all_ids(&db)?;
            let expected: Vec<i64> = model.keys().copied().collect();
            assert_eq!(
                actual, expected,
                "Run {} tx {} (seed=0x{:x}, do_commit={}): mismatch\nesperado: {:?}\nactual: {:?}",
                run, tx_i, seed, do_commit, expected, actual
            );
        }

        assert!(
            integrity_check_clean(&db)?,
            "Run {} (seed=0x{:x}): INTEGRITY CHECK post-chain encontró findings",
            run,
            seed
        );

        cleanup(&db);
    }
    Ok(())
}
