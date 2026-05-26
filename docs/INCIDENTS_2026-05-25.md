# Incidentes y fixes · sesión 2026-05-25

> **Registro completo de fallos detectados y corregidos durante la sesión maratón del 2026-05-25** (cierre de 7 bloques del roadmap + infraestructura de release + Pages + audit de seguridad).
>
> Este documento es complementario a:
> - **`CHANGELOG.md`** — cuenta qué entró nuevo (features, no bugs).
> - **`SECURITY_AUDIT_2026-05-25.md`** — solo los 5 hallazgos de seguridad con CWE.
>
> Acá viven los **bugs operativos, regresiones de CI, errores de configuración y fixes menores** que aparecieron y se cerraron en flight. Útil como:
> - Aprendizaje (qué patrones rompieron).
> - Referencia para troubleshooting (si vuelve a aparecer, buscar acá).
> - Auditoría histórica (qué commit arregló qué).

---

## 📊 Resumen ejecutivo

**16 incidentes resueltos** en 13 commits "fix" / "style". Ningún data loss. Ningún rollback en `main` necesario.

| Categoría | # | Commits |
|---|---|---|
| 🔒 Seguridad | 5 | `7245afb` |
| 🏗️ CI/CD (formatting, lints, SHAs) | 4 | `23daa7e`, `b6f8df2`, `211ee6c`, `9ce203f` |
| 🐛 Lógica de motor (HAVING, regresiones tests) | 3 | `f93b188`, `797e9b5` |
| 📄 GitHub Pages (case-sensitivity, baseurl) | 2 | `0d93e31`, `f6312f7` |
| 📚 Docs desactualizadas (sweeps reactivos) | 2 | `23daa7e` (E1+E2+E3), `df6e40e` (post-F) |

---

## 🔒 Seguridad (5 hallazgos · commit `7245afb`)

Detalle completo y PoCs en [`SECURITY_AUDIT_2026-05-25.md`](../SECURITY_AUDIT_2026-05-25.md). Resumen:

| # | Severidad | Bug | Fix |
|---|---|---|---|
| S1 | 🔴 CRÍTICO | `Content-Length` unbounded en `/exec` → memory DoS (CWE-400) | `MAX_REQUEST_BODY_BYTES = 100 MiB` + reject pre-read con `[GBY-5007]` |
| S2 | 🟠 ALTO | Token comparado con `==` → timing attack recoverea byte por byte (CWE-208) | `constant_time_eq()` con XOR + fold sin short-circuit |
| S3 | 🟠 ALTO | `phpgabyadmin` POSTs sin CSRF token (CWE-352) | Token de sesión + `csrf_field()` + `require_csrf_token()` en 5 handlers + 5 forms |
| S4 | 🟠 ALTO | `install.ps1` saltaba verificación SHA256 con `catch [WebException]` silencioso → MITM (CWE-347) | Removido el catch silencioso, fail hard si no se puede verificar |
| S5 | 🟡 MEDIO | Parser recursivo sin límite → `WHERE ((((...))))` crashea proceso (CWE-674) | `MAX_PARSE_DEPTH = 100` + counter `where_depth` en parser → `[GBY-4033]` |

---

## 🏗️ CI/CD — formatting, lints, SHAs inventados

### CI-1 · `cargo fmt --check` fallaba en CI desde el commit de E1

**Síntoma:** Después del push de E1 (`a67503f`), todos los jobs de Windows/Linux/macOS fallaban en el primer step `cargo fmt --check` con diffs sobre `IsNull { column, negated }` y otros structs/match arms.

**Causa raíz:** En la sesión yo formateé a mano (line wrapping subjetivo) sin correr `cargo fmt` localmente. Mi entorno Windows no tenía MSVC linker, así que validaba con `cargo check` que NO impone reglas de formato. CI sí.

**Fix:** Ejecutar `cargo fmt` después de cada edición. Aplicado en `23daa7e` para los 3 bloques E1/E2/E3 acumulados.

**Archivos tocados por el reformat:** `src/sql.rs` (41 ajustes), `tests/integration_test.rs` (21 ajustes).

**Lección:** `cargo check` no es sustituto de `cargo fmt --check` ni de `cargo clippy`. Toda sesión sin MSVC debe correr `cargo fmt && cargo clippy --all-targets -- -D warnings` antes de cada commit.

