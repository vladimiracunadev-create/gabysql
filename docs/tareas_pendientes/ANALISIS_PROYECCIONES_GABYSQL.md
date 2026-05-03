# 📌 Análisis de proyecciones y mejoras de `gabysql`

> **Fuente**: `gabysql_roadmap_rdbms.pdf` + `tabla_desafios_bases_datos_priorizada_gabysql.pdf`  
> **Objetivo**: traducir ambos documentos a una dirección realista para el producto actual.
>
> **Nota de actualización (2026-05-03)**: parte de los riesgos identificados como prioritarios ya fueron entregados (B+Tree real con nodos internos, hashing del catálogo estable, CRC32 por página, `UPDATE`/`DELETE` por PK, tope de conexiones del server). Ver [CHANGELOG](../../CHANGELOG.md) y [PLAN_MAESTRO_GABYSQL](../PLAN_MAESTRO_GABYSQL.md) para el estado actual; este documento se mantiene como lectura ejecutiva original.

---

## 1. 🧭 Qué dicen realmente estos documentos

Los PDFs no describen solo mejoras puntuales. Describen dos niveles distintos de ambición:

1. **Hoja de ruta hacia un RDBMS comercial**
   - desde un motor embebido MVP
   - hasta un sistema con planner, concurrencia real, seguridad, backups, drivers, observabilidad, replicación y operación formal

2. **Priorización de problemas típicos de bases de datos**
   - qué duele más en la práctica
   - qué conviene atacar primero para que el producto sea usable y estable
   - cómo aterrizar eso específicamente en `gabysql`

La lectura correcta no es “hacer todo”. La lectura correcta es: **usar estos documentos como brújula**, y convertirlos en una estrategia por fases para no romper foco.

---

## 2. 🧠 Conclusión principal

El mayor hallazgo es este:

> `gabysql` todavía no debe perseguir primero el sueño de “RDBMS gigante”. Debe consolidarse antes como **motor embebido serio tipo SQLite early-stage**.

Eso significa priorizar:
- durabilidad y recovery
- formato en disco estable
- índices correctos
- constraints básicas
- observabilidad mínima
- backups y herramientas operativas
- semántica SQL pequeña pero confiable

Y dejar para después:
- protocolo compatible con MySQL/Postgres
- replicación
- clustering
- sharding
- optimizer cost-based completo
- MVCC serio

---

## 3. 🚦 Qué debe pasar a prioridad real del producto

### P0 — crítico para salir del estado MVP

#### 1. Índices secundarios
El segundo PDF pone esto como el problema #1 por impacto/frecuencia.

**Interpretación para `gabysql`:**
- hoy la PK cubre demasiado
- si no aparecen índices secundarios, el motor se queda limitado a demos simples
- esto es probablemente la mejora funcional más importante del siguiente ciclo

**Recomendación**:
- índices secundarios simples primero
- luego índices compuestos
- métricas de uso de índices más adelante

#### 2. Formato en disco versionado y estable
El roadmap RDBMS lo sube muy arriba por una razón correcta.

**Interpretación para `gabysql`:**
- sin compatibilidad/versionado del file format, cualquier evolución del motor es frágil
- esto es base para recovery, tooling, upgrades y soporte

**Recomendación**:
- especificación v1 explícita del formato
- política de compatibilidad
- migrador básico o al menos detector claro de versión

#### 3. WAL más robusto + crash tests + checksums
Ambos PDFs convergen aquí: la recuperación ante crash y la corrupción son críticas.

**Interpretación para `gabysql`:**
- hoy ya existe WAL usable
- lo siguiente no es “más features SQL” sino endurecer recovery

**Recomendación**:
- checksums por página o frame
- crash tests automatizados
- `integrity_check`
- herramienta de replay/diagnóstico

#### 4. Constraints básicas reales
No basta con PK única.

**Recomendación**:
- `NOT NULL`
- `DEFAULT`
- `UNIQUE`
- casts básicos y mejor semántica de `NULL`

---

## 4. 🔥 Qué debe ser prioridad alta después del bloque crítico

### P1 — alto valor para volverlo usable de verdad

#### 1. Observabilidad mínima
El segundo PDF lo deja muy arriba y tiene sentido total.

**Qué debería incluir `gabysql`:**
- logs estructurados del server
- métricas simples: latencia, errores, scans, cache hit más adelante
- endpoint `/metrics` en fase posterior
- base para troubleshooting real

#### 2. Backup / restore / integrity tools
Un motor sin respaldo confiable no es producto serio.

**Recomendación**:
- backup offline formal
- restore verificado
- comando `integrity_check`
- runbook claro de recuperación

