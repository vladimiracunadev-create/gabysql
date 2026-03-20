# 🧪 GABYBENCH SPEC

> **Base de datos canónica de prueba para `gabysql`: validación funcional, regresión y comparación contra otros motores.**

---

## 🎯 Objetivo

`gabybench` debe ser la base estándar para:
- pruebas funcionales repetibles
- benchmarks reproducibles
- regresión de performance
- comparación con SQLite, PostgreSQL, MySQL/MariaDB y DuckDB

---

## 🧱 Principios del dataset

- schema suficientemente realista, pero no innecesariamente complejo
- consultas que reflejen el roadmap real de `gabysql`
- tamaños escalables `S`, `M`, `L`
- datos determinísticos cuando sea posible
- exportables o reproducibles desde scripts

---

## 🗂️ Schema lógico recomendado

### `customers`
- `id INT PRIMARY KEY`
- `name TEXT`
- `email TEXT`
- `country TEXT`
- `created_at DATETIME`

### `products`
- `id INT PRIMARY KEY`
- `sku TEXT`
- `name TEXT`
- `category TEXT`
- `price FLOAT`
- `active BOOL`

### `orders`
- `id INT PRIMARY KEY`
- `customer_id INT`
- `status TEXT`
- `order_date DATETIME`
- `total FLOAT`

### `order_items`
- `id INT PRIMARY KEY`
- `order_id INT`
- `product_id INT`
- `quantity INT`
- `unit_price FLOAT`

### `events`
- `id INT PRIMARY KEY`
- `entity_type TEXT`
- `entity_id INT`
- `event_type TEXT`
- `created_at DATETIME`
- `payload JSON`

---

## 📏 Escalas recomendadas

| Escala | customers | products | orders | order_items | events |
|---|---:|---:|---:|---:|---:|
| `S` | 1,000 | 500 | 10,000 | 30,000 | 20,000 |
| `M` | 10,000 | 5,000 | 100,000 | 300,000 | 200,000 |
| `L` | 100,000 | 20,000 | 1,000,000 | 3,000,000 | 2,000,000 |

> [!NOTE]
> `L` no debe exigirse al comienzo. Para las primeras fases bastan `S` y `M`.

---

## 🔍 Cargas a medir

## OLTP base
- crear schema
- carga inicial batch
- insert de una fila
- update por PK
- delete por PK
- point lookup por PK
- range scan por PK

## Consulta operativa
- filtro por estado
- filtro por fecha
- ordenamiento simple
- paginación `LIMIT/OFFSET`
- filtro por columna secundaria cuando exista índice

## Integridad y operación
- tiempo de apertura de DB
- tiempo de `integrity_check`
- tiempo de backup
- tiempo de restore
- recovery tras crash simulado

---

## ⏱️ Métricas a registrar

| Métrica | Unidad |
|---|---|
| latencia por comando | ms |
| throughput batch insert | rows/s |
| tiempo de open/init | ms |
| tiempo de backup/restore | s |
| tamaño de `.db` | MB |
| tamaño de `.wal` durante carga | MB |
| memoria del proceso cuando sea medible | MB |

---

## 🧪 Comandos de medición

### PowerShell
```powershell
$time = Measure-Command { cargo run --release --bin gabysql -- exec gabybench.db "SELECT * FROM orders WHERE id = 5000;" }
$time.TotalMilliseconds
```

### Linux / macOS
```bash
/usr/bin/time -f "%E real, %M KB" cargo run --release --bin gabysql -- exec gabybench.db "SELECT * FROM orders WHERE id = 5000;"
```

---

## 🥊 Motores de comparación

### Comparación mínima obligatoria
- SQLite
- PostgreSQL
- DuckDB

### Comparación adicional útil
- MySQL o MariaDB

---

## 📊 Cómo comparar correctamente

No basta con correr una consulta y mirar el tiempo.

Cada benchmark debe registrar:
- motor
- versión
- tamaño del dataset
- hardware/host
- comando o consulta exacta
- tiempo total
- observaciones relevantes

### Qué no hacer
- comparar consultas que `gabysql` todavía no soporta con consultas avanzadas de otro motor como si fueran equivalentes
- mezclar benchmarks de analytics con benchmarks OLTP sin etiquetarlos
- vender una comparación parcial como si fuera una equivalencia total de producto

---

## ✅ Uso esperado en el roadmap

`gabybench` debe aparecer en todas las fases críticas:
- Fase 0: baseline
- Fase 1: crash/recovery/integrity
- Fase 2: constraints y mutaciones
- Fase 3: índices y filtros
- Fase 4: backup/restore
- Fase 5: benchmarks y comparación con otros motores