---

### CI-2 · `cargo clippy -D warnings` rechazó 4 patrones idiomáticos

**Síntoma:** CI verde en formato, pero falla en `cargo clippy --all-targets -- -D warnings`:

```
error: manual implementation of `Option::map`   (× 3 sitios)
error: useless use of `format!`                 (× 1 sitio)
```

**Causa raíz:** Escribí `match self.eval(...)? { Some(b) => Some(!b), None => None }` (idiomático para humanos) y `format!("texto literal sin args")`. Clippy con `-D warnings` los rechaza por preferir `.map()` y `.to_string()`.

**Sitios afectados:**
- `src/sql.rs:1999` — `WhereExpr::Not` en `eval_where_expr_joined`.
- `src/sql.rs:2276` — mismo patrón en `eval_where_expr_single`.
- `src/sql.rs:4303` — `eval_in_list` combinando `in_result` + `negated`.
- `src/sql.rs:4572` — error de `!` suelto con `format!` sin args.

**Fix:** Cambios mecánicos sugeridos por clippy. Commit `b6f8df2`. Sin cambios de comportamiento.

**Lección:** Aplicar siempre `cargo clippy --all-targets -- -D warnings` antes de cada commit. La política `-D warnings` del repo es estricta.

---

### CI-3 · SHAs inventados en actions de GitHub Pages

**Síntoma:** Workflow `Pages` falla en setup-job con:

```
##[error]Unable to resolve action `actions/deploy-pages@2533ba2dafde5172a984c0d2cd55fc824a76823a`,
unable to find version `2533ba2dafde5172a984c0d2cd55fc824a76823a`
```

**Causa raíz:** Cuando armé el workflow `pages.yml`, no resolví los SHAs reales de las acciones — los inventé. 4 SHAs no existían en sus repos:

- `actions/configure-pages@983d7736d9b0ae728b81ab479565c72886d7745b` ❌
- `actions/jekyll-build-pages@44a6e6beabd48582f863aeeb6cb2151cc1716697` ✅ (coincidencia)
- `actions/upload-pages-artifact@7b1f4a764d45c48632c6b24a0339c27f5614fb0b` ❌
- `actions/deploy-pages@2533ba2dafde5172a984c0d2cd55fc824a76823a` ❌

**Fix intermedio (`9ce203f`):** Reemplazo por `@v5`/`@v1`/`@v3`/`@v4` argumentando "son acciones oficiales del org `actions/`". Esto **rompió la política de seguridad del repo** (CI-4 abajo).

**Fix definitivo (`211ee6c`):** Resolución de los SHAs reales vía `gh api repos/<org>/<repo>/git/refs/tags/<tag>`:

```
actions/configure-pages       v6.0.0  → 45bfe0192ca1faeb007ade9deae92b16b8254a0d
actions/jekyll-build-pages    v1.0.13 → 44a6e6beabd48582f863aeeb6cb2151cc1716697
actions/upload-pages-artifact v5.0.0  → fc324d3547104276b827a68afc52ff2a11cc49c9
actions/deploy-pages          v5.0.0  → cd2ce8fcbc39b97be8ca5fce6e763baed58fa128
```

**Lección:** **Nunca** inventar SHAs. Siempre resolverlos con:

```bash
gh api repos/<org>/<repo>/git/refs/tags/<tag> --jq '.object.sha'
```

---

### CI-4 · `workflow-security.yml` exigía SHA pin estricto

**Síntoma:** Tras el "fix" CI-3 intermedio (usar `@v5`/`@v4`), el workflow `Workflow security` (que corre `zizmor` + un check propio de pin-to-SHA) empezó a fallar:

```
FAIL: 4 accion(es) sin SHA.
##[error]Accion third-party sin SHA pin: actions/configure-pages@v5
```

**Causa raíz:** El repo tiene una política propia en `.github/workflows/workflow-security.yml` que enforce pin a SHA en **todas** las third-party actions, **incluso las del org `actions/`**. Mi razonamiento "son oficiales y se pueden usar por tag" no aplicaba a la política del proyecto.

**Fix:** Ver CI-3 — los SHAs reales también arreglan esto (`211ee6c`).

