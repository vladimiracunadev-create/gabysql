# 🧭 BEGINNERS GUIDE

> **Recorrido corto para levantar `gabysql` y entender su flujo base sin perderte en el detalle.**

---

## 🎯 Objetivo

En 10 minutos deberías poder:
- crear una base
- crear una tabla
- insertar datos
- consultar por CLI
- levantar el server HTTP
- abrir `phpgabyadmin`

---

## 🐳 Opción rápida con Docker

```powershell
docker compose up -d --build
```

Luego abre:
- `http://localhost:8080/health`
- `http://localhost:8000/phpgabyadmin/`

---

## 💻 Opción rápida nativa

### 1. Compilar
```powershell
cargo build --release --bin gabysql --bin gabysql-server
```

### 2. Crear la base
```powershell
cargo run --release --bin gabysql -- init demo.db
```

### 3. Crear tabla con constraints
```powershell
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, email TEXT NOT NULL UNIQUE, name TEXT, active BOOL DEFAULT TRUE);"
```

> Acabás de declarar `NOT NULL`, `UNIQUE` y `DEFAULT` inline. El motor los enforza al `INSERT`/`UPDATE`. Para FOREIGN KEYs ver paso 7b.

### 4. Insertar datos
```powershell
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,email,name) VALUES (1,'ana@x','Ana');"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,email,name,active) VALUES (2,'beto@x','Beto',FALSE);"
```

### 5. Consultar
```powershell
cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users;"
cargo run --release --bin gabysql -- exec demo.db "SELECT id,name FROM users ORDER BY name ASC LIMIT 10;"
```

### 6. Modificar y borrar
```powershell
cargo run --release --bin gabysql -- exec demo.db "UPDATE users SET name = 'Ana M' WHERE id = 1;"
cargo run --release --bin gabysql -- exec demo.db "DELETE FROM users WHERE id = 2;"
```

> `UPDATE` y `DELETE` siempre se filtran por la PK. Esta versión rechaza filtros por otras columnas y no permite cambiar la PK con `UPDATE`.

### 6b. Índices secundarios — buscar por columna no-PK
```powershell
cargo run --release --bin gabysql -- exec demo.db "CREATE INDEX idx_users_name ON users (name);"
cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users WHERE name = 'Ana M';"
```

Con el índice creado, los `SELECT WHERE name = ...` ya no requieren full scan.

### 7. Tabla hija con FOREIGN KEY
```powershell
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE orders (id INT PRIMARY KEY, user_id INT REFERENCES users(id) ON DELETE CASCADE, total FLOAT, tries INT DEFAULT 0);"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO orders (id,user_id,total) VALUES (10,1,99.5);"
```

Ahora `DELETE FROM users WHERE id = 1` arrastra automáticamente la order 10 (cascade). Sin la FK, podrías borrar el usuario y dejar orders huérfanas.

### 7c. Cruzar dos tablas con `JOIN`
```powershell
cargo run --release --bin gabysql -- exec demo.db "SELECT u.name, o.total FROM users u INNER JOIN orders o ON u.id = o.user_id;"
```

- `users u` y `orders o` son aliases — más cortos que repetir el nombre completo.
- `u.id` / `o.user_id` son **columnas cualificadas**: en un `JOIN` cualquier columna que existe en más de una tabla **debe** ir cualificada.
- Hay 4 kinds: `INNER` (default; descarta filas sin match), `LEFT JOIN` (conserva todas las del izq con NULL en el der sin match), `RIGHT`, `FULL OUTER`. Y `CROSS JOIN` para el producto cartesiano.
- Si la columna del `ON` es la PK o tiene índice del lado derecho (como acá: `users.id` es PK), el motor hace lookup directo en vez de full scan — sin que tengas que pedirlo.

### 7d. Filtrar con subqueries
```powershell
cargo run --release --bin gabysql -- exec demo.db "SELECT name FROM users WHERE id IN (SELECT user_id FROM orders);"
cargo run --release --bin gabysql -- exec demo.db "SELECT id, name FROM users WHERE EXISTS (SELECT id FROM orders WHERE user_id = users.id);"
```

- `IN (SELECT …)` y `= (SELECT …)` escalar: la subquery se ejecuta una sola vez.
- `EXISTS (SELECT … WHERE inner = outer.col)`: subquery correlacionada — se re-evalúa por cada fila del outer. Útil para "padres que tienen al menos un hijo".

### 7b. Verificar consistencia
```powershell
cargo run --release --bin gabysql -- exec demo.db "INTEGRITY CHECK;"
```

Sweep operacional: valida CRCs, decodifica filas, chequea índices y FKs. Devuelve `OK · ...` o lista los hallazgos.

### 8. Levantar API
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
```

### 9. Abrir el modelador ER
```powershell
# Si ya tienes php -S corriendo o docker compose up:
# http://localhost:8000/modeler/
```

`gabymodeler v2` (PowerDesigner-style) te deja diseñar entidades drag&drop con todas las constraints (PK, NOT NULL, UNIQUE, DEFAULT, FK con ON DELETE) y exportar el DDL completo. También importa schemas existentes desde el server real con **↘ Importar de gabysql**. Manual completo con screenshots: [web/modeler/USER_MANUAL.md](../web/modeler/USER_MANUAL.md).

### 9. Abrir admin web
En otra terminal:
```powershell
php -S localhost:8000 -t web
```

Abre `http://localhost:8000/phpgabyadmin/`.

---

## 🧩 Qué revisar si algo falla

- build nativo en Windows: [TROUBLESHOOTING](../TROUBLESHOOTING.md)
- endpoints HTTP: [API](API.md)
- instalación: [INSTALL](../INSTALL.md)

---

## 📍 Siguiente paso recomendado

Después del primer recorrido, sigue con:
- [ARCHITECTURE](ARCHITECTURE.md)
- [TECHNICAL_SPECS](TECHNICAL_SPECS.md)
- [RUNBOOK](../RUNBOOK.md)
