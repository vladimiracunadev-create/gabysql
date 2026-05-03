# 🔐 SECURITY

> **Postura de seguridad actual y hardening recomendado para `gabysql`.**

---

## 🚦 Estado actual

`gabysql` es hoy un motor embebido con server HTTP opcional. Su postura de seguridad es suficiente para laboratorio, desarrollo local y un producto base controlado, pero no debe presentarse todavía como plataforma multiusuario endurecida de nivel enterprise.

## 🧾 Versiones soportadas

| Línea | Estado |
|---|---|
| `0.1.x` | soportada |
| implementación previa al rewrite en Rust | no soportada |

---

## 🛡️ Protecciones que existen hoy

- `phpgabyadmin` usa cookie firmada cuando `GABYADMIN_TOKEN` está definido
- `phpgabyadmin` bloquea hosts remotos salvo `GABYADMIN_ALLOW_REMOTE=1`
- `gabysql-server` puede exigir token HTTP con `-token`
- `gabysql-server` cap de conexiones simultáneas (default `64`, configurable con `-max-connections`); las extra reciben `503` y se cierran sin spawning de threads (mitigación básica de exhausting de recursos)
- el nombre de DB en modo `-dir` se normaliza y bloquea rutas arbitrarias
- el motor hace rollback ante errores de ejecución SQL dentro de la transacción activa
- `Pager::create` rehúsa sobrescribir un archivo existente (mitiga pérdida de datos por uso accidental de `init`)
- cada página persistida valida CRC32-IEEE al leerse y al replay del WAL (mitiga corrupción silenciosa, no es protección contra adversarios con acceso al disco)

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
1. no publiques secretos, tokens ni pasos de explotación destructivos
2. repórtala de forma privada al mantenedor por GitHub o canal directo antes de divulgación pública
3. incluye versión, entorno y pasos de reproducción mínimos

---

## 🧠 Riesgos conocidos

- el modelo de concurrencia sigue siendo básico (mutex de proceso para escrituras)
- el API no implementa rate limiting por cliente; el techo de `-max-connections` es una salvaguarda mínima, no un sustituto de rate limiting o WAF
- el admin web depende de la exposición segura del entorno donde se publica
- el formato en disco aún no tiene sistema formal de migraciones entre versiones mayores; un bump de VERSION bloquea la apertura con error explícito (no migra automáticamente)
- los CRC32 detectan corrupción accidental, no manipulación adversarial intencional (un atacante con acceso de escritura al `.db` puede recomputar el CRC)