**Lección:** Antes de relajar pin-to-SHA "porque es oficial", revisar `.github/workflows/workflow-security.yml`. La política del repo manda sobre el sentido común genérico.

---

## 🐛 Lógica de motor (HAVING + regresiones tests)

### M-1 · HAVING `SUM(x) > 100` devolvía `[GBY-2002] columna 'sum_x' no existe`

**Síntoma:** Tras pushear el bloque F (`cbc7586`), el test `f_having_filter_after_aggregation` falló en CI con:

```
[GBY-2002] columna 'sum_monto' no existe en 'ventas'
```

**Causa raíz:** El executor de HAVING reusaba `eval_atom_single` que validaba `meta.column(&key).is_none()` — correcto en WHERE/UPDATE/DELETE (donde la columna debe estar en el schema físico), pero rechaza claves virtuales del pipeline de agregación (`sum_monto`, `count_*`, aliases).

**Fix (`f93b188`):** Helper `ensure_column_visible(meta, key, raw, row)` que acepta la columna si está en `meta.columns` **O** ya está materializada como key en la fila. Preserva UX de "columna inexistente" en WHERE y deja pasar las virtuales en HAVING.

**Archivos:** `src/sql.rs` — 4 sitios en `eval_atom_single` refactorizados al helper.

---

### M-2 · HAVING `SUM(x) > 100` seguía fallando cuando el agregado tenía alias

**Síntoma:** Test `f_having_filter_after_aggregation` **seguía** fallando con el mismo `[GBY-2002] columna 'sum_monto'`, incluso después de M-1.

```sql
SELECT region, SUM(monto) AS total FROM ventas ... HAVING SUM(monto) > 500;
```

**Causa raíz:** El bucket se populaba solo bajo el `output_name` (= alias `total` cuando existe), pero el parser de HAVING canonicalizaba `SUM(monto)` a la clave `sum_monto`. Mismatch → fila no encontrada en el bucket → `[GBY-2002]` propagado por `ensure_column_visible` (fix anterior se aplicaba bien, pero el bucket no tenía `sum_monto`).

**Fix (`797e9b5`):** Cuando un agregado tiene alias, populamos el bucket bajo **ambas** claves: el alias (`total`) Y la clave canónica (`sum_monto`). HAVING entonces resuelve tanto vía `HAVING total > 500` como vía `HAVING SUM(monto) > 500`.

**Archivos:** `src/sql.rs` — `exec_aggregate_pipeline`.

**Lección:** Los layers separados (parser ↔ executor) tienen que acordar el formato de keys virtuales. Si el parser canonicaliza `SUM(monto)` → `sum_monto`, el executor debe almacenar bajo esa misma clave AUNQUE haya alias visible.

---

### M-3 · 6 tests pre-existentes rotos por cambios E1/E2/E3 (mensajes de error)

**Síntoma:** CI del commit E3 (`d439bb0`) tenía 6 tests rojos:

- `parser_returns_error_for_invalid_where` — esperaba `"WHERE soporta solo"`, ahora no aparece.
- `update_and_delete_by_pk_roundtrip` — esperaba error en `DELETE FROM u WHERE name = 1`, ahora pasa (0 filas borradas).
- `secondary_index_lookup_and_maintenance` — esperaba parser error en `WHERE x AND y`, ahora parsea OK (E1).
- `e3_update_by_compound_where`, `e3_update_by_in_subquery`, `e3_update_by_indexed_column_affects_all_matches` — mis propios tests E3 que usaban `SELECT ... WHERE col_no_indexed = val` (cae al fast-path indexado que exige índice).

**Causa raíz:**
- Tests pre-existentes asumían comportamiento pre-E1/E2/E3 (mensajes viejos, errores que ahora no son errores).
- Mis tests E3 nuevos no consideraron que SELECT con `=` sobre columna no-indexada aún requiere índice (eso no cambió en E3).

**Fix (incluido en `23daa7e`):**
- Tests pre-existentes: assertions actualizados a los nuevos códigos (`[GBY-4001]` con mensaje nuevo) o reescritos para reflejar el comportamiento E1/E2/E3.
- Tests E3: SELECT verificador usa `WHERE … AND id > 0` para forzar FullScan + 3VL (path post-E1 que no exige índice).

