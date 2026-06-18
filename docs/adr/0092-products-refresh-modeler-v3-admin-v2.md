# ADR-0092: Refresh integral de productos web — gabymodeler v3 + phpgabyadmin v2

**Fecha:** 2026-06-17 → 2026-06-18
**Estado:** Aceptado
**Bloque:** Productos de gestión (Pushes 1-6, 8, 9, 11-13, 16, 17, 21 — 14 commits a `main`).
**Refina:** [ADR-0091](0091-catalog-listing-endpoints.md) (consume los endpoints), todos los ADRs de motor del bloque Y/Z/X1/X3 (modela las features).

## Contexto

Antes de esta sesión los dos productos web tenían cobertura muy
limitada vs el motor:

| Producto | Pre-sesión | Lo que el motor ya soportaba |
|---|---|---|
| `phpgabyadmin` | Browse / Structure (con índices CRUD) / SQL editor con snippets | + Sessions M13 / EXPLAIN ANALYZE bias M6 / Stats / Policies / Routines (trg+prc+fn) / Users+Roles+Grants |
| `gabymodeler` | ER con 14 reglas Check Model, INT/TEXT/BOOL/FLOAT/DATE/DATETIME/JSON, FKs single-col, PK forzada INT | + tipos Y1-Y9 (DECIMAL exacto, BLOB, UUID, TIME, VARCHAR enforcement, INT widths, UNSIGNED) / CHECK / composite PK/UNIQUE/INDEX / Views / RLS Policies / Triggers / Procedures / Functions / Users / Roles / Grants |

La distancia entre **lo que el motor sabe hacer** y **lo que el
usuario puede operar desde la web** se había ampliado mucho durante
la sesión maratón del 2026-06-15 (ADRs 0078-0090) y los bloques
Y/Z/X1/X3 anteriores. Ningún cliente externo usaría la mitad de las
features sin escribir SQL a mano.

Adicionalmente, ambos productos tenían identidad visual desalineada
con la landing page (`/`), lo que rompía la sensación de "un solo
producto".

## Decisión

Refresh integral en **dos ejes simultáneos**:

### Eje A — visual unificado

Palette + tipografía + componentes alineados con la landing y entre
los dos productos:

| Token | Valor | Uso |
|---|---|---|
| `--bg-0` ... `--bg-4` | `#0a0e14` → `#1c2128` | Backgrounds en escala (deepest a hover) |
| `--accent` | `#58a6ff` | GitHub blue — primary action |
| `--success` | `#7ee787` | PK badge, OK toast |
| `--warning` | `#f0883e` | FK badge, MILD bias |
| `--danger` | `#ff7b72` | Destructive, HIGH bias |
| Font | **Inter** (UI) + **JetBrains Mono** (code/data) | Google Fonts CDN |

Componentes compartidos por convención (no por código — son archivos
independientes, no React/Vue):

- Brand mark con gradient (▦ azul) en topbar.
- Pills (status, version, session) con dot indicador.
- Cards con header (title + meta) y body.
- Toast notifications auto-dismiss 5s.
- Modales con backdrop blur + shadow-lg.
- Tags semánticos por constraint (PK verde, FK naranja, UNIQUE azul, CHK púrpura).

### Eje B — cobertura funcional 1:1 con el motor

#### `gabymodeler` v3 → estado del schema

```ts
{
  dbName, ifNotExists,
  entities: [{
    id, name, x, y,
    columns: [{ id, name, type, pk, notNull, unique,
                hasDefault, defaultValue,
                hasCheck, checkExpr,
                fk: null | { table, column, onDelete } }],
    constraints: [{ id, kind:'pk'|'unique', name?, columns:[colName] }],
    indexes:     [{ id, name?, unique, columns:[colName] }]
  }],
  views:      [{ id, name, body }],
  policies:   [{ id, name, table, action, role, using, check }],
  triggers:   [{ id, name, table, timing, event, body }],
  procedures: [{ id, name, params:[{name,type}], body }],
  functions:  [{ id, name, params:[{name,type}], returnType, body }],
  users:      [{ id, name, password }],
  roles:      [{ id, name }],
  grants:     [{ id, grantee, object, privs:[...] }]
}
```

Backward-compat con localStorage antiguo: cada `loadState()` hace
init de las propiedades faltantes a `[]`, así que un usuario que tenía
un modelo v2 abre v3 sin perder nada.

Status bar refleja 13 contadores en vivo: `N tablas · cols · idx · FKs ·
composite · views · policies · trg · prc · fn · usr · rol · grants`.

#### `phpgabyadmin` v2 → 9 tabs

| Tab | Trigger | Bloque motor |
|---|---|---|
| Browse | `?table=X&tab=browse` | core SELECT + paginado |
| Structure | `?tab=structure` | catalog.get_table |
| SQL editor | `?tab=sql` | exec con CodeMirror |
| Sessions | `?tab=sessions` | M13 cross-request tx |
| Explain | `?tab=explain` | M6 EXPLAIN ANALYZE bias |
| Stats | `?tab=stats` | derivado de /tables + /metrics |
| Policies | `?tab=policies` | Z3 RLS |
| Routines | `?tab=routines` | X1 + X3 |
| Security | `?tab=security` | Z1 + Z2 |

#### CodeMirror SQL editor

`textarea[name="sql"]` y los textareas de policies (`p_using`, `p_check`)
montan **CodeMirror 5.65.16** desde cdnjs con:

