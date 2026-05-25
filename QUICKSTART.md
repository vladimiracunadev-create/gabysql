# 🚀 gabysql · Quickstart

> **Arrancar el motor en 3 pasos.** Para detalles de instalación por OS, ver [INSTALL.md](INSTALL.md). Para uso completo, ver [USER_MANUAL.md](USER_MANUAL.md).

---

## 0️⃣ Requisitos

- **Rust toolchain estable** (`1.94+`). Instala con `https://rustup.rs`.
- **Git**.
- *(Opcional)* **Docker + Docker Compose** si prefieres no compilar.
- *(Opcional)* **PHP 8.2** si quieres levantar `phpgabyadmin` localmente.

---

## 1️⃣ Build + DB local

```bash
git clone https://github.com/vladimiracunadev-create/gabysql
cd gabysql
cargo build --release --bin gabysql --bin gabysql-server
./target/release/gabysql init demo.db
```

> `init` rehúsa sobrescribir un archivo existente. Si quieres reset: `init --force <file.db>`.

---

## 2️⃣ Tu primera sesión SQL

```bash
./target/release/gabysql exec demo.db "
  CREATE TABLE users (
    id INT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    name TEXT,
    score INT DEFAULT 0
  );
  CREATE TABLE orders (
    id INT PRIMARY KEY,
    user_id INT REFERENCES users(id) ON DELETE CASCADE,
    total FLOAT
  );
  INSERT INTO users (id,email,name,score) VALUES (1,'ana@x','Ana',9);
  INSERT INTO users (id,email,name)       VALUES (2,'beto@x','Beto');
  INSERT INTO orders (id,user_id,total)   VALUES (10,1,99.5);
  INSERT INTO orders (id,user_id,total)   VALUES (11,2,42.0);
  CREATE INDEX idx_users_name ON users (name);
  SELECT id,email,name,score FROM users ORDER BY name ASC;
  SELECT * FROM users WHERE name = 'Ana';
  -- INNER JOIN clásico con aliases y columnas cualificadas:
  SELECT u.name, o.total FROM users u INNER JOIN orders o ON u.id = o.user_id ORDER BY u.name ASC;
  -- LEFT JOIN: usuarios sin orders aparecen con o.total = NULL
  SELECT u.name, o.total FROM users u LEFT JOIN orders o ON u.id = o.user_id;
  -- Subquery IN: usuarios que tienen al menos un order
  SELECT name FROM users WHERE id IN (SELECT user_id FROM orders);
  UPDATE users SET score = 10 WHERE id = 1;
  DELETE FROM users WHERE id = 2;       -- cascade: tira orders.id=11 también
  INTEGRITY CHECK;
"
```

Lo que acabas de probar:
- `CREATE TABLE` con `NOT NULL`, `UNIQUE`, `DEFAULT`, y `REFERENCES … ON DELETE CASCADE` inline.
- `INSERT` que aplica DEFAULTs (`score=0` para Beto), valida NOT NULL y UNIQUE.
- Persistencia con WAL + CRC32 (cualquier reinicio recupera o rechaza explícitamente).
- `ORDER BY name ASC` (sort en memoria post-scan; ASC es default; NULLs primero).
- `INNER`/`LEFT JOIN` con aliases (`AS`) y columnas cualificadas (`tabla.col`). Index-loop automático: como `orders.user_id` matchea contra `users.id` (PK), el engine usa lookup directo en vez de full scan.
- Subquery `WHERE col IN (SELECT ...)` no-correlacionada — la subquery se ejecuta una vez y se materializa como set.
- Índice secundario sobre `name` con backfill automático.
- `WHERE name = 'Ana'` resuelto por índice (no full scan).
- `UPDATE`/`DELETE` por PK con mantenimiento automático del índice.
- `DELETE` con cascade: borrar `users.id=2` arrastra `orders` que lo referencia.
- `INTEGRITY CHECK`: barre páginas + filas + índices + FKs y reporta hallazgos.

---

## 3️⃣ Levantar la API HTTP (opcional)

```bash
./target/release/gabysql-server -db demo.db -addr :8080 -max-connections 64
# en otra terminal:
curl -s http://localhost:8080/health
curl -s -X POST http://localhost:8080/exec \
  -H 'content-type: application/json' \
  -d '{"sql":"SELECT * FROM users ORDER BY name ASC;"}'

# /tables y /schema devuelven el schema completo (constraints, FKs, índices)
curl -s "http://localhost:8080/tables?db=demo.db" | jq
```

Para token y multi-DB: `gabysql-server -dir ./dbs -token secret -max-connections 32`.

Para admin web: `php -S localhost:8000 -t web` y abre `http://localhost:8000/phpgabyadmin/`.

---

## 🐳 Vía Docker (si prefieres no compilar)

```bash
docker compose up -d --build
# server:        http://localhost:8080
# landing:       http://localhost:8000/
# admin web:     http://localhost:8000/phpgabyadmin/
# modelador ER:  http://localhost:8000/modeler/
```

> El modelador `gabymodeler v2` (PowerDesigner-style) genera DDL listo para gabysql desde una UI drag&drop — incluye `CREATE DATABASE`, `CREATE TABLE` con todas las constraints (NOT NULL/UNIQUE/DEFAULT/REFERENCES/ON DELETE), `CREATE INDEX`/`CREATE UNIQUE INDEX`. Pegás la salida en `phpgabyadmin → tab SQL` o usás **↘ Importar de gabysql** para reverse-engineering desde una DB existente. Manual completo con screenshots: [web/modeler/USER_MANUAL.md](web/modeler/USER_MANUAL.md).

---

## ⏭️ Siguiente paso

- Manual de uso completo: [USER_MANUAL.md](USER_MANUAL.md).
- Manual del modelador con screenshots: [web/modeler/USER_MANUAL.md](web/modeler/USER_MANUAL.md).
- Gramática SQL completa: [docs/SQL_REFERENCE.md](docs/SQL_REFERENCE.md).
- Endpoints HTTP: [docs/API.md](docs/API.md).
- Arquitectura del motor: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
- Si algo falla: [TROUBLESHOOTING.md](TROUBLESHOOTING.md).
