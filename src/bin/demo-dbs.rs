//! demo-dbs — crea 3 bases de datos demo de tamaño chico para mostrar
//! las features más nuevas (Z = USERS/ROLES/RLS, Y = DECIMAL/BLOB/UUID,
//! W3 + P3 = window functions + ANALYZE/EXPLAIN). Sirve para verificación
//! visual rápida desde `phpgabyadmin` o `gabysql repl`.
//!
//! Uso:
//!     cargo run --release --bin demo-dbs

use gabysql::sql::{parse, Engine};
use gabysql::storage::Pager;
use gabysql::DbResult;
use std::path::Path;

const OUT_DIR: &str = "demo-dbs";

fn rm_if_exists(p: &Path) {
    let _ = std::fs::remove_file(p);
    let mut wal = p.as_os_str().to_owned();
    wal.push(".wal");
    let _ = std::fs::remove_file(std::path::PathBuf::from(wal));
}

fn run_script(pager: &mut Pager, sql: &str) -> DbResult<()> {
    pager.begin()?;
    let res = (|| -> DbResult<()> {
        let mut engine = Engine::new(pager);
        for stmt in parse(sql)? {
            engine.exec(stmt)?;
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

fn main() -> DbResult<()> {
    std::fs::create_dir_all(OUT_DIR).ok();
    if let Err(e) = build_auth_demo() {
        eprintln!("auth_demo FAIL: {}", e);
    }
    if let Err(e) = build_inventory() {
        eprintln!("inventory FAIL: {}", e);
    }
    if let Err(e) = build_analytics() {
        eprintln!("analytics FAIL: {}", e);
    }
    println!("\nDemo DBs creadas en `{}/`:", OUT_DIR);
    for name in &["auth_demo.db", "inventory.db", "analytics.db"] {
        let p = format!("{}/{}", OUT_DIR, name);
        let sz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        println!("  {:20} {:>10} bytes", name, sz);
    }
    println!("\nAbrir con:  gabysql repl demo-dbs/<name>.db");
    println!("Ver con:    cargo run --bin gabysql-server -- --dir demo-dbs (admin web)");
    Ok(())
}

// ============================================================
// DEMO 1 — auth_demo.db  (Z: USERS/ROLES + RLS WITH CHECK)
// ============================================================

fn build_auth_demo() -> DbResult<()> {
    let path = format!("{}/auth_demo.db", OUT_DIR);
    let path = Path::new(&path);
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;

    println!("== auth_demo.db (Z: USERS + ROLES + RLS) ==");

    run_script(
        &mut pager,
        "CREATE TABLE customers (
            id INT PRIMARY KEY,
            name TEXT NOT NULL,
            country TEXT NOT NULL,
            credit_limit DECIMAL(12,2) NOT NULL DEFAULT 1000.00
         );
         INSERT INTO customers (id, name, country, credit_limit) VALUES
            (1, 'Acme AR', 'AR', 50000.00),
            (2, 'Beta BR', 'BR', 25000.00),
            (3, 'Gamma CL', 'CL', 15000.00),
            (4, 'Delta AR', 'AR', 80000.00),
            (5, 'Epsilon UY', 'UY', 10000.00);
         CREATE USER alice WITH PASSWORD 'demo-alice';
         CREATE USER bob WITH PASSWORD 'demo-bob';
         CREATE ROLE ar_analyst;
         GRANT SELECT ON customers TO alice;
         GRANT SELECT ON customers TO bob;",
    )?;

    // RLS: bob solo ve clientes AR. alice (sin policy, modelo default-deny
    // per-tabla post-policy) ve todo si no hay policy aplicable... ACA
    // gabysql implementa default-deny cuando hay AL MENOS una policy USING
    // — entonces bob ve solo AR y alice ve nada salvo que también haya
    // policy. Para que alice vea todo, le damos policy USING (true).
    run_script(
        &mut pager,
        "CREATE POLICY p_bob_ar ON customers FOR SELECT TO bob USING (country = 'AR');
         CREATE POLICY p_alice_all ON customers FOR SELECT TO alice USING (true);
         CREATE POLICY p_credit_cap ON customers FOR INSERT WITH CHECK (credit_limit <= 100000.00);",
    )?;

    // Probar que las stats funcionan (P3).
    run_script(&mut pager, "ANALYZE TABLE customers;")?;

    pager.close()?;
    println!("   ok — 5 customers, 2 users, 1 role, 3 policies, ANALYZE corrido");
    println!("   probar: SET SESSION AUTHORIZATION 'bob' WITH PASSWORD 'demo-bob';");
    println!("           SELECT * FROM customers;  -- ve 2 rows (AR)");
    println!("           EXPLAIN SELECT * FROM customers;  -- [est.rows=5] de ANALYZE");
    Ok(())
}

// ============================================================
// DEMO 2 — inventory.db  (Y: DECIMAL exacto + BLOB + UUID)
// ============================================================

fn build_inventory() -> DbResult<()> {
    let path = format!("{}/inventory.db", OUT_DIR);
    let path = Path::new(&path);
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;

    println!("\n== inventory.db (Y: DECIMAL/BLOB/UUID) ==");

    run_script(
        &mut pager,
        "CREATE TABLE products (
            id INT PRIMARY KEY,
            sku TEXT NOT NULL UNIQUE,
            name VARCHAR(80) NOT NULL,
            price DECIMAL(12,2) NOT NULL,
            cost DECIMAL(12,4) NOT NULL,
            stock_qty INT UNSIGNED NOT NULL DEFAULT 0,
            thumb BLOB,
            sku_uuid UUID NOT NULL
         );
         INSERT INTO products (id, sku, name, price, cost, stock_qty, thumb, sku_uuid) VALUES
            (1, 'SKU-001', 'Laptop Pro 14',    1499.99, 1100.0000, 45, X'89504E47',          '11111111-1111-4111-8111-111111111111'),
            (2, 'SKU-002', 'Mouse inalambrico',   29.50,   12.7500, 200, X'47494638',        '22222222-2222-4222-8222-222222222222'),
            (3, 'SKU-003', 'Monitor 27 4K',     449.00,  315.5000, 18, X'FFD8FFE0',          '33333333-3333-4333-8333-333333333333'),
            (4, 'SKU-004', 'Teclado mecanico',  119.95,   78.2500, 60, X'89504E470D0A1A0A',  '44444444-4444-4444-8444-444444444444'),
            (5, 'SKU-005', 'Webcam HD',          85.00,   55.0000, 30, NULL,                 '55555555-5555-4555-8555-555555555555');",
    )?;

    // Demo aritmética DECIMAL exacta (Y7+Y8+Y9). margin = (price - cost) /
    // cost. Estos cálculos son exact, no float-rounded.
    run_script(
        &mut pager,
        "CREATE TABLE inventory_summary AS
            SELECT
                id,
                sku,
                price,
                cost,
                price - cost AS margin_abs,
                (price - cost) / cost AS margin_ratio
            FROM products;
         ANALYZE TABLE products;
         ANALYZE TABLE inventory_summary;",
    )?;

    pager.close()?;
    println!("   ok — 5 productos con DECIMAL(12,2)+(12,4), BLOB X'hex', UUID generadas");
    println!("   probar: SELECT * FROM inventory_summary;  -- margin_abs y margin_ratio exact");
    println!(
        "           SELECT SUM(price), AVG(cost) FROM products;  -- agregados Decimal-exact (Y9)"
    );
    println!("           EXPLAIN SELECT * FROM products;  -- [est.rows=5]");
    Ok(())
}

// ============================================================
// DEMO 3 — analytics.db  (W3 window functions + P3 stats)
// ============================================================

fn build_analytics() -> DbResult<()> {
    let path = format!("{}/analytics.db", OUT_DIR);
    let path = Path::new(&path);
    rm_if_exists(path);
    let mut pager = Pager::create(path)?;

    println!("\n== analytics.db (W3 window functions + P3 stats) ==");

    run_script(
        &mut pager,
        "CREATE TABLE sales (
            id INT PRIMARY KEY,
            region TEXT NOT NULL,
            salesperson TEXT NOT NULL,
            qty INT NOT NULL,
            revenue DECIMAL(10,2) NOT NULL,
            sold_at DATETIME NOT NULL
         );",
    )?;

    // Cargamos 30 ventas distribuidas en 3 regiones, 5 vendedores cada una.
    let regions = ["NORTH", "SOUTH", "CENTER"];
    let names = ["Ana", "Luis", "Maria", "Pedro", "Sofia"];
    let mut sql =
        String::from("INSERT INTO sales (id, region, salesperson, qty, revenue, sold_at) VALUES\n");
    let mut id = 1;
    let mut first = true;
    for r in &regions {
        for n in &names {
            // 2 ventas por (region, salesperson) — pseudo-random determinístico
            for k in 0..2 {
                let qty = ((id * 7 + k * 13) % 50 + 1) as i64;
                let revenue_cents = qty * (50 + ((id * 11) % 30) as i64);
                let dollars = revenue_cents / 10;
                let cents = revenue_cents % 100;
                if !first {
                    sql.push_str(",\n");
                }
                first = false;
                sql.push_str(&format!(
                    "  ({}, '{}', '{}', {}, {}.{:02}, '2026-04-{:02} 10:00:00')",
                    id,
                    r,
                    n,
                    qty,
                    dollars,
                    cents,
                    (id % 28) + 1
                ));
                id += 1;
            }
        }
    }
    sql.push(';');
    run_script(&mut pager, &sql)?;

    run_script(&mut pager, "ANALYZE TABLE sales;")?;

    pager.close()?;
    println!(
        "   ok — {} ventas distribuidas en 3 regiones × 5 vendedores × 2 transacciones",
        id - 1
    );
    println!("   probar:");
    println!("     -- W3: ranking de vendedores por revenue dentro de cada region");
    println!("     SELECT region, salesperson, revenue,");
    println!("            ROW_NUMBER() OVER (PARTITION BY region ORDER BY revenue DESC) AS rk");
    println!("     FROM sales;");
    println!("     -- P3: estimación de filas en el plan");
    println!("     EXPLAIN SELECT * FROM sales WHERE region = 'NORTH';");
    Ok(())
}
