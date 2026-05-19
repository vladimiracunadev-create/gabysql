# 🚨 Manejo de errores en gabysql

> **Cómo se modelan, escriben, traducen y propagan los errores. Las reglas que siguen las ~212 ocurrencias de `DbError::new(...)` en el repo.**

Este documento es **normativo**: cualquier `DbError::new(...)` agregado al motor, al server, al CLI o a los módulos auxiliares debe respetar estas reglas. Pull requests que introducen mensajes pobres o en otro idioma se rechazan en review.

---

## 1. Filosofía

Un error en `gabysql` es **una experiencia para un operador humano** (o para un agente que lo va a leer textualmente). No es solo un código de salida. Tiene que poder responder tres preguntas, en este orden:

1. **¿QUÉ pasó?** — la operación que falló, con el nombre concreto del objeto involucrado.
2. **¿POR QUÉ?** — el dato preciso que disparó el fallo (valor, PK, offset, versión, conteo).
3. **¿CÓMO se resuelve?** — cuando el remedio es conocido y razonable, el mensaje lo incluye explícitamente.

No todos los errores tienen las tres capas. Un error de corrupción profunda solo puede dar "qué" + "por qué" (el "cómo" es "restaurar de backup", y eso vive en `RUNBOOK.md` y `TROUBLESHOOTING.md`). Un error de validación de input casi siempre puede dar las tres.

**Mal mensaje** (real, ya arreglado): `"db vacío"`. No dice qué operación, no dice qué se esperaba, no orienta.
**Buen mensaje**: `"falta el parámetro 'db' en la query string (ej: ?db=demo.db)"`.

---

## 2. Tipo único, sin variantes

[src/lib.rs:12-58](../src/lib.rs:12). Un solo `DbError` con un solo campo `message: String`. Implementa `std::error::Error` y `Display`, tiene `impl From<...>` para `std::io::Error`, `ParseIntError`, `ParseFloatError` y `FromUtf8Error`.

```rust
pub type DbResult<T> = Result<T, DbError>;

pub struct DbError {
    message: String,
}
```

**Por qué no hay `enum DbErrorKind` todavía**: el motor es chico, los call sites son visuales, y agregar variantes obliga a un mapeo discriminado en cada propagación que hoy no aporta. **Cuándo introducirlo**: si aparece la necesidad de que un cliente reaccione programáticamente al *tipo* de error (no a su texto), o si los handlers del server crecen al punto de necesitar un mapeo automático kind → status HTTP. Mientras tanto, la disciplina de los mensajes es la línea de defensa.

---

## 3. Reglas de estilo (obligatorias)

### 3.1. Idioma: español

**Todos los mensajes son en español.** Sin excepción. Esto es producto, no convención.

- ❌ `"tx already started"`
- ✅ `"transacción ya iniciada"`

- ❌ `"page too small"`
- ✅ `"página demasiado pequeña: tiene {n} bytes, requiere al menos {expected}"`

Los identificadores técnicos en inglés (`SELECT`, `WHERE`, `PRIMARY KEY`, `NOT NULL`, `FOREIGN KEY`, nombres de columnas SQL) **se mantienen en inglés** — son lenguaje SQL, no traducible.

### 3.2. Capitalización y puntuación

- Empieza en **minúscula** (a menos que la primera palabra sea un identificador SQL que va en mayúscula: `PRIMARY KEY`, `WHERE`, etc.).
- **Sin punto final** salvo cuando el mensaje tiene más de una oración (la primera oración cierra con punto; el resto del mensaje agrega contexto o hint).
- Sin signo de exclamación. Sin emojis.

```
✅ "tabla no existe: orders"
✅ "violación de UNIQUE en índice 'uq_users_email' (PK existente: 42)"
✅ "refusing to overwrite existing database: demo.db. Use --force to overwrite."
❌ "Tabla No Existe."
❌ "ERROR: tabla no existe!"
```

> El último ejemplo de "Buen mensaje" arriba tiene texto en inglés porque es el mensaje literal que escribió alguien previo a esta guía. Está marcado para traducción en el sweep del bloque actual (ver §10).

### 3.3. Incluir nombres concretos