**Lección:** Cambiar un mensaje de error es una **breaking change** para tests downstream. Cuando cambia un código de error, sweep de tests pre-existentes obligatorio.

---

## 📄 GitHub Pages

### P-1 · `docs/INDEX.md` mayúscula colisionaba con `docs/index.md` en Windows

**Síntoma:** Tras crear `docs/index.md` (landing nueva), `git status` mostraba `M docs/INDEX.md` con el CONTENIDO de la landing nueva — pero ni rastro de un `docs/index.md` separado.

**Causa raíz:** Windows tiene filesystem case-insensitive. Mi `Write` a `docs/index.md` sobrescribió `docs/INDEX.md` que existía previamente. En git el archivo seguía llamándose `INDEX.md` (case del nombre tracked).

GitHub es case-sensitive: `docs/index.md` (root del sitio Pages) y `docs/INDEX.md` (índice de docs viejo) son archivos distintos. Sin `docs/index.md` real, Pages no encontraba root del sitio.

**Fix (`0d93e31`):**

```bash
git config core.ignorecase false
git mv docs/INDEX.md docs/index.md
# (el rename usó el contenido tracked viejo → reescribí el contenido nuevo)
```

**Lección:** En proyectos cross-platform, **nunca** dos archivos en el mismo directorio con el mismo nombre case-insensitive. El test rápido: `git ls-files | sort -f | uniq -d -i` debería estar vacío.

---

### P-2 · El sitio Pages cargaba sin CSS (estilo "sin formato")

**Síntoma:** Tras pushear el layout custom (`2a0774a`), el sitio en `https://vladimiracunadev-create.github.io/gabysql/` se veía sin formato, como HTML plano del navegador, ignorando el CSS.

**Diagnóstico:**

```bash
$ curl -s https://vladimiracunadev-create.github.io/gabysql/ | grep stylesheet
<link rel="stylesheet" href="/assets/css/style.css">

$ curl -sI https://vladimiracunadev-create.github.io/assets/css/style.css
HTTP/1.1 200 OK
Content-Length: 76559   # ← tamaño RARO, mi CSS es 16 KB

$ curl -sI https://vladimiracunadev-create.github.io/gabysql/assets/css/style.css
HTTP/1.1 200 OK
Content-Length: 16034   # ← mi CSS real
```

**Causa raíz:** El usuario tiene OTRO sitio de GitHub Pages publicado en `vladimiracunadev-create.github.io` (root del dominio, sin subpath). Mi `_config.yml` tenía `baseurl: ""` lo que hacía que `relative_url` emitiera `/assets/css/style.css` como **path absoluto del dominio**. El navegador lo resolvía contra el ROOT, cargando el CSS del **otro** sitio.

**Fix (`f6312f7`):** `baseurl: "/gabysql"` en `docs/_config.yml`. `relative_url` ahora emite `/gabysql/assets/css/style.css` que es la ruta real del sitio del proyecto.

**Lección:** Cuando se publica un proyecto en `<user>.github.io/<proyecto>/` y el usuario tiene además un sitio en `<user>.github.io/`, los assets con paths absolutos colisionan. `baseurl: "/<proyecto>"` es obligatorio.

---

## 📚 Docs desactualizadas (sweeps reactivos)

### D-1 · Sweep post-E1/E2/E3 (commit `23daa7e`)

**Síntoma:** Después de cerrar 3 bloques en sesión, varios archivos `.md` periféricos seguían diciendo "WHERE solo soporta `=` y `BETWEEN`", "UPDATE/DELETE solo por PK", etc.

**Archivos afectados (11):** README, ROADMAP, RUNBOOK, TROUBLESHOOTING, USER_MANUAL, API, ARCHITECTURE, ERROR_CODES, SQL_REFERENCE, TECHNICAL_SPECS, web/modeler/USER_MANUAL.

**Causa raíz:** Las docs principales (CHANGELOG, SQL_REFERENCE, MISSING_COMMANDS, ERROR_CODES) se actualizan en el mismo push del bloque, pero las **docs periféricas** (positioning, recruiter, competitive, etc.) tienen menciones tangenciales que se desactualizan sin que nadie note.