#### 3. Locking menos global y mejor concurrencia
El roadmap largo habla de 2PL o MVCC; para `gabysql` eso todavía es demasiado si se intenta completo.

**Recomendación realista**:
- pasar de lock global a granularidad por tabla o página primero
- deadlock detection simple más adelante
- MVCC dejarlo fuera del primer bloque serio

#### 4. Suite de benchmarks y regresión de performance
Muy bien priorizado en los PDFs.

**Recomendación**:
- benchmarks de inserts, point lookup, range scan y full scan
- umbrales de regresión básicos en CI, aunque sea inicialmente informativos

#### 5. Tipos temporales y UTC
El segundo PDF acierta en que fecha/hora siempre trae bugs.

**Recomendación**:
- normalizar criterio UTC
- definir semántica exacta de `DATE` y `DATETIME`
- agregar funciones temporales básicas más adelante

#### 6. Defaults seguros y configuración simple
Muy importante si el server va a crecer.

**Recomendación**:
- límites de conexiones si el modo server evoluciona
- defaults seguros
- perfiles `dev` / `prod` más adelante

---

## 5. 🏗️ Qué debe ir a mediano plazo, no antes

### P2 — importante, pero no es el siguiente paso

#### 1. EXPLAIN / ANALYZE y estadísticas básicas
Esto sí tiene sentido, pero después de tener índices secundarios y scans más maduros.

#### 2. `ORDER BY`, `GROUP BY`, joins y subqueries
Son valiosos, pero deben llegar cuando:
- el row store esté más firme
- haya más de un índice útil
- exista al menos un planner básico

#### 3. Connection pool y límites en modo server
Tiene sentido si `gabysql-server` empieza a ser usado como servicio real, no solo como puente HTTP local.

#### 4. Semántica de `NULL`, casts y collations
Muy necesarios, pero su impacto es más de madurez y corrección que de supervivencia inmediata del producto.

---

## 6. 🧱 Qué debe quedar explícitamente fuera del foco temprano

### P3 / horizonte largo

Estas ideas son correctas, pero hoy deben tratarse como horizonte, no backlog inmediato:
- MVCC completo
- protocolo wire compatible con MySQL o Postgres
- replicación basada en WAL
- clustering / HA
- sharding / particionado
- procedimientos almacenados
- multi-tenancy seria
- cifrado completo en reposo y stack enterprise de authz/audit exhaustivo

No porque no sean valiosas, sino porque **matarían foco** en esta etapa.

---

## 7. 🗺️ Traducción recomendada a fases reales de `gabysql`

### Fase A — consolidación del motor base
- file format versionado
- checksums
- WAL endurecido
- crash tests
- `integrity_check`
- `UPDATE` / `DELETE` por PK
- `NOT NULL`, `DEFAULT`, `UNIQUE`

### Fase B — usabilidad técnica real
- índices secundarios
- backup / restore
- benchmarks
- logs estructurados
- mejor semántica de tipos y `NULL`
- locking por tabla/página

### Fase C — consultas más competitivas
- `ORDER BY`
- planner por reglas
- `EXPLAIN`
- estadísticas básicas
- `GROUP BY`
- joins seleccionados

### Fase D — evolución server/producto
- límites de conexiones
- authz más serio
- auditoría
- métricas `/metrics`
- tooling operativo más rico

### Fase E — horizonte RDBMS
- MVCC
- protocolo wire
- replicación
- HA
- clustering

---

## 8. ✅ Qué tomar como mandato inmediato

Si hay que convertir ambos documentos en una lista de ejecución inmediata para el producto actual, mi recomendación es esta:

1. **Versionar el formato en disco y endurecer el WAL**
2. **Agregar checksums, crash tests e `integrity_check`**
3. **Implementar `UPDATE`, `DELETE` y constraints básicas**
4. **Agregar índices secundarios**
5. **Construir backup/restore y benchmarks básicos**
6. **Añadir logs estructurados y primeras métricas del server**

Ese bloque sí cambia de forma real la percepción del producto: pasa de “motor MVP interesante” a “base técnica seria y defendible”.

---

## 9. 🎯 Decisión estratégica recomendada

Los dos PDFs son valiosos, pero deben leerse así:

- **PDF 1** = visión de largo plazo si `gabysql` quiere aspirar a RDBMS comercial
- **PDF 2** = priorización práctica de lo que más duele y más conviene resolver antes

Para el estado actual del repositorio, el **PDF 2 debe pesar más en el próximo ciclo**.

En otras palabras:
- primero haz que `gabysql` sea **sólido, operable y confiable**
- después hazlo **más amplio**
- y solo mucho después intenta hacerlo **gigante**
