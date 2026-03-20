# 📘 USER MANUAL

> **Guía de uso diario de `gabysql`, `gabysql-server` y `phpgabyadmin`.**

---

## 🧭 Navegación rápida

| Si quieres... | Abre esta sección |
|---|---|
| crear o consultar una DB local | CLI `gabysql` |
| usar el motor en modo interactivo | REPL |
| exponer la base por HTTP | `gabysql-server` |
| operar desde navegador | `phpgabyadmin` |
| ver ejemplos mínimos | `examples/README.md` |

---

## 1. 🖥️ CLI `gabysql`

El binario principal sirve para inicializar bases, inspeccionar metadatos, ejecutar SQL y abrir un REPL.

### Comandos
```text
gabysql init <file.db>
gabysql info <file.db>
gabysql exec <file.db> "<SQL...>"
gabysql repl <file.db>
```

### Ejemplos
```powershell
cargo run --release --bin gabysql -- init demo.db
cargo run --release --bin gabysql -- info demo.db
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, active BOOL);"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,name,active) VALUES (1,'Ana',TRUE);"
cargo run --release --bin gabysql -- exec demo.db "SELECT id,name FROM users WHERE id = 1;"
```

---

## 2. 🧾 SQL soportado

### DDL
```sql
CREATE TABLE users (
  id INT PRIMARY KEY,
  name TEXT,
  active BOOL,
  score FLOAT,
  born DATE,
  meta JSON
);
```

### DML
```sql
INSERT INTO users (id,name,active) VALUES (1,'Ana',TRUE);
SELECT * FROM users;
SELECT id,name FROM users WHERE id = 1;
SELECT id,name FROM users WHERE id BETWEEN 1 AND 10 LIMIT 5 OFFSET 0;
```

### Tipos soportados
- `INT`
- `TEXT`
- `BOOL`
- `FLOAT`
- `DATE`
- `DATETIME`
- `JSON`
- `NULL` para columnas no PK

### Reglas importantes
- La PK debe existir y ser `INT`
- `PRIMARY KEY` no puede ser `NULL`
- PK duplicada se rechaza
- `WHERE` hoy solo soporta `=` y `BETWEEN` sobre la PK

---

## 3. ⌨️ REPL

```powershell
cargo run --release --bin gabysql -- repl demo.db
```

Cada sentencia debe terminar con `;`.

---

## 4. 🌐 `gabysql-server`

Expone la base por HTTP/JSON.

### Single DB
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
```

### Multi DB
```powershell
cargo run --release --bin gabysql-server -- -dir ./dbs -addr :8080
```

### Token opcional
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -token secret
```

El cliente debe enviar uno de estos headers:
- `X-Gabysql-Token: secret`
- `Authorization: Bearer secret`

> [!TIP]
> Para detalles de endpoints, payloads y errores, ve a [docs/API.md](docs/API.md).

---

## 5. 🧭 `phpgabyadmin`

Es un cliente PHP del API HTTP, no un motor separado.

### Levantarlo localmente
```powershell
php -S localhost:8000 -t web
```

### Flujo recomendado
1. Arranca `gabysql-server`
2. Arranca `phpgabyadmin`
3. Entra a `http://localhost:8000/phpgabyadmin/`
4. Selecciona DB, tabla o ejecuta SQL

### Seguridad del admin
- Si defines `GABYADMIN_TOKEN`, el admin pedirá login
- La cookie de login está firmada
- Por defecto solo permite `localhost` y loopback
- Para apuntar a un server remoto debes definir `GABYADMIN_ALLOW_REMOTE=1`

---

## 6. 🧪 Ejemplos incluidos

Consulta [examples/README.md](examples/README.md) para ejemplos de clientes Python y PHP contra CLI y HTTP.

---

## 7. ✅ Buenas prácticas de uso

- Usa `gabysql` para operaciones locales y pruebas rápidas
- Usa `gabysql-server` cuando necesites UI web o integración HTTP
- Haz backup offline del `.db` antes de cambios manuales o pruebas destructivas
- Si necesitas reproducibilidad entre equipos, usa Docker Compose