**Fix:** Sweep con agente Explore que mapea referencias obsoletas + edits en paralelo. Aplicado tras E1/E2/E3 (`23daa7e`) y tras F (`df6e40e`).

**Lección:** Cada bloque que cambia la **superficie SQL** debe disparar un sweep automático. Idealmente esto sería un grep en CI (`grep -rni 'solo soporta =' docs/ *.md` que falla).

---

### D-2 · Sweep post-F (commit `df6e40e`)

Mismo patrón después del bloque F. 9 archivos con referencias a "GROUP BY/agregados son futuro" cuando ya estaban implementados:

- README "Fase 3", `STATUS.md` con `GROUP BY: 🔴`, `POSITIONING.md` "no resuelve agregados", etc.

Misma estrategia (Explore + edits).

---

## 🔧 Acciones de prevención (post-mortem)

Lo que se podría agregar al repo para que estos patrones no vuelvan:

| # | Acción | Costo | Beneficio |
|---|---|---|---|
| 1 | **Pre-commit hook** que corre `cargo fmt --check + clippy + check`. | 10 min instalar | Captura CI-1 y CI-2 antes del push. |
| 2 | **Script `scripts/resolve-action-shas.sh`** que toma `.github/workflows/*.yml`, busca `uses: x/y@TAG`, resuelve SHA real y propone el reemplazo. | 30 min | Previene CI-3. |
| 3 | **Test en CI**: `git ls-files docs/ \| sort -f \| uniq -d -i` debe estar vacío. | 5 min | Previene P-1. |
| 4 | **Linter de docs**: grep weekly en CI buscando frases obsoletas (`"solo soporta ="`, `"no admite GROUP BY"`, etc.). | 1 h | Previene D-1, D-2. |
| 5 | **Doc convention**: cuando se cambia un código de error, sweep obligatorio de `grep -rn '\[GBY-NNNN\]' tests/`. | gratis (convención) | Previene M-3. |

Ninguno crítico — todos los incidentes se cerraron en sesión. Pero ítems 1 y 2 son de bajo costo y alto valor.

---

## 📎 Apéndice: hashes de commits

```
7245afb  fix(security): remediar 5 hallazgos del audit interno 2026-05-25
211ee6c  fix(security): pin actions/* a SHA real + badges README completos
f6312f7  fix(pages): baseurl /gabysql para que el CSS cargue
2a0774a  feat(pages): landing custom híbrida — hero moderno + cuerpo técnico denso
9ce203f  fix(pages): reemplazar SHAs invenntados por tags @v* en actions/* oficiales
0d93e31  fix(pages): rename docs/INDEX.md → docs/index.md (case-sensitive)
cb5ba04  docs(pages): setup preliminar GitHub Pages servido desde docs/
76f6dba  build(release): workflow cross-platform + install.ps1 one-liner para Windows
9e2b13e  docs: barrido profundo post-sesión (E1+E2+E3+F+T+J+J2)
2f42eb5  feat(sql): UPSERT + REPLACE INTO + RETURNING (bloque J2)
4141f1e  feat(sql): multi-row INSERT + INSERT...SELECT + TRUNCATE (bloque J)
d0f1d36  feat(sql): BEGIN / COMMIT / ROLLBACK explícitos (bloque T)
797e9b5  fix(sql): HAVING SUM(x) resuelve cuando el agregado tiene alias
df6e40e  docs: barrido post-F — reflejar cierre de agregaciones en toda la doc
f93b188  fix(sql): HAVING acepta columnas virtuales (alias + canonical agg)
cbc7586  feat(sql): agregaciones GROUP BY/HAVING/COUNT/SUM/AVG/MIN/MAX/DISTINCT (bloque F)
b6f8df2  style: fix clippy lints introduced en E1/E2 (manual_map, useless_format)
23daa7e  style+docs: cargo fmt + barrido completo de docs post E1/E2/E3
d439bb0  feat(sql): UPDATE / DELETE con WHERE completo (bloque E3)
bc07f1e  feat(sql): operadores <, >, <=, >=, <>/!=, LIKE, IS NULL, IN literal (bloque E2)
a67503f  feat(sql): WHERE con AND/OR/NOT + paréntesis y 3VL (bloque E1)
```

Última actualización: 2026-05-25 post-commit `7245afb`.