El mensaje **debe** incluir el nombre del objeto que falló: tabla, columna, índice, PK, archivo, status, etc. Si el dato está en scope, va al mensaje.

```rust
❌ DbError::new("columna no existe")
✅ DbError::new(format!("columna '{}' no existe en tabla '{}'", col, meta.name))
```

### 3.4. Datos concretos sobre el "por qué"

Cuando el fallo es por un valor fuera de rango, un tamaño inesperado, un offset, una versión: **incluir el dato**, no solo el síntoma.

```rust
❌ DbError::new("string corrupto")
✅ DbError::new(format!("string serializado corrupto en offset {} (esperaba {} bytes, hay {})", offset, expected, actual))

❌ DbError::new("fila corrupta (INT)")
✅ DbError::new(format!("fila pk={} corrupta: campo '{}' (INT) esperaba 8 bytes, hay {}", pk, col, len))

❌ DbError::new("LIMIT debe ser >= 0")
✅ DbError::new(format!("LIMIT debe ser >= 0; recibí {}", n))
```

### 3.5. Sugerir el remedio cuando es conocido

Si el error tiene una resolución estándar y razonable, **incluirla** en el mensaje. Esto es lo que mata el síntoma "errores pobres que no aclaran nada".

```rust
✅ "refusing to overwrite existing database: demo.db. Use --force to overwrite."
✅ "unsupported gabysql file format: version=6 (expected 7). Re-create the database with the current binary."
✅ "database is locked by another process: demo.db. Close the other gabysql process or wait for it to release the lock."
```

Cuando el remedio es "consultar el manual", **no lo digas en el mensaje** — el operador ya sabe que existe un manual. En cambio, escribe el mensaje para que sea buscable en `TROUBLESHOOTING.md` con un copy-paste del texto.

### 3.6. Limitaciones declaradas — decirlo explícitamente

Si el motor no soporta una operación, el mensaje **lo dice** + **menciona la alternativa**:

```rust
✅ "WHERE BETWEEN sobre '{col}': el índice secundario es hash-based (equality only). Solo columnas INT-indexadas admiten BETWEEN."
✅ "WHERE solo soporta PK ({pk_name}) o columnas con índice secundario; '{col}' no está indexada"
```

---

## 4. Las 8 categorías canónicas

Estos son los grupos en los que cae todo error del motor, con el patrón de mensaje recomendado para cada uno. Si tu nuevo error no encaja en ninguno, abrí un Issue antes de inventar un noveno.

### 4.1. Validación de input (parser, schema, identificadores)

**Patrón**: `"<concepto SQL> <restricción>: <dato concreto>"` o `"<operación> rechazada: <razón>"`.

```rust
"identificador '{}' excede el máximo de 64 caracteres (tiene {})"
"identificador '{}' es palabra reservada del motor"
"CREATE TABLE rechazado: tipo de columna inválido '{}'"
"LIMIT debe ser >= 0; recibí {}"
```

### 4.2. Resolución de nombre (NotFound)

**Patrón**: `"<objeto> no existe: <nombre>"` o `"<objeto> '{}' no existe en <contexto>"`.

```rust
"tabla no existe: orders"
"columna 'email' no existe en tabla 'users'"
"índice no existe: idx_users_email"
"FK rota: tabla 'orders' no expone su PK 'id'"
```

### 4.3. Conflicto / duplicado

**Patrón**: `"<objeto> '{}' ya existe en <contexto>"` o `"violación de <constraint>: <detalle>"`.

```rust
"ya existe un índice llamado '{}' en la tabla '{}'"
"violación de UNIQUE en índice '{}' (PK existente: {})"
"PRIMARY KEY duplicada: pk={} ya está en la tabla"
```

### 4.4. Restricción / constraint violado

**Patrón**: `"<constraint> rechazado: <contexto> ({dato concreto})"`.

```rust
"NOT NULL rechazado en columna '{}' (tabla '{}')"
"FOREIGN KEY '{}.{}' apunta a tabla inexistente '{}'"
"ON DELETE RESTRICT: la fila pk={} tiene {} hijos en '{}'"
```

### 4.5. Limitación deliberada del motor

**Patrón**: `"<operación> sobre <objeto>: <limitación>. <alternativa>"`.

Ver §3.6 arriba.

### 4.6. Integridad / corrupción

