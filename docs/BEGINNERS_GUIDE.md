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

### 3. Crear tabla
```powershell
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, active BOOL);"
```

### 4. Insertar datos
```powershell
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,name,active) VALUES (1,'Ana',TRUE);"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,name,active) VALUES (2,'Beto',FALSE);"
```

### 5. Consultar
```powershell
cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users;"
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

### 7. Levantar API
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
```

### 8. Abrir el modelador ER
```powershell
# Si ya tienes php -S corriendo o docker compose up:
# http://localhost:8000/modeler/
```

`gabymodeler` te deja diseñar entidades drag&drop y exportar el DDL completo (`CREATE DATABASE` + `CREATE TABLE` + `CREATE INDEX`) listo para pegar en `phpgabyadmin`. Click `📦 Cargar ejemplo` para ver el formato esperado.

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
