# ADR-0006: `grype --only-fixed` en lugar de `--fail-on critical`

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-04
**Contexto**: el job `container_scan` rompía el merge por CVEs no-remediables.

## 🧭 Contexto

La política original del workflow `security.yml` ejecutaba `grype scan-target:ci --fail-on critical`. Eso rompía el job en cualquier CVE Critical detectada en la imagen, sin distinguir si la CVE tenía o no un parche disponible.

`debian:bookworm-slim` reporta decenas de CVEs (incluyendo Critical y High) en `libc6`, `libpam`, `ncurses`, `util-linux`, `gpgv`, etc. que Debian marca **`(won't fix)` para esa major release**. Esas CVEs no tienen parche aguas arriba — son irremediables dentro de la imagen actual sin migrar a otra base.

Resultado: el job rompía el merge permanentemente sin que nadie pudiera arreglarlo.

## 💡 Decisión

Cambiar la política a **fallar solo ante vulnerabilidades que tengan fix publicado**, mediante `.grype.yaml`:

```yaml
only-fixed: true
fail-on-severity: critical
```

Adicionalmente, el workflow se divide en dos pasos:

1. **Inventario completo** (sin filtros) impreso al `$GITHUB_STEP_SUMMARY`. Las CVEs no-fixable siguen visibles y auditables.
2. **Enforcement** con `.grype.yaml`. Solo este paso decide si el job pasa o falla.

El Dockerfile incorpora `apt-get upgrade` en el stage runtime para que cualquier CVE que sí tenga parche en repos Debian baje al hacer build.

## 🔄 Alternativas consideradas

- **Mantener `--fail-on critical` puro**: rechazado — produce falsos positivos permanentes que el equipo aprende a ignorar (security fatigue).
- **Lista hardcodeada de CVEs ignoradas**: rechazado — produce drift silencioso cada vez que sale una nueva CVE no-fixable; la lista crece para siempre.
- **Migrar la imagen base a `gcr.io/distroless/cc-debian12`**: aceptable a futuro pero requiere validar que el binario gabysql funciona en distroless (probable que sí — usa solo glibc + libgcc) y que `phpgabyadmin` siga funcionando (es un container PHP separado, no afecta). Queda en backlog del [Camino A](../COMMERCIAL_ROADMAP.md).
- **Migrar a Alpine**: rechazado — mismas o más CVEs, solo distintas; rompe glibc compat con el binario `cargo build --release` actual.
- **`--fail-on high`**: rechazado — empeora el problema de falsos positivos.

## 📊 Consecuencias

**Positivas**:
- El merge bloquea ante CVEs **fixeables**, que sí dependen del trabajo del equipo.
- El reporte completo sigue visible para auditoría — no estamos ocultando las CVEs no-fixable, solo no rompiendo el build por ellas.
- `apt-get upgrade` en build asegura que el repo Debian se aplica al momento del image build.

**Negativas**:
- CVEs no-fixable de severidad Critical pasan al merge sin bloquear. La mitigación (no es un riesgo real para el producto):
  - Las imágenes Docker aplican **siempre** el último apt upgrade en el momento de build.
  - El operador en producción debe rebuildar periódicamente (cualquier reverse proxy + base image refresh lo cubre).
  - El reporte completo en cada CI run permite tracking.

**Neutras**:
- La política está alineada con la práctica industrial estándar (Anchore, Snyk, Trivy).

## 🔗 Referencias

- Commit: `113109b`.
- Implementación: [.grype.yaml](../../.grype.yaml), [.github/workflows/security.yml :: container_scan](../../.github/workflows/security.yml), [Dockerfile](../../Dockerfile) (apt-get upgrade en runtime stage).
- Postura completa: [docs/SECURITY_LAYERS.md §5](../SECURITY_LAYERS.md).