**Patrón**: `"<objeto> corrupto en <ubicación>: <dato>"`. Para corrupción a-priori unrecoverable, sugerir `INTEGRITY CHECK` o restore.

```rust
"página {} corrupta: CRC32 esperado={:08x}, calculado={:08x}"
"WAL record en offset {} corrupto: CRC32 inválido"
"unsupported gabysql file format: version={} (expected {}). Re-create the database with the current binary."
```

### 4.7. Estado interno inconsistente (bugs detectables)

**Patrón**: `"<invariante> roto: <detalle>"`. Estos no deberían pasar nunca; si pasan, hay un bug. Mensaje detallado para diagnóstico.

```rust
"page cache inconsistente: la página {} marcada como cargada no está en el HashMap"
"root page es 0: el catálogo no registró un root válido"
```

### 4.8. I/O del sistema operativo

**Patrón**: se delega a `From<io::Error>` (lib.rs:35). Si necesitamos contexto adicional (path, qué se intentaba), envolvemos:

```rust
✅ .map_err(|e| DbError::new(format!("no se pudo abrir '{}': {}", path.display(), e)))
```

---

## 5. Mapeo a HTTP

[src/server.rs](../src/server.rs). Las funciones de handler eligen el status según la categoría:

| Categoría (§4) | Status HTTP | Cuerpo |
|---|---|---|
| 4.1 Validación de input | `400 Bad Request` | `{"ok":false,"error":"<mensaje>"}` |
| 4.2 Resolución de nombre | `404 Not Found` (si es el path) / `400` (si es body) | idem |
| 4.3 Conflicto | `409 Conflict` | idem |
| 4.4 Constraint | `400 Bad Request` | idem |
| 4.5 Limitación | `400 Bad Request` | idem |
| 4.6 Corrupción | `500 Internal Server Error` | idem (porque el server no sabe restaurar) |
| 4.7 Bug interno | `500 Internal Server Error` | idem |
| 4.8 I/O | `500 Internal Server Error` | idem |
| Auth fallida | `401 Unauthorized` | `{"ok":false,"error":"unauthorized"}` |
| Server saturado | `503 Service Unavailable` | `{"ok":false,"error":"server busy: ..."}` |
| Método inválido | `405 Method Not Allowed` | text |

**Catch-all**: cualquier `DbError` que escape al handler termina en 500. Si tu handler **sabe** que es una validación, lo mapea a 400 explícito antes de propagar.

---

## 6. Mapeo a CLI

[src/bin/gabysql.rs:9-13](../src/bin/gabysql.rs:9). Una sola política:

```rust
fn main() {
    if let Err(err) = run() {
        eprintln!("error: {}", err);
        std::process::exit(1);
    }
}
```

Texto plano a `stderr`, exit code `1`. Sin formato JSON, sin colores, sin códigos diferenciados. Esto es deliberado: la CLI es un binario embebible en shell scripts; el caller decide cómo procesar.

> **No usar `panic!` en flujos esperables.** `panic!` aborta sin pasar por `eprintln!` y sin exit code 1 limpio. `unwrap()` solo en código que matemáticamente no puede fallar.

---

## 7. Rollback automático en transacciones

Todos los flujos que abren transacción siguen este patrón ([src/sql.rs::run_exec](../src/sql.rs), todos los handlers del server):

```rust
pager.begin()?;
let result = (|| -> DbResult<_> {
    // ... ejecutar ...
    pager.commit()?;
    Ok(out)
})();
match result {
    Ok(rs)  => Ok(rs),
    Err(e)  => { let _ = pager.rollback(); Err(e) }
}
```

**No existe** un escenario válido donde un error deja la transacción "abierta a medias". Si tu nuevo flujo abre `begin()`, debe garantizar `commit()` o `rollback()` antes de retornar.

---

## 8. Anti-patrones

Lista explícita de cosas que **no** hacer:

### 8.1. Mensajes de una palabra

```rust
❌ "corrupto"
❌ "inválido"
❌ "vacío"
❌ "error"
```

Cero contexto. Cero remedio. Imposibles de buscar en troubleshooting.

### 8.2. `unwrap()` / `expect()` que mienten

