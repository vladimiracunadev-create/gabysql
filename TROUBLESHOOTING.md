# 🩺 TROUBLESHOOTING

> **Fallos frecuentes, causas probables y resolución rápida para `gabysql`.**

---

## 🪟 Windows: `cargo test` falla por `link.exe` o `kernel32.lib`

### Síntoma
El build compila pero no logra linkear tests o binarios release.

### Causa
Falta el toolchain nativo MSVC/Windows SDK.

### Solución
Instala:
- Visual Studio Build Tools
- MSVC C++ build tools
- Windows SDK

Mientras tanto puedes validar el proyecto con Docker:
```powershell
docker build -t gabysql .
```

---

## 🧱 `bad magic (not gabysql db)`

### Causa
El archivo no fue inicializado por `gabysql` o está corrupto.

### Solución
```powershell
cargo run --release --bin gabysql -- init demo.db
```

---

## 🔢 `unsupported gabysql file format: version=N (expected 33)`

### Causa
Intentas abrir un archivo con una versión de formato anterior. La versión actual es `33` (bump P4 / 2026-06-10 — column stats persistidas). Las versiones `1` a `32` quedaron explícitamente fuera — la política del motor es **no auto-upgrade** entre versiones (ver TECHNICAL_SPECS.md y los ADRs por bloque). La lista completa de bumps con su contexto vive en [COMPATIBILITY.md §5](COMPATIBILITY.md#5--formato-en-disco).

### Solución
- Re-crear la base con el binario actual: `gabysql init <file.db>`.
- Si tenías datos en la DB vieja, exportarlos antes con la versión que la creó (no hay aún herramienta automatizada de migración).

---

## 🛡️ `columna 'X' es NOT NULL; INSERT no la cubre`

### Causa
Una columna declarada `NOT NULL` quedó ausente del `INSERT` y no tiene `DEFAULT`, o se le pasó `NULL` literal.

### Solución
- Pasar un valor explícito en el `INSERT`, o
- Declarar `DEFAULT <literal>` en el `CREATE TABLE` para que el motor rellene la columna cuando se omita.

---

## 🔁 `violación de UNIQUE en índice 'uq_t_c' (PK existente: N)`

### Causa
El `INSERT`/`UPDATE` intenta colocar un valor que ya existe en otra fila para una columna `UNIQUE` (inline o `CREATE UNIQUE INDEX`). El pre-check del motor lo rechaza **antes de tocar disco**, así que la transacción queda intacta.

### Solución
- Cambiar el valor a uno único, o
- Buscar la PK ofensora (`N`) y decidir si se actualiza o se borra esa fila.

---

## 🔗 `violación de FK: 'X.col' = N no existe en 'Y'`

### Causa
El `INSERT`/`UPDATE` apunta a un parent que no existe en la tabla referenciada. Las FKs no se autocompletan ni se ignoran.

### Solución
- Verificar que el parent exista con `SELECT * FROM <parent> WHERE id = N`.
- Si la columna debe poder estar vacía, declararla nullable y dejar `NULL`.

---

## 🚫 `violación de FK: 'X.col' referencia 'Y' (ON DELETE RESTRICT, M fila(s) afectadas)`

### Causa
Estás intentando borrar un parent que todavía tiene filas hijas, y la FK fue declarada (o tomó el default) `ON DELETE RESTRICT`.

### Solución
- Borrar primero las filas hijas, o
- Re-declarar la FK con `ON DELETE CASCADE` (requiere recrear la tabla; en esta versión no hay `ALTER TABLE ... DROP CONSTRAINT`).

---

## 🩺 `INTEGRITY CHECK` reporta hallazgos

### Causa
El sweep operacional detectó algo: CRC inválido, fila no decodificable, entrada de índice huérfana o FK colgada. Los `kind` posibles están en [docs/SQL_REFERENCE.md §INTEGRITY CHECK](docs/SQL_REFERENCE.md#integrity-check).

### Solución
- `page_corrupt` o `row_decode` → restore desde backup; el archivo está físicamente comprometido.
- `orphan_index_entry` → suele indicar un crash entre escritura del índice y la fila; `DROP INDEX` + `CREATE INDEX` reconstruye limpio.
- `fk_target_missing` o `fk_orphan` → datos inconsistentes; arreglar manualmente con `INSERT` del parent o `DELETE` de las hijas.

---

## 🛡️ `page N corrupt: checksum mismatch` o `WAL record for page N fails checksum`

### Causa
La verificación CRC32 detectó que la página leída del `.db` o del `.wal` no coincide con su checksum almacenado. Causa típica: corte de energía durante una escritura, fallo de disco, o edición manual del archivo.

### Solución
- Restaurar `.db` desde el backup más reciente.
- Borrar el `.wal` solo si el error sale del WAL replay: el `.db` previo aún es válido.
- Si la corrupción es persistente, validar el medio de almacenamiento (smartctl, chkdsk).

---

## 🛢️ `base de datos 'X' ya existe` / `base de datos 'X' no existe`

### Causa
Salidas de `CREATE DATABASE` / `DROP DATABASE` cuando la operación no es idempotente.

### Solución
- Para crear: `CREATE DATABASE IF NOT EXISTS <name>;`
- Para borrar: `DROP DATABASE IF EXISTS <name>;`

---

## 🚦 `CREATE/DROP/SHOW DATABASE requieren modo -dir`

### Causa
El server fue arrancado con `-db <archivo.db>` (single-DB) y se mandó una sentencia que opera sobre el directorio.

### Solución
Reiniciar el server con `-dir <carpeta>`:
```bash
gabysql-server -dir ./dbs -addr :8080
```
o usar `gabysql exec` directamente desde la CLI sobre cualquier path dentro del directorio objetivo.

---

## 🔀 `no se admite mezclar CREATE/DROP/SHOW DATABASE con sentencias de tabla en el mismo /exec`

### Causa
En un `/exec` mandaste un combo como:
```sql
CREATE DATABASE shop;
CREATE TABLE shop_items (id INT PRIMARY KEY);
```
Esos statements no comparten transacción (uno opera sobre el directorio, el otro sobre la `.db` recién creada que aún no existe en el momento del parse).

### Solución
Separar en dos llamadas:
1. `CREATE DATABASE shop;` (sin `db` en el body, server lo despacha al directorio).
2. `CREATE TABLE ...;` (con `"db":"shop.db"` en el body).

---

## 🔒 `database is locked by another process`

### Causa
Otro proceso `gabysql` ya tiene la `.db` abierta. Desde [ADR-0013](docs/adr/0013-process-level-file-lock.md), `Pager::create/open` adquiere un lock exclusivo cross-process vía `File::try_lock()` (advisory en Linux/macOS, mandatory en Windows). El segundo proceso que intenta abrir el mismo archivo falla rápido en vez de corromperlo.

### Solución
- Identifica el proceso que tiene el archivo abierto (`gabysql-server`, otra CLI, un script colgado) y deténlo.
- Si quedó un proceso zombi en Linux/macOS y el lock no se liberó tras kill, reintenta tras unos segundos (el OS limpia el `flock` al cerrar el FD).
- En Windows, si un binario crasheó dejando el handle abierto, suele bastar con esperar a que el Resource Manager libere el handle; un reboot es el último recurso.

### Lo que esto NO significa
- No hay corrupción. El lock es preventivo: bloquea **antes** de tocar el archivo.
- No reemplaza MVCC ni un protocolo de replicación. Sigue siendo un solo escritor a la vez por archivo.

---

## 🚫 `refusing to overwrite existing database`

### Causa
`gabysql init <file.db>` se ejecutó sobre un archivo que ya existe. Es deliberado: la versión actual no destruye archivos sin pedirlo.

### Solución
- Si querías iniciar la DB existente, usa `gabysql info <file.db>` o `gabysql exec <file.db> ...` directamente.
- Si querías reset intencional: `gabysql init --force <file.db>`.

---

## 🔁 `duplicate primary key`

### Causa
Insertaste una fila con una PK `INT` ya usada.

### Solución
- usa otra PK
- consulta primero
- si quieres modificar el row, usa `UPDATE <tabla> SET ... WHERE <pk> = N`

---

## ✏️ `fila no existe: PK=N`

### Causa
Un `UPDATE` o `DELETE` apuntó a una PK que no existe en la tabla.

### Solución
- consulta primero con `SELECT ... WHERE pk = N`
- los `UPDATE`/`DELETE` son explícitos: no son no-ops silenciosos como en otros motores

---

## 🚦 `server busy: N active connections (max M)` (HTTP 503)

### Causa
El servidor alcanzó el techo de conexiones simultáneas (default `64`).

### Solución
- el cliente debe reintentar tras un backoff corto
- subir el techo si tu carga lo justifica: `gabysql-server -max-connections 128`

---

## 🔗 Errores de JOIN

### `[GBY-4017]` alias/nombre de tabla duplicado
Dos tablas del `FROM` quedaron expuestas bajo el mismo qualifier (alias o nombre). Reescribir con alias distintos: `FROM users AS u JOIN orders AS o`.

### `[GBY-4018]` columna ambigua
Una columna sin qualifier existe en más de una tabla del `FROM`. Cualificar con `tabla.col` o `alias.col`. Ejemplo: `SELECT u.id` en vez de `SELECT id`.

### `[GBY-4019]` qualifier no encontrado
`tabla.col` donde `tabla` no coincide con ningún nombre ni alias del `FROM`. Verificar que la tabla esté en el FROM y que estés usando el alias correcto (el alias **oculta** el nombre real).

### `[GBY-4020]` INNER JOIN sin ON
`INNER JOIN ...` siempre requiere `ON l = r`. Si querés cartesiano usar `CROSS JOIN` o la comma-syntax `FROM a, b`.

### `[GBY-4021]` CROSS JOIN con ON
`CROSS JOIN` es producto cartesiano puro y no admite predicado. Cambiar a `INNER JOIN ... ON ...`.

---

## 🔎 `EXISTS requiere '(SELECT ...)' a continuación` `[GBY-4015]`

### Causa
Tras `EXISTS` (o `NOT EXISTS`) no había un `(` que abriera una subquery `SELECT`.

### Solución
Reescribir como `EXISTS (SELECT col FROM tabla [WHERE ...])`.

---

## 🔎 `outer column 'X.Y' fuera de alcance` `[GBY-4016]`

### Causa
Una referencia a una columna cualificada (`outer_table.col`) en el RHS de un `=` se usó **fuera** de una subquery correlacionada — el SQL outer solo admite literales o subqueries `(SELECT ...)` ahí, no referencias a otras tablas.

### Solución
- Si querés correlacionar: envolver el predicado en `EXISTS (SELECT ... FROM ... WHERE inner_col = outer_table.outer_col)`.
- Si querés un valor: usar `= (SELECT ...)` (subquery escalar) en lugar de la referencia.

---

## 🔎 `subquery escalar en WHERE devolvió N filas; debe devolver a lo sumo 1` `[GBY-4014]`

### Causa
La consulta usa `WHERE col = (SELECT ...)` pero la subquery matcheó más de una fila. Una subquery escalar puede devolver a lo sumo 1 fila × 1 columna.

### Solución
- Restringir la subquery con un `WHERE` que la haga unívoca (típicamente filtrando por una columna `UNIQUE`).
- O usar `IN (SELECT ...)` en lugar de `=` si querés conservar el conjunto.

---

## 🔎 `WHERE: no se reconoció el operador después de la columna 'X'`

### Causa
La consulta usa un operador fuera de la gramática actual del WHERE. Desde los bloques **E1** y **E2** la lista soportada es: `=`, `<`, `>`, `<=`, `>=`, `<>`/`!=`, `BETWEEN ... AND ...`, `IS [NOT] NULL`, `[NOT] LIKE 'patron'`, `[NOT] IN (lista | SELECT)`, `EXISTS (...)`, conectados con `AND`/`OR`/`NOT` y paréntesis.

### Solución
- Revisar la lista en [docs/SQL_REFERENCE.md §SELECT](docs/SQL_REFERENCE.md#select).
- `ILIKE`, `REGEXP`, `GLOB`, `IS TRUE/FALSE` aún no se soportan — workaround con `LIKE` case-sensitive o reescritura.

---

## 🛑 `WHERE solo soporta PK (...) o columnas con índice secundario; '<col>' no está indexada`

### Causa
Hiciste `SELECT ... WHERE col = val` o `SELECT ... WHERE col BETWEEN a AND b` sobre una columna que no es PK y no tiene índice secundario. Solo aplica al fast-path indexado de `SELECT`; el WHERE compuesto (AND/OR/NOT, `<`, `>`, `LIKE`, `IS NULL`, `IN literal`) cae a FullScan y no exige índice.

### Solución
- Crear el índice: `CREATE INDEX idx_<tabla>_<col> ON <tabla> (<col>);` para que el SELECT con `=` o `BETWEEN` use lookup directo en vez de FullScan.
- O filtrar por la PK.
- En `UPDATE` / `DELETE` (desde el bloque E3) este mensaje no aparece: el WHERE puede ser cualquier expresión (FullScan + 3VL por defecto, fast-path solo para `pk = N` literal).

---

## 🔍 `ya existe un índice llamado '<name>' en la tabla '<table>'`

### Causa
Los nombres de índice son únicos en toda la base de datos (no solo por tabla).

### Solución
- usa otro nombre, idealmente con prefijo de tabla: `idx_users_name`, `idx_orders_status`.

---

## 🔍 `la columna '<col>' ya tiene un índice secundario`

### Causa
Esta versión soporta solo un índice secundario por columna.

### Solución
- si quieres reemplazarlo, primero `DROP INDEX <viejo>` y luego `CREATE INDEX`.

---

## 🚫 `no se admiten índices sobre columnas JSON en esta versión`

### Causa
`JSON` no tiene una semántica canónica de igualdad (dos representaciones distintas pueden ser equivalentes).

### Solución
- indexa una columna escalar derivada del JSON (`status TEXT`, `category INT`, etc.) y mantenla sincronizada en cliente.

---

## 🗃️ `tabla no existe`

### Causa
La tabla no fue creada o se consultó con otro nombre.

### Solución
```powershell
Invoke-WebRequest -UseBasicParsing "http://localhost:8080/tables?db=demo.db"
```

---

## 📁 `falta db (modo -dir)`

### Causa
Estás usando `gabysql-server -dir ...` pero no enviaste `db` en la query o payload.

### Solución
Incluye `db` en:
- `GET /tables?db=demo.db`
- `GET /schema?db=demo.db&table=users`
- `GET /rows?db=demo.db&table=users`
- `POST /exec` con `{ "db": "demo.db", "sql": "..." }`

---

## 🔐 `401 unauthorized`

### Causa
El server fue arrancado con `-token` y el cliente no envió token correcto.

### Solución
Envía:
- `X-Gabysql-Token`
- `Authorization: Bearer <token>`

---

## 🧭 `phpgabyadmin` no conecta al server

### Revisa
- que `gabysql-server` esté arriba
- que el host/puerto sean correctos
- que `GABYADMIN_SERVER` apunte al server correcto
- que no esté bloqueado un host remoto por `GABYADMIN_ALLOW_REMOTE`
- que el token HTTP coincida si el server está protegido

---

## 🌍 `Servidor remoto bloqueado` en `phpgabyadmin`

### Causa
Por seguridad, el admin solo acepta loopback por defecto.

### Solución
```powershell
$env:GABYADMIN_ALLOW_REMOTE='1'
php -S localhost:8000 -t web
```

---

## 🐳 Docker: puerto en uso

### Síntoma
`docker compose up` falla porque `8080` o `8000` ya están ocupados.

### Solución
- libera el puerto
- o cambia el mapeo en `docker-compose.yml`

---

## 🔄 Docker: cambios de código no se reflejan

### Solución
```powershell
docker compose up -d --build
```

---

## 🧱 Errores de CHECK constraints (L2 / L3)

### `[GBY-3008] CHECK_VIOLATED`
La fila viola un `CHECK (expr)` declarado en `CREATE TABLE`, agregado por `ALTER TABLE ADD CHECK`, o evaluado durante UPSERT/cascade.

**Resolución**:
- Ajustar el valor para que satisfaga la expresión, o
- Si el CHECK ya no aplica al negocio, removerlo con `ALTER TABLE <t> DROP CONSTRAINT <name>` (requiere nombre — re-declarar el CHECK con `CONSTRAINT <name> CHECK (...)` si fue creado anónimo).

### `[GBY-3009] FK_SET_NULL_VIOLATES_NOT_NULL`
La cascada `ON DELETE SET NULL` / `ON UPDATE SET NULL` intentaría poner NULL en la columna FK del hijo, pero esa columna está declarada `NOT NULL`. Borrar los hijos primero, o redeclarar la columna del hijo como nullable, o cambiar la acción a `CASCADE` / `SET DEFAULT`.

### `[GBY-3010] FK_SET_DEFAULT_MISSING`
La cascada `SET DEFAULT` no encuentra un `DEFAULT` declarado en la columna FK del hijo (o el DEFAULT es NULL con `NOT NULL`). Declarar un DEFAULT compatible o cambiar la acción.

### `[GBY-4069]` subquery dentro de CHECK
El AST del CHECK no admite subqueries (ANSI tampoco las exige). Reescribir como FK o trigger lógico desde la aplicación.

### `[GBY-4070]` CHECK fuera de contexto soportado
Aparece si el parser ve un `CHECK` en una forma sintáctica no contemplada.

---

## 🏷️ Errores de constraints nombradas (residual #2)

### `[GBY-4071] CONSTRAINT_NOT_FOUND`
`ALTER TABLE <t> DROP CONSTRAINT <name>` con un nombre que no existe.

**Resolución**: usar `DROP CONSTRAINT IF EXISTS <name>` para volverlo no-op, o consultar `INTEGRITY CHECK` / `phpgabyadmin` → Structure para ver los nombres reales.

### `[GBY-4072] CANNOT_DROP_PRIMARY_KEY_CONSTRAINT`
No se puede dropear la PK de una tabla. Crear una tabla nueva, copiar las filas y reemplazar (workaround manual; `ALTER PK` está en backlog).

---

## 🔁 Errores de UPDATE de PK con cascade (residual #4)

### `[GBY-4073] FK_RESTRICT_BLOCKS_UPDATE`
El UPDATE quiere cambiar la PK pero hay hijos con FK `ON UPDATE RESTRICT` o `NO ACTION`. Borrar primero los hijos, o redeclarar la FK con `ON UPDATE CASCADE` / `SET NULL` / `SET DEFAULT` (requiere recrear la tabla en esta versión).

### `[GBY-4074] FK_UPDATE_CASCADE_AFFECTS_CHILD_PK`
La cascada `ON UPDATE CASCADE` propagaría el cambio a una columna que es PK del hijo. Como el motor evita corrupción de PKs por efecto colateral, se aborta. Romper la cadena: usar `SET NULL` en la FK del hijo o resolver la actualización manualmente en dos pasos.

### `[GBY-4008] UPDATE_PK_NOT_ALLOWED`
Vivo únicamente en el path `UPSERT ... DO UPDATE SET pk = ...`. El UPDATE regular permite cambiar la PK desde residual #4; mover el `SET pk = ...` a una sentencia `UPDATE` plana.

---

## 👁️ Errores de vistas (bloque V)

### `[GBY-4075] VIEWS_ARE_READONLY`
Se intentó `INSERT`/`UPDATE`/`DELETE`/`TRUNCATE` sobre una vista. Las vistas son read-only en esta versión. Mutar la tabla base.

### `[GBY-4076] VIEW_DEPTH_EXCEEDED`
La expansión de una vista referenció otra vista que referenció otra... más allá de `MAX_VIEW_DEPTH = 32`. Suele indicar un ciclo (`A` usa `B`, `B` usa `A`). Romper el ciclo con `DROP VIEW`.

### `[GBY-4077] NAME_ALREADY_EXISTS`
`CREATE VIEW v` con `v` ya tomado por una tabla (o viceversa). Renombrar la nueva vista o `DROP` el objeto previo.

### `[GBY-4078] VIEW_SOURCE_NOT_SIMPLE_SELECT`
La fuente de `CREATE VIEW v AS ...` no es un SELECT simple (e.g. `UNION`, `VALUES`). Reescribir como SELECT plano; si se necesita combinar, hacerlo en el SELECT que consulta la vista.

---

## 🟢 Necesito saber si el producto está sano

### Checklist corto
```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
docker build -t gabysql .
```

Si eso pasa y `/health` responde, la base actual del producto está íntegra para su alcance actual.
