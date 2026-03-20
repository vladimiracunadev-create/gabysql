# Examples

Este directorio contiene clientes mínimos para probar `gabysql` por CLI y por HTTP.

## Estructura
- `examples/python/client_cli.py`: invoca el binario `gabysql`.
- `examples/python/client_http.py`: consume `gabysql-server` por HTTP.
- `examples/php/client_cli.php`: invoca el binario `gabysql`.
- `examples/php/client_http.php`: consume `gabysql-server` por HTTP.

## 1. HTTP/JSON (`gabysql-server`)
Levanta el server:

```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
# o multi-db:
cargo run --release --bin gabysql-server -- -dir .\dbs -addr :8080
```

### Python (HTTP)
```powershell
python examples/python/client_http.py
```

### PHP (HTTP)
```powershell
php examples/php/client_http.php
```

## 2. CLI
Compila primero el binario:

```powershell
cargo build --release --bin gabysql
```

Luego:
```powershell
python examples/python/client_cli.py
php examples/php/client_cli.php
```

## Nota para Windows
Si usas los ejemplos CLI en Windows, recuerda apuntar al binario generado en `target\release\gabysql.exe`.

## Nota para Docker
Los ejemplos HTTP asumen que el server escucha en `http://localhost:8080`.
Si levantaste el stack con `docker compose`, esa URL ya coincide con el mapeo por defecto.