- Mode `text/x-sql`, lineNumbers, autoCloseBrackets, matchBrackets.
- Atajos: `Ctrl/Cmd+Enter` ejecuta el form; `Ctrl/Cmd+/` toggleComment.
- Tokens colorizados estilo GitHub (keywords rojo, strings azul claro,
  numbers azul, vars blanco, atom púrpura, comment gris itálico).
- Theme override CSS vía las mismas `--bg-*` / `--text` para que
  CodeMirror no rompa la identidad.

#### Canvas del modeler — zoom + pan + minimap

`#canvas-wrap` pasa a `overflow:hidden`. Interior `#canvas-content`
(5000×3000 lógicos) recibe `transform: translate() scale()`. Las
entities se appendean a `#canvas-content`, no al wrap.

- **Wheel + Ctrl/Cmd**: zoom-to-cursor, factor exponencial (0.0015 *
  -deltaY). Rango 25%-250%.
- **Middle-click drag** / **Alt+Left**: pan.
- **Wheel sin Ctrl**: pan vertical; **Shift+Wheel**: pan horizontal.
- **Toolbar** bottom-right: +/−/⌂ reset/⛶ fit-all + label "100%".
- **Atajos teclado**: `+` `-` `0` `F` (ignorados si foco en input).
- **Drag de entidades** compensa por zoom dividiendo el delta de
  clientX/Y por `canvasZoom`, así el cursor queda pegado a la entidad
  bajo cualquier nivel de zoom.
- **Minimap** 220×150 px bottom-left con click-drag navegación y
  viewport indicator verde. `MutationObserver` sobre `#canvas-content`
  re-renderiza el minimap cuando se agregan/eliminan entities.

## Reverse engineering completo (Push 21)

`doImport()` antes leía sólo `/tables`. Ahora hace 9 GETs en paralelo
vía `Promise.all`, con un helper `fetchList()` que tolera 404/error
(degrada silenciosamente para apuntar a servers viejos).

Mapeo limitaciones:
- **Policy multi-rol**: el modelo del modeler tiene un solo `role`, así
  que se toma `roles[0]` del array del server. Round-trip pierde roles
  adicionales — known issue documentado.
- **Password round-trip**: el server NUNCA expone el hash (correcto por
  ADR-0091). Import deja `users[].password = ""`. Si el usuario quiere
  regenerar el .sql con password, debe re-introducirlo.

## Alternativas descartadas

- **React/Vue/Svelte para el modeler**: rechazado — ADR-0001 vanilla.
  Single-file HTML+JS+CSS es **deploy-zero**: `python -m http.server`
  o `php -S` y listo.
- **CodeMirror 6**: requiere bundle webpack. Rechazado por la misma
  razón. CM5 vía CDN funciona standalone.
- **Refactorizar admin a SPA**: scope creep. El query-string + PHP
  if/elseif es feo pero funciona, no hay sesión state intercambiada
  entre tabs salvo `$_SESSION['gby_session_id']` para M13.

## Métricas

- 14 commits a `main` sin breaking changes para esos 2 productos.
- `web/modeler/index.html`: 1036 → ~2400 LOC.
- `web/phpgabyadmin/index.php`: 1110 → ~1900 LOC.
- 13 contadores en status bar del modeler; 9 tabs en el admin.

## Consecuencias

### Positivas
- Cualquier feature del motor ahora se opera desde la web sin SQL a
  mano (excepto `REVOKE` granular y `DROP TRIGGER/PROCEDURE/FUNCTION`,
  que requieren tab SQL).
- Identidad visual coherente entre landing, admin y modeler.
- Reverse engineering completo: importar un DB en producción trae
  TODO (excepto secretos).

### Negativas / tradeoffs
- Tamaño de cada archivo crece — el modeler superó las 2k LOC en un
  solo HTML. Aceptable porque no hay build system y el navegador lo
  parsea cada vez, no es runtime hot.
- CodeMirror se carga desde CDN: si la red del usuario lo bloquea,
  los textareas siguen funcionando como `<textarea>` plano (graceful
  degradation por construcción).
- Tab Routines en admin es read-only — para crear/editar el usuario
  va al SQL editor. Decisión consciente: redundancia con un wizard
  inline no agrega valor cuando el editor SQL ya tiene CodeMirror.

## Referencias

- Pushes 1-6 (visual + Sessions + Explain + Stats + Policies + CodeMirror): commits 5c42a2e → b7be3c9.
- Pushes 8, 11, 17 (admin listado real + Routines + Security): 70ba720, 7424336, 69d57af.
- Pushes 2-4, 9, 12, 13, 16 (modeler v3 + composite + canvas + routines + security): a5d9124, 198c232, f412f55, ff5ea63, 83e33b1, 29a05ac, c8a2dc1.
- Push 21 (reverse engineering): 75e9604.
- Endpoints HTTP que habilitan todo: [ADR-0091](0091-catalog-listing-endpoints.md).
- Bloques motor referenciados: ADR-0019 (composite PK/index), ADR-0021 (CHECK), ADR-0025 (views), ADR-0050 (Z1 users/roles), ADR-0051 (Z2 grants), ADR-0052 (Z3 RLS), ADR-0078-0090 (sesión maratón previa).
- Implementación: [web/modeler/index.html](../../web/modeler/index.html), [web/phpgabyadmin/index.php](../../web/phpgabyadmin/index.php).
