# BEGINNERS GUIDE

## Objetivo
En 10 minutos deberías poder:
- crear una base
- crear una tabla
- insertar datos
- consultar por CLI
- levantar el server HTTP
- abrir `phpgabyadmin`

## Opción rápida con Docker
```powershell
docker compose up -d --build
```

Luego abre:
- `http://localhost:8080/health`
- `http://localhost:8000/phpgabyadmin/`

## Opción rápida nativa
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

### 6. Levantar API
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
```

### 7. Abrir admin web
En otra terminal:
```powershell
php -S localhost:8000 -t web
```

Abre `http://localhost:8000/phpgabyadmin/`.

## Qué revisar si algo falla
- build nativo en Windows: revisa [../TROUBLESHOOTING.md](../TROUBLESHOOTING.md)
- endpoints HTTP: revisa [API.md](./API.md)
- instalación: revisa [../INSTALL.md](../INSTALL.md)

## Siguiente paso recomendado
Después del primer recorrido, sigue con:
- [ARCHITECTURE.md](./ARCHITECTURE.md)
- [TECHNICAL_SPECS.md](./TECHNICAL_SPECS.md)
- [../RUNBOOK.md](../RUNBOOK.md)
