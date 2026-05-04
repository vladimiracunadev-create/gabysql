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
gabysql init [--force] <file.db>
gabysql info <file.db>
gabysql exec <file.db> "<SQL...>"
gabysql repl <file.db>
```

> `init` rehúsa sobrescribir un archivo existente. Pasa `--force` para reemplazarlo intencionalmente.

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
UPDATE users SET name = 'Ana M', active = FALSE WHERE id = 1;
DELETE FROM users WHERE id = 1;
```

### Índices secundarios
```sql
-- Crear un índice secundario sobre una columna no-PK.
-- Si la tabla ya tiene filas, el índice se backfillea automáticamente.
CREATE INDEX idx_users_name ON users (name);

-- Una vez creado, las búsquedas por igualdad sobre la columna usan el índice.
SELECT * FROM users WHERE name = 'Ana';

-- Eliminar el índice. Vuelve a fallar el SELECT por esa columna.
DROP INDEX idx_users_name;
```

Reglas de los índices secundarios:
- una sola columna por índice (no compuestos).
- soportan equality (`=`), no rangos ni `BETWEEN`.
- se mantienen automáticamente al `INSERT`, `UPDATE` (cuando cambia la columna indexada) y `DELETE`.
- **no admiten** columnas `JSON` (sin semántica canónica de igualdad).
- el nombre del índice debe ser único en toda la base de datos.

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
- La PK debe ser **una sola columna `INT`**. Esta versión no soporta PKs compuestas ni de otros tipos.
- `PRIMARY KEY` no puede ser `NULL`.
- PK duplicada se rechaza al `INSERT`.
- `WHERE col = val` funciona sobre la PK siempre, y sobre cualquier otra columna **solo si tiene un índice secundario**.
- `WHERE col BETWEEN a AND b` solo funciona sobre la PK (sin range scan en índices secundarios todavía).
- `UPDATE` y `DELETE` solo aceptan `WHERE pk = N`, no por columna no-PK.
- `UPDATE` no permite cambiar la PK; intentarlo devuelve error explícito.
- `UPDATE` y `DELETE` sobre una PK inexistente retornan error (no son no-ops silenciosos).

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

### Tope de conexiones simultáneas
El servidor por defecto limita a `64` conexiones activas. Las conexiones por encima del techo reciben `503` y se cierran sin generar threads. Para ajustarlo:
```powershell
cargo run --release --bin gabysql-server -- -dir ./dbs -max-connections 32
```

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
