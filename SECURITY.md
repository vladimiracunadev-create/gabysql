# 🔐 SECURITY

> **Postura de seguridad actual y hardening recomendado para `gabysql`.**
>
> 📑 Para el **mapa completo de capas de seguridad** (storage, acceso, SDLC, workflows, container, operación) con archivos exactos de cada control, ver [docs/SECURITY_LAYERS.md](docs/SECURITY_LAYERS.md). Este documento se enfoca en política de disclosure y postura general.

---

## 🚦 Estado actual

`gabysql` es hoy un motor embebido con server HTTP opcional. Su postura de seguridad es suficiente para laboratorio, desarrollo local y un producto base controlado, pero no debe presentarse todavía como plataforma multiusuario endurecida de nivel enterprise.

## 🧾 Versiones soportadas

| Línea | Estado | Formato en disco |
|---|---|---|
| `0.1.x` (último binario en `main`) | soportada | `VERSION = 8` |
| `0.1.x` previos | sin soporte de seguridad | `VERSION = 1` a `7` (cada bump rechaza explícitamente las DBs anteriores; ver [COMPATIBILITY.md §5](COMPATIBILITY.md#5--formato-en-disco)) |
| implementación previa al rewrite en Rust | no soportada | n/a |

> Cualquier reporte de vulnerabilidad debe estar reproducido contra el `HEAD` de `main` o el último release publicado. No se publicarán parches retroactivos para versiones anteriores del formato en disco.

---

## 🛡️ Protecciones que existen hoy

- `phpgabyadmin` usa cookie firmada cuando `GABYADMIN_TOKEN` está definido
- `phpgabyadmin` bloquea hosts remotos salvo `GABYADMIN_ALLOW_REMOTE=1`
- `gabysql-server` puede exigir token HTTP con `-token`
- `gabysql-server` cap de conexiones simultáneas (default `64`, configurable con `-max-connections`); las extra reciben `503` y se cierran sin spawning de threads (mitigación básica de exhausting de recursos)
- el nombre de DB en modo `-dir` se normaliza y bloquea rutas arbitrarias
- el motor hace rollback ante errores de ejecución SQL dentro de la transacción activa
- `Pager::create` rehúsa sobrescribir un archivo existente (mitiga pérdida de datos por uso accidental de `init`)
- `Pager::create/open` adquiere un **lock exclusivo cross-process** sobre el `.db` (advisory en Linux/macOS, mandatory en Windows): impide que dos procesos abran y escriban concurrentemente el mismo archivo. Ver [ADR-0013](docs/adr/0013-process-level-file-lock.md).
- cada página persistida valida CRC32-IEEE al leerse y al replay del WAL (mitiga corrupción silenciosa, no es protección contra adversarios con acceso al disco)
- el CLI ofrece `gabysql backup/restore/verify` que validan CRC página por página y re-abren el destino post-escritura — operación canónica para snapshots, en vez de `cp` (ver [ADR-0015](docs/adr/0015-verified-backup-restore.md))
- el modelador web `gabymodeler` es **zero-coupling** (no llama a la API ni lee tokens; el usuario copia el SQL al portapapeles y lo pega en `phpgabyadmin`), evitando exposición de credenciales en el front

---

## ⚠️ Qué no existe todavía

- no hay TLS nativo en `gabysql-server`
- no hay cifrado en reposo del `.db`
- no hay control fino de usuarios/roles
- no hay aislamiento multi-tenant
- no hay auditoría avanzada ni security logs estructurados

---

## 🧱 Recomendaciones de hardening

### Para server HTTP
- publica `gabysql-server` detrás de un reverse proxy con TLS
- usa `-token` siempre que no sea un laboratorio efímero
- expón el puerto solo en red de confianza o localhost

### Para `phpgabyadmin`
- usa `GABYADMIN_TOKEN`
- no habilites `GABYADMIN_ALLOW_REMOTE=1` salvo necesidad real
- si lo expones por red, hazlo detrás de un proxy con autenticación adicional

### Para almacenamiento
- restringe permisos del archivo `.db`
- realiza backups offline
- evita compartir directorios de datos sin controles del sistema operativo

---

## 📣 Divulgación responsable

Si encuentras una vulnerabilidad:
1. **No abrir un Issue público**. Usa "Report a vulnerability" en la pestaña *Security* del repositorio, o contacta al mantenedor por DM.
2. No publiques secretos, tokens ni pasos de explotación destructivos en canales abiertos hasta que el reporte esté triado.
3. Incluye:
   - commit SHA exacto contra el que reproduces (idealmente `HEAD` de `main`).
   - sistema operativo, toolchain de Rust, modo (CLI / server / Docker).
   - pasos mínimos de reproducción, sin payloads destructivos contra terceros.
   - impacto estimado (lectura de datos ajenos, escritura, DoS, ejecución, etc).

### Compromiso de respuesta
- Acuse de recibo: **3 días hábiles**.
- Triage inicial: **7 días hábiles**.
- Fix público + advisory: depende de severidad. Crítico → rama `main` + nota en [CHANGELOG.md](CHANGELOG.md) tan pronto como exista parche verificable.

### Lo que cubre
- Bypass de los CRC32 que valida el WAL/Pager.
- Lectura/escritura cruzada entre DBs en modo `-dir` (path traversal).
- Bypass del filtro `WHERE pk = N` para mutar filas no autorizadas.
- Saltarse `-token` o el cap de conexiones.
- Inyección SQL más allá del subconjunto soportado.
- Ejecución arbitraria a través del parser, codec o `phpgabyadmin`.

### Lo que NO se considera vulnerabilidad
- Falta de TLS nativo en `gabysql-server` (documentado, usar reverse proxy).
- Que un atacante con acceso de escritura al disco pueda recomputar CRCs (los CRCs son anti-corrupción, no anti-tampering).
- Exposición pública de `phpgabyadmin` sin token (la doc lo desaconseja explícitamente).

---

## 🧠 Riesgos conocidos

- el modelo de concurrencia sigue siendo básico (mutex de proceso para escrituras)
- el API no implementa rate limiting por cliente; el techo de `-max-connections` es una salvaguarda mínima, no un sustituto de rate limiting o WAF
- el admin web depende de la exposición segura del entorno donde se publica
- el formato en disco aún no tiene sistema formal de migraciones entre versiones mayores; un bump de VERSION bloquea la apertura con error explícito (no migra automáticamente)
- los CRC32 detectan corrupción accidental, no manipulación adversarial intencional (un atacante con acceso de escritura al `.db` puede recomputar el CRC)
