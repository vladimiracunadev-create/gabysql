<?php ?><!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>gabysql - Base de Datos Embebida en Rust</title>
  <style>
    :root{
      --bg:#0d1117;
      --panel:#141b24;
      --panel-2:#0f151d;
      --text:#ecf2ff;
      --muted:#b7c5df;
      --line:#253244;
      --accent:#2f8fdd;
      --accent-2:#7bc5ff;
      --danger:#ffb3c1;
    }
    *{box-sizing:border-box}
    body{margin:0;background:radial-gradient(circle at top,#162131 0%,#0d1117 48%,#090c12 100%);color:var(--text);font-family:Georgia,"Times New Roman",serif}
    .wrap{max-width:1120px;margin:0 auto;padding:32px 20px 56px}
    .hero,.card{background:rgba(20,27,36,.92);border:1px solid var(--line);border-radius:20px;box-shadow:0 18px 50px rgba(0,0,0,.28)}
    .hero{padding:30px}
    .grid{display:grid;gap:18px;margin-top:18px}
    @media(min-width:980px){.grid{grid-template-columns:1.2fr .8fr}}
    .card{padding:20px}
    h1,h2,h3{margin:0 0 10px;font-family:"Palatino Linotype","Book Antiqua",Palatino,serif}
    h1{font-size:36px;letter-spacing:.3px}
    h2{font-size:21px}
    h3{font-size:17px}
    p,li{line-height:1.65;color:var(--muted)}
    .lead{font-size:18px;max-width:820px}
    .badges{display:flex;flex-wrap:wrap;gap:10px;margin-top:16px}
    .badges span,.pill{display:inline-block;border:1px solid #36506f;background:#102033;color:#d6e8ff;border-radius:999px;padding:7px 12px;font-size:12px;letter-spacing:.2px}
    .columns{display:grid;gap:14px}
    @media(min-width:920px){.columns{grid-template-columns:1fr 1fr}}
    pre,code{font-family:"Cascadia Code","Consolas",monospace}
    pre{margin:0;background:var(--panel-2);border:1px solid var(--line);border-radius:14px;padding:14px;overflow:auto;color:#dce9ff}
    a.btn{display:inline-block;background:linear-gradient(135deg,var(--accent),var(--accent-2));color:#08111b;text-decoration:none;font-weight:700;padding:10px 14px;border-radius:12px}
    ul{padding-left:18px;margin:10px 0 0}
    .muted{font-size:13px;color:#9db0ce}
    .warn{color:var(--danger)}
  </style>
</head>
<body>
  <div class="wrap">
    <section class="hero">
      <h1>gabysql</h1>
      <p class="lead">
        <b>gabysql</b> es una base de datos <b>embebida</b> escrita en <b>Rust</b>, orientada a archivo único,
        WAL simple, árbol de hojas enlazadas persistente y un subconjunto de SQL lo bastante pequeño como para ser entendible,
        pero lo bastante sólido para operar como producto base.
      </p>
      <div class="badges">
        <span>Rust core</span>
        <span>Archivo .db</span>
        <span>WAL + recovery</span>
        <span>Índice primario INT</span>
        <span>SELECT full scan + rangos</span>
        <span>HTTP/JSON + phpgabyadmin</span>
      </div>
    </section>

    <div class="grid">
      <section class="card">
        <h2>Qué soporta hoy</h2>
        <div class="columns">
          <div>
            <h3>SQL disponible</h3>
            <pre><code>CREATE TABLE users (
  id INT PRIMARY KEY,
  name TEXT,
  active BOOL,
  score FLOAT,
  born DATE,
  meta JSON
);

INSERT INTO users (id,name,active,score)
VALUES (1,'Ana',TRUE,9.5);

SELECT * FROM users;
SELECT id,name FROM users LIMIT 10 OFFSET 20;
SELECT * FROM users WHERE id = 1;
SELECT * FROM users WHERE id BETWEEN 1 AND 10;</code></pre>
          </div>
          <div>
            <h3>Tipos</h3>
            <ul>
              <li><code>INT</code></li>
              <li><code>TEXT</code></li>
              <li><code>BOOL</code></li>
              <li><code>FLOAT</code></li>
              <li><code>DATE</code> y <code>DATETIME</code> almacenados como texto</li>
              <li><code>JSON</code> almacenado como texto</li>
              <li><code>NULL</code> real para columnas no PK</li>
            </ul>
            <p class="muted">La PK actual sigue siendo <code>INT</code> y se valida como única.</p>
          </div>
        </div>
      </section>

      <section class="card">
        <h2>Panel Web</h2>
        <p>
          <b>phpgabyadmin</b> se mantiene como frontend ligero. No ejecuta el motor directamente:
          consume el API HTTP expuesto por <code>gabysql-server</code>.
        </p>
        <p><a class="btn" href="/phpgabyadmin/">Abrir phpgabyadmin</a></p>
        <p class="muted">Por defecto se espera un servidor local en <code>http://localhost:8080</code>.</p>
        <p class="warn">El admin está pensado para entorno controlado. Si se expone fuera de localhost, usa token.</p>
      </section>
    </div>

    <div class="grid">
      <section class="card">
        <h2>Cómo levantarlo</h2>
        <pre><code># compilar binarios
cargo build --release --bin gabysql --bin gabysql-server

# crear base
.\target\release\gabysql.exe init demo.db

# ejecutar SQL
.\target\release\gabysql.exe exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, name TEXT);"

# levantar API (single-db)
.\target\release\gabysql-server.exe -db demo.db -addr :8080

# o multi-db
mkdir dbs
.\target\release\gabysql-server.exe -dir .\dbs -addr :8080

# levantar web PHP
php -S localhost:8000 -t web</code></pre>
      </section>

      <section class="card">
        <h2>Límites deliberados</h2>
        <ul>
          <li>No hay <code>UPDATE</code> ni <code>DELETE</code> todavía.</li>
          <li>No hay <code>JOIN</code>, <code>ORDER BY</code> ni planner cost-based.</li>
          <li>No hay índices secundarios ni concurrencia multiwriter.</li>
          <li>El storage sigue priorizando claridad sobre throughput máximo.</li>
        </ul>
        <p class="muted">La meta de esta versión es estabilidad razonable y operabilidad simple, no competir todavía con SQLite/PostgreSQL.</p>
      </section>
    </div>
  </div>
</body>
</html>
