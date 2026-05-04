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
  CREATE TABLE users (id INT PRIMARY KEY, name TEXT, score INT);
  INSERT INTO users (id,name,score) VALUES (1,'Ana',9);
  INSERT INTO users (id,name,score) VALUES (2,'Beto',7);
  CREATE INDEX idx_users_name ON users (name);
  SELECT * FROM users WHERE name = 'Ana';
  UPDATE users SET score = 10 WHERE id = 1;
  SELECT * FROM users WHERE id BETWEEN 1 AND 10;
"
```

Lo que acabas de probar:
- `CREATE TABLE` con PK `INT`.
- Persistencia con WAL + CRC32 (cualquier reinicio recupera o rechaza explícitamente).
- Índice secundario sobre `name` con backfill automático.
- `WHERE name = 'Ana'` resuelto por índice (no full scan).
- `UPDATE`/`DELETE` por PK con mantenimiento automático del índice.
- `BETWEEN` sobre PK.

---

## 3️⃣ Levantar la API HTTP (opcional)

```bash
./target/release/gabysql-server -db demo.db -addr :8080 -max-connections 64
# en otra terminal:
curl -s http://localhost:8080/health
curl -s -X POST http://localhost:8080/exec \
  -H 'content-type: application/json' \
  -d '{"sql":"SELECT * FROM users;"}'
```

Para token y multi-DB: `gabysql-server -dir ./dbs -token secret -max-connections 32`.

Para admin web: `php -S localhost:8000 -t web` y abre `http://localhost:8000/phpgabyadmin/`.

---

## 🐳 Vía Docker (si prefieres no compilar)

```bash
docker compose up -d --build
# server: http://localhost:8080
# admin web: http://localhost:8000/phpgabyadmin/
```

---

## ⏭️ Siguiente paso

- Manual de uso completo: [USER_MANUAL.md](USER_MANUAL.md).
- Endpoints HTTP: [docs/API.md](docs/API.md).
- Arquitectura del motor: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
- Si algo falla: [TROUBLESHOOTING.md](TROUBLESHOOTING.md).