```rust
❌ data[offset..].try_into().unwrap()    // panic si offset overflowea
✅ data[offset..offset+8].try_into().map_err(|_| DbError::new(format!("buffer corto en offset {}", offset)))?
```

Si el caller pasa un buffer corto, el motor debe responder con un `DbError`, no con un panic.

### 8.3. Errores anónimos en `From`

```rust
❌ impl From<io::Error> for DbError {
       fn from(e: io::Error) -> Self { Self::new("io error") }
   }
✅ impl From<io::Error> for DbError {
       fn from(e: io::Error) -> Self { Self::new(e.to_string()) }
   }
```

El `From` por defecto **debe** propagar el detalle. Si se quiere agregar contexto, se envuelve con `.map_err(...)` en el call site, no se enmascara el `From`.

### 8.4. Inglés mezclado con español sin razón

```rust
❌ "tx already started: la transacción ya está abierta"
```

Decidí un idioma — para gabysql es español — y mantenelo.

### 8.5. Mensajes que dependen del estado de planning

```rust
❌ "TODO: implementar UPDATE por columna no-PK"
❌ "FIXME: este path debería rechazar antes de llegar acá"
```

Si el motor no implementa algo, el mensaje habla en términos del **usuario**, no del **equipo de desarrollo**.

### 8.6. Errores que cuentan secretos

Nunca incluir en el mensaje:
- Tokens, contraseñas, claves de API.
- Path completo del servidor (`/home/admin/secret/...`) en respuestas HTTP; sí es OK en logs locales.
- Stack traces (no aplica hoy porque no usamos `anyhow`/`color-eyre`, pero queda asentado).

---

## 9. Checklist para revisar un PR que toca errores

Cuando se agrega o modifica un mensaje, antes de mergear:

- [ ] **Idioma**: ¿está en español? (excepto identificadores SQL).
- [ ] **Capitalización**: ¿empieza en minúscula, sin punto final salvo que tenga frase HOW?
- [ ] **Nombre concreto**: ¿incluye el identificador del objeto (tabla/columna/índice/PK/path)?
- [ ] **Dato del fallo**: ¿incluye el valor, tamaño, offset, status, versión que disparó el error?
- [ ] **Remedio**: si existe una resolución estándar, ¿la dice el mensaje?
- [ ] **Categoría**: ¿encaja en una de las 8 de §4? Si no, ¿realmente la necesitamos?
- [ ] **HTTP**: si el handler la propaga, ¿está mapeada al status correcto (§5)?
- [ ] **Test**: si el mensaje es accionable por un cliente, ¿hay un test que asegure el texto contiene la palabra clave?
- [ ] **Sin secretos**: ¿no expone tokens, paths internos sensibles, ni stack traces?

---

## 10. Estado actual (mayo 2026)

Tras el sweep aplicado junto con esta guía:

- ✅ Todos los mensajes en producción están en **español**.
- ✅ Los mensajes egregios de una palabra (`"db vacío"`, `"string corrupto"`, `"fila corrupta (INT)"`) recibieron contexto.
- ✅ Los mensajes en inglés heredados (`"tx already started"`, `"page too small"`, `"database is locked by another process: …"`, etc.) están traducidos.
- ✅ Tests de integración que asertaban sobre el texto original están actualizados.
- 🟡 No hay `enum DbErrorKind` todavía. Cuando se introduzca, vendrá con un mapeo automático kind→HTTP que reemplazará al mapeo manual en cada handler.
- 🟡 No hay códigos numéricos / SQLSTATE. Sería un bump no-trivial al contrato externo; si llega, viene con su propio ADR.

---

## 11. Referencias

- [src/lib.rs](../src/lib.rs) — definición de `DbError` y `DbResult`.
- [src/server.rs](../src/server.rs) — mapeo HTTP, helpers `Response::json/text`.
- [src/bin/gabysql.rs](../src/bin/gabysql.rs) — política CLI.
- [TROUBLESHOOTING.md](../TROUBLESHOOTING.md) — el operador entra acá con un mensaje exacto del motor.
- [RUNBOOK.md](../RUNBOOK.md) — procedimientos de recovery cuando el error apunta a corrupción.
- [CONTRIBUTING.md](../CONTRIBUTING.md) — link a este documento como parte de la revisión.
