<#
.SYNOPSIS
    Instalador de gabysql para Windows.

.DESCRIPTION
    Descarga el último release de gabysql desde GitHub Releases,
    extrae los binarios a %LOCALAPPDATA%\Programs\gabysql\ y agrega
    ese directorio al PATH del usuario.

    No requiere permisos de administrador. No instala en Program Files.
    No registra entradas en "Programas y características" — la
    desinstalación es eliminar el directorio + sacar el PATH.

.PARAMETER Version
    Tag específico a instalar (ej: 'v0.2.0'). Por defecto: último.

.PARAMETER InstallDir
    Directorio destino. Por defecto: %LOCALAPPDATA%\Programs\gabysql.

.PARAMETER NoPath
    Si se pasa, NO modifica el PATH del usuario. Útil si querés
    invocar `gabysql.exe` con ruta absoluta solamente.

.EXAMPLE
    # Instalación típica desde PowerShell:
    iwr https://raw.githubusercontent.com/vladimiracunadev-create/gabysql/main/scripts/install.ps1 | iex

.EXAMPLE
    # Versión específica:
    & ([scriptblock]::Create((iwr https://raw.githubusercontent.com/vladimiracunadev-create/gabysql/main/scripts/install.ps1).Content)) -Version v0.2.0

.NOTES
    Si tu PowerShell rechaza scripts remotos, abrí PowerShell y corré
    una vez (solo para tu usuario):
        Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned
#>

[CmdletBinding()]
param(
    [string]$Version = '',
    [string]$InstallDir = '',
    [switch]$NoPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = 'vladimiracunadev-create/gabysql'
$AssetPattern = 'gabysql-{0}-windows-x86_64.zip'   # placeholder {0} = tag
$ProgressPreference = 'SilentlyContinue'           # acelera Invoke-WebRequest

function Write-Step([string]$msg) {
    Write-Host "==> $msg" -ForegroundColor Cyan
}

function Resolve-Tag {
    if ($Version) { return $Version }
    Write-Step "Consultando el último release de $Repo ..."
    $api = "https://api.github.com/repos/$Repo/releases/latest"
    try {
        $r = Invoke-RestMethod -Uri $api -Headers @{ 'User-Agent' = 'gabysql-install.ps1' }
        return $r.tag_name
    } catch {
        throw "No pude consultar GitHub API: $($_.Exception.Message)"
    }
}

function Resolve-InstallDir {
    if ($InstallDir) { return $InstallDir }
    return Join-Path $env:LOCALAPPDATA 'Programs\gabysql'
}

function Download-Release([string]$tag, [string]$destDir) {
    $assetName = ($AssetPattern -f $tag)
    $zipUrl    = "https://github.com/$Repo/releases/download/$tag/$assetName"
    $shaUrl    = "$zipUrl.sha256"
    $zipPath   = Join-Path $env:TEMP $assetName
    $shaPath   = "$zipPath.sha256"

    Write-Step "Descargando $assetName ..."
    try {
        Invoke-WebRequest -Uri $zipUrl -OutFile $zipPath -UseBasicParsing
    } catch {
        throw "No pude descargar $zipUrl . ¿Existe ese tag en Releases? Detalles: $($_.Exception.Message)"
    }

    # Sec4 (2026-05-25): verificación de SHA256 OBLIGATORIA.
    #
    # Pre-fix, el `catch [System.Net.WebException]` saltaba el chequeo si
    # la descarga del `.sha256` fallaba. Un MITM activo podía interceptar
    # AMBOS el zip y la solicitud del `.sha256` (devolver 404), forzando
    # al script a continuar sin verificación e instalando un binario
    # malicioso (CWE-347: Improper Verification of Cryptographic
    # Signature). Ahora cualquier error en la descarga o verificación es
    # fatal — la instalación se aborta limpia, sin escribir nada al
    # destino.
    Write-Step "Descargando hash de verificación SHA256 ..."
    try {
        Invoke-WebRequest -Uri $shaUrl -OutFile $shaPath -UseBasicParsing -ErrorAction Stop
    } catch {
        throw "No pude descargar el SHA256 desde $shaUrl. La instalación se aborta — sin verificación de integridad NO se ejecuta el binario. Detalles: $($_.Exception.Message)"
    }
    if (-not (Test-Path $shaPath) -or ((Get-Item $shaPath).Length -eq 0)) {
        throw "El archivo .sha256 descargado está vacío o no existe. Abortando."
    }
    $expected = ((Get-Content $shaPath -Raw) -split '\s+')[0].ToLower()
    if ($expected.Length -ne 64) {
        throw "El SHA256 publicado tiene formato inválido ('$expected'). Abortando."
    }
    $actual = (Get-FileHash $zipPath -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) {
        throw "SHA256 no coincide. Esperado $expected, obtuve $actual. Posible MITM o release corrupto. Abortando."
    }
    Write-Step "SHA256 verificado: $actual"

    Write-Step "Extrayendo a $destDir ..."
    if (Test-Path $destDir) {
        Remove-Item $destDir -Recurse -Force
    }
    New-Item -ItemType Directory -Path $destDir -Force | Out-Null

    # El zip trae los binarios dentro de un subdirectorio
    # `gabysql-${TAG}-windows-x86_64/`. Extraemos a un temp y movemos
    # el CONTENIDO de ese subdirectorio al destino final.
    $stagingDir = Join-Path $env:TEMP "gabysql-install-staging-$([guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $stagingDir -Force | Out-Null
    try {
        Expand-Archive -Path $zipPath -DestinationPath $stagingDir -Force
        $inner = Get-ChildItem -Path $stagingDir -Directory | Select-Object -First 1
        if (-not $inner) {
            throw "El zip no contiene un subdirectorio esperado."
        }
        Get-ChildItem -Path $inner.FullName | Move-Item -Destination $destDir
    } finally {
        Remove-Item $stagingDir -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item $zipPath -ErrorAction SilentlyContinue
        Remove-Item $shaPath -ErrorAction SilentlyContinue
    }
}

function Update-UserPath([string]$dirToAdd) {
    if ($NoPath) {
        Write-Warning "PATH no modificado (--NoPath). Usá la ruta absoluta: `"$dirToAdd\gabysql.exe`""
        return
    }
    # Leemos el PATH del usuario directamente del registro (no de
    # $env:PATH, que mezcla user+machine). Si ya está, no duplicamos.
    $current = [Environment]::GetEnvironmentVariable('PATH', 'User')
    if (-not $current) { $current = '' }
    $entries = $current.Split(';', [StringSplitOptions]::RemoveEmptyEntries)
    if ($entries -contains $dirToAdd) {
        Write-Step "PATH ya contiene $dirToAdd — no se duplica."
        return
    }
    $new = if ($current.Length -gt 0) { "$current;$dirToAdd" } else { $dirToAdd }
    [Environment]::SetEnvironmentVariable('PATH', $new, 'User')
    Write-Step "PATH del usuario actualizado: $dirToAdd agregado."
    Write-Warning "Tenés que abrir una NUEVA terminal para que el PATH se refresque (las terminales abiertas no lo ven)."
}

function Verify-Install([string]$binDir) {
    $exe = Join-Path $binDir 'gabysql.exe'
    if (-not (Test-Path $exe)) {
        throw "gabysql.exe no quedó en $binDir tras la extracción."
    }
    Write-Step "Verificando: $exe --version"
    try {
        & $exe --version
    } catch {
        Write-Warning "El binario está pero --version falló: $($_.Exception.Message)"
    }
}

# ───────── main ─────────
$tag       = Resolve-Tag
$installTo = Resolve-InstallDir

Write-Step "gabysql $tag → $installTo"

Download-Release -tag $tag -destDir $installTo
Update-UserPath -dirToAdd $installTo
Verify-Install -binDir $installTo

Write-Host ""
Write-Host "✅ gabysql instalado." -ForegroundColor Green
Write-Host "   Abrí una nueva terminal y probá:"
Write-Host "       gabysql --help"
Write-Host "       gabysql init mi-base.db"
Write-Host "       gabysql exec mi-base.db `"SELECT 1;`""
Write-Host ""
Write-Host "Para desinstalar:"
Write-Host "       Remove-Item -Recurse -Force `"$installTo`""
Write-Host "       (y editá manualmente el PATH del usuario si querés sacarlo)"
