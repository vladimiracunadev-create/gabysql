<?php
session_start();

// ---------------------------------------------------------------------------
// CSRF protection (Sec5, 2026-05-25).
// ---------------------------------------------------------------------------
if (!isset($_SESSION['csrf_token']) || !is_string($_SESSION['csrf_token']) || strlen($_SESSION['csrf_token']) !== 64) {
  $_SESSION['csrf_token'] = bin2hex(random_bytes(32));
}

function csrf_token(): string {
  return $_SESSION['csrf_token'];
}

function csrf_field(): string {
  return '<input type="hidden" name="csrf_token" value="' . htmlspecialchars(csrf_token(), ENT_QUOTES, 'UTF-8') . '">';
}

function require_csrf_token(): void {
  if ($_SERVER['REQUEST_METHOD'] !== 'POST') {
    return;
  }
  $submitted = isset($_POST['csrf_token']) && is_string($_POST['csrf_token']) ? $_POST['csrf_token'] : '';
  $expected  = csrf_token();
  if (!hash_equals($expected, $submitted)) {
    http_response_code(403);
    echo '<!doctype html><meta charset="utf-8"><title>403 CSRF</title>';
    echo '<h1>403 — CSRF token inválido o ausente</h1>';
    echo '<p>El POST recibido no incluye un token CSRF válido. Si llegaste acá desde un link externo, ese link es malicioso. Volvé al admin desde la URL original.</p>';
    exit;
  }
}

function cookie_options(int $expires): array {
  $secure = !empty($_SERVER['HTTPS']) && $_SERVER['HTTPS'] !== 'off';
  return [
    'expires' => $expires,
    'path' => '/',
    'secure' => $secure,
    'httponly' => true,
    'samesite' => 'Strict',
  ];
}

function auth_cookie_value(string $token): string {
  return hash_hmac('sha256', 'phpgabyadmin-auth', $token);
}

// ---------------------------------------------------------------------------
// Server URL normalization + remote-allow check.
// ---------------------------------------------------------------------------
function normalize_server(string $value): array {
  $value = trim($value);
  if ($value === '') {
    $value = trim((string)getenv('GABYADMIN_SERVER'));
  }
  if ($value === '') {
    $value = 'http://localhost:8080';
  }
  if (!preg_match('#^https?://#i', $value)) {
    return ['http://localhost:8080', 'Servidor inválido: usa http://host:puerto'];
  }
  $parts = parse_url($value);
  if (!$parts || empty($parts['scheme']) || empty($parts['host'])) {
    return ['http://localhost:8080', 'Servidor inválido'];
  }
  $host = strtolower($parts['host']);
  $allowRemote = getenv('GABYADMIN_ALLOW_REMOTE') === '1';
  if (!$allowRemote && !in_array($host, ['localhost', '127.0.0.1', '::1'], true)) {
    return ['http://localhost:8080', 'Servidor remoto bloqueado. Define GABYADMIN_ALLOW_REMOTE=1 si realmente quieres usar uno externo.'];
  }
  $scheme = strtolower($parts['scheme']);
  $port = isset($parts['port']) ? ':' . $parts['port'] : '';
  return [$scheme . '://' . $parts['host'] . $port, null];
}

// ---------------------------------------------------------------------------
// HTTP helpers (GET / POST / POST con session header — M13).
// ---------------------------------------------------------------------------
function http_get_json(string $url, string $apiToken, string $sessionId = ''): array {
  $headers = "Accept: application/json\r\n";
  if ($apiToken !== '') {
    $headers .= "X-Gabysql-Token: " . $apiToken . "\r\n";
  }
  if ($sessionId !== '') {
    $headers .= "X-Gabysql-Session: " . $sessionId . "\r\n";
  }
  $ctx = stream_context_create([
    'http' => [
      'method' => 'GET',
      'timeout' => 10,
      'header' => $headers,
      'ignore_errors' => true,
    ]
  ]);
  $raw = @file_get_contents($url, false, $ctx);
  if ($raw === false) {
    return [null, "No se pudo conectar a $url"];
  }
  $json = json_decode($raw, true);
  if (!is_array($json)) {
    return [null, "Respuesta inválida (no JSON) desde $url"];
  }
  return [$json, null];
}

function http_post_json(string $url, array $payload, string $apiToken, string $sessionId = ''): array {
  $headers = "Content-Type: application/json\r\nAccept: application/json\r\n";
  if ($apiToken !== '') {
    $headers .= "X-Gabysql-Token: " . $apiToken . "\r\n";
  }
  if ($sessionId !== '') {
    $headers .= "X-Gabysql-Session: " . $sessionId . "\r\n";
  }
  $ctx = stream_context_create([
    'http' => [
      'method' => 'POST',
      'timeout' => 30,
      'header' => $headers,
      'content' => json_encode($payload, JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES),
      'ignore_errors' => true,
    ]
  ]);
  $raw = @file_get_contents($url, false, $ctx);
  if ($raw === false) {
    return [null, "No se pudo conectar a $url"];
  }
  $json = json_decode($raw, true);
  if (!is_array($json)) {
    return [null, "Respuesta inválida (no JSON) desde $url"];
  }
  return [$json, null];
}

// ---------------------------------------------------------------------------
// Auth gate (GABYADMIN_TOKEN).
// ---------------------------------------------------------------------------
$uiToken = getenv('GABYADMIN_TOKEN');
$cookieName = 'gabyadmin_auth';
if ($uiToken) {
  if (isset($_POST['logout'])) {
    require_csrf_token();
    setcookie($cookieName, '', cookie_options(time() - 3600));
    header('Location: ' . $_SERVER['PHP_SELF']);
    exit;
  }
  $expectedCookie = auth_cookie_value($uiToken);
  $ok = isset($_COOKIE[$cookieName]) && hash_equals($expectedCookie, (string)$_COOKIE[$cookieName]);
  if (!$ok) {
    $err = null;
    if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['ui_token'])) {
      if (hash_equals($uiToken, trim((string)$_POST['ui_token']))) {
        setcookie($cookieName, $expectedCookie, cookie_options(time() + 3600 * 6));
        header('Location: ' . $_SERVER['PHP_SELF']);
        exit;
      }
      $err = 'Token inválido';
    }
    ?><!doctype html>
    <html lang="es">
    <head>
      <meta charset="utf-8"/>
      <meta name="viewport" content="width=device-width, initial-scale=1"/>
      <title>phpgabyadmin · login</title>
      <link rel="preconnect" href="https://fonts.googleapis.com">
      <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
      <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap">
      <style>
        :root{
          --bg:#0a0e14; --surface:#11161d; --surface-2:#161c25;
          --border:#21262d; --border-strong:#30363d;
          --text:#e6edf3; --text-muted:#7d8590; --text-soft:#9ca3af;
          --accent:#58a6ff; --accent-hover:#79b8ff;
          --success:#7ee787; --warning:#f0883e; --danger:#ff7b72;
          --shadow:0 24px 64px rgba(0,0,0,.45);
        }
        *{box-sizing:border-box}
        body{margin:0;font-family:'Inter',-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif;
             background:radial-gradient(circle at 50% -10%,#1b2330 0%,#0a0e14 50%);color:var(--text);
             min-height:100vh;display:flex;align-items:center;justify-content:center;padding:20px}
        .login-card{background:var(--surface);border:1px solid var(--border);border-radius:16px;
                    padding:32px;box-shadow:var(--shadow);max-width:420px;width:100%}
        .brand{display:flex;align-items:center;gap:10px;margin-bottom:6px}
        .brand-mark{width:36px;height:36px;background:linear-gradient(135deg,var(--accent) 0%,#3d7fc6 100%);
                    border-radius:8px;display:flex;align-items:center;justify-content:center;
                    font-weight:700;font-size:18px;color:#0a0e14}
        .brand-name{font-size:20px;font-weight:600;letter-spacing:-.01em}
        .login-card h1{margin:0 0 4px;font-size:24px;font-weight:600}
        .login-card .subtitle{color:var(--text-muted);font-size:14px;margin-bottom:20px}
        .login-card label{display:block;font-size:13px;font-weight:500;color:var(--text-soft);margin-bottom:6px}
        .input{width:100%;padding:11px 14px;border-radius:8px;border:1px solid var(--border-strong);
               background:var(--bg);color:var(--text);font-size:14px;font-family:inherit;
               transition:border-color .15s ease}
        .input:focus{outline:none;border-color:var(--accent);box-shadow:0 0 0 3px rgba(88,166,255,.15)}
        .btn{display:inline-flex;align-items:center;justify-content:center;gap:6px;
             padding:11px 20px;border-radius:8px;border:1px solid transparent;cursor:pointer;
             font-family:inherit;font-size:14px;font-weight:600;transition:all .15s ease;text-decoration:none}
        .btn-primary{background:var(--accent);color:#0a0e14;width:100%;margin-top:16px}
        .btn-primary:hover{background:var(--accent-hover)}
        .error{margin-top:14px;background:rgba(255,123,114,.08);border:1px solid rgba(255,123,114,.3);
               color:var(--danger);padding:10px 12px;border-radius:8px;font-size:13px}
        .muted{color:var(--text-muted);font-size:13px}
        code{font-family:'JetBrains Mono',ui-monospace,SFMono-Regular,Consolas,monospace;
             background:var(--bg);padding:2px 6px;border-radius:4px;font-size:12px;color:var(--accent)}
      </style>
    </head>
    <body>
      <div class="login-card">
        <div class="brand">
          <div class="brand-mark">▣</div>
          <div class="brand-name">phpgabyadmin</div>
        </div>
        <h1>Entrar</h1>
        <p class="subtitle">Protegido por <code>GABYADMIN_TOKEN</code></p>
        <form method="post">
          <label for="ui_token">Token</label>
          <input id="ui_token" class="input" name="ui_token" type="password" autocomplete="current-password" autofocus/>
          <button class="btn btn-primary" type="submit">Continuar →</button>
        </form>
        <?php if ($err): ?><div class="error"><?= htmlspecialchars($err) ?></div><?php endif; ?>
      </div>
    </body>
    </html><?php
    exit;
  }
}

// ---------------------------------------------------------------------------
// Estado global derivado de URL/POST.
// ---------------------------------------------------------------------------
[$server, $serverErr] = normalize_server((string)($_GET['server'] ?? ''));
$apiToken = trim((string)($_GET['token'] ?? ''));
$db = trim((string)($_GET['db'] ?? ''));
$table = trim((string)($_GET['table'] ?? ''));
$tab = $_GET['tab'] ?? 'browse';
$activeSession = isset($_SESSION['active_session']) && is_string($_SESSION['active_session']) ? $_SESSION['active_session'] : '';
$activeSessionDb = isset($_SESSION['active_session_db']) ? (string)$_SESSION['active_session_db'] : '';
$sql = $_POST['sql'] ?? "-- Tip: SAVEPOINT name / ROLLBACK TO SAVEPOINT name / RELEASE SAVEPOINT name (M12)\n--      EXPLAIN ANALYZE SELECT ... (P2 + M6 bias en queries scan-only)\n--      ANALYZE TABLE t (P3 + P4 column stats)\n\nSELECT * FROM users LIMIT 10;";

// Toast notifications (in-page, cookieless).
$toasts = [];
function toast(string $kind, string $msg): void {
  global $toasts;
  $toasts[] = ['kind' => $kind, 'msg' => $msg];
}

// ---------------------------------------------------------------------------
// Actions: server discovery (/dbs, /tables, /health).
// ---------------------------------------------------------------------------
[$dbsResp, $dbsErr] = http_get_json($server . '/dbs', $apiToken);
$dbs = [];
$serverMode = '';
if (!$dbsErr && ($dbsResp['ok'] ?? false)) {
  $dbs = $dbsResp['dbs'] ?? [];
  $serverMode = $dbsResp['mode'] ?? '';
  if ($db === '' && count($dbs) > 0) {
    $db = (string)$dbs[0];
  }
}

[$healthResp, $healthErr] = http_get_json($server . '/health', $apiToken);
$serverHealthy = !$healthErr && ($healthResp['ok'] ?? false);

$tables = [];
$tablesErr = null;
if ($db !== '') {
  [$tablesResp, $tablesErr] = http_get_json($server . '/tables?db=' . urlencode($db), $apiToken);
  if (!$tablesErr && ($tablesResp['ok'] ?? false)) {
    $tables = $tablesResp['tables'] ?? [];
  }
}

// ---------------------------------------------------------------------------
// Actions: create DB, import CSV, export CSV.
// ---------------------------------------------------------------------------
if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['new_db'])) {
  require_csrf_token();
  [$createResp, $createErr] = http_post_json($server . '/dbs', ['db' => trim((string)$_POST['new_db'])], $apiToken);
  if ($createErr) {
    toast('error', $createErr);
  } elseif (!($createResp['ok'] ?? false)) {
    toast('error', $createResp['error'] ?? 'Error al crear DB');
  } else {
    toast('success', 'DB creada: ' . ($createResp['db'] ?? ''));
  }
}

$execJson = null;
$execErr = null;
if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['import_csv']) && $db !== '' && $table !== '' && isset($_FILES['csv'])) {
  require_csrf_token();
  $tmp = $_FILES['csv']['tmp_name'] ?? '';
  if (!$tmp || !is_uploaded_file($tmp)) {
    toast('error', 'No se recibió archivo');
  } else {
    $rows = array_map('str_getcsv', file($tmp));
    if (count($rows) < 2) {
      toast('error', 'CSV debe tener header + filas');
    } else {
      $header = $rows[0];
      $stmts = [];
      for ($i = 1; $i < count($rows); $i++) {
        $record = $rows[$i];
        $vals = [];
        for ($c = 0; $c < count($header); $c++) {
          $raw = trim((string)($record[$c] ?? ''));
          if ($raw === '' || strtoupper($raw) === 'NULL') $vals[] = 'NULL';
          elseif (strtoupper($raw) === 'TRUE' || strtoupper($raw) === 'FALSE') $vals[] = strtoupper($raw);
          elseif (is_numeric($raw)) $vals[] = $raw;
          else $vals[] = "'" . str_replace("'", "''", $raw) . "'";
        }
        $stmts[] = 'INSERT INTO ' . $table . ' (' . implode(',', $header) . ') VALUES (' . implode(',', $vals) . ')';
      }
      [$execJson, $execErr] = http_post_json($server . '/exec', ['db' => $db, 'sql' => implode(";\n", $stmts) . ';'], $apiToken);
      if ($execErr) toast('error', $execErr);
      elseif (!($execJson['ok'] ?? false)) toast('error', $execJson['error'] ?? 'Error');
      else toast('success', 'Import OK: ' . count($stmts) . ' filas');
    }
  }
}

if (isset($_GET['export']) && $db !== '' && $table !== '') {
  [$rowsResp, $rowsErr] = http_get_json($server . '/rows?db=' . urlencode($db) . '&table=' . urlencode($table) . '&limit=10000&offset=0', $apiToken);
  if ($rowsErr || !($rowsResp['ok'] ?? false)) {
    header('Content-Type: text/plain; charset=utf-8');
    echo 'Error export';
    exit;
  }
  $cols = $rowsResp['columns'] ?? [];
  $rows = $rowsResp['rows'] ?? [];
  header('Content-Type: text/csv; charset=utf-8');
  header('Content-Disposition: attachment; filename="' . basename($table) . '.csv"');
  $out = fopen('php://output', 'w');
  fputcsv($out, $cols);
  foreach ($rows as $row) fputcsv($out, $row);
  fclose($out);
  exit;
}

// ---------------------------------------------------------------------------
// Actions: CREATE / DROP INDEX.
// ---------------------------------------------------------------------------
if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['create_index']) && $db !== '') {
  require_csrf_token();
  $idxTable = trim((string)($_POST['create_index_table'] ?? ''));
  $idxName  = trim((string)($_POST['create_index_name']  ?? ''));
  $idxCol   = trim((string)($_POST['create_index_column']?? ''));
  $ident = '/^[A-Za-z_][A-Za-z0-9_]*$/';
  if (!preg_match($ident, $idxTable) || !preg_match($ident, $idxName) || !preg_match($ident, $idxCol)) {
    toast('error', 'Nombres inválidos: solo se admiten identificadores [A-Za-z_][A-Za-z0-9_]*');
  } else {
    $createSql = "CREATE INDEX {$idxName} ON {$idxTable} ({$idxCol});";
    [$execJson, $execErr] = http_post_json($server . '/exec', ['db' => $db, 'sql' => $createSql], $apiToken);
    if ($execErr) toast('error', $execErr);
    elseif (!($execJson['ok'] ?? false)) toast('error', $execJson['error'] ?? 'Error');
    else toast('success', 'Índice creado: ' . $idxName);
    $sql = $createSql;
  }
}

// ---------------------------------------------------------------------------
// Actions: SAVEPOINT helpers + SESSION lifecycle (M13).
// ---------------------------------------------------------------------------
if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['tx_begin'])) {
  require_csrf_token();
  [$txResp, $txErr] = http_post_json($server . '/tx/begin', $db !== '' ? ['db' => $db] : [], $apiToken);
  if ($txErr) toast('error', $txErr);
  elseif (!($txResp['ok'] ?? false)) toast('error', $txResp['error'] ?? 'Error');
  else {
    $_SESSION['active_session'] = (string)($txResp['session'] ?? '');
    $_SESSION['active_session_db'] = (string)($txResp['db'] ?? $db);
    $activeSession = $_SESSION['active_session'];
    $activeSessionDb = $_SESSION['active_session_db'];
    toast('success', 'Session abierta: ' . substr($activeSession, 0, 8) . '… sobre ' . $activeSessionDb);
  }
}

if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['tx_commit']) && $activeSession !== '') {
  require_csrf_token();
  [$txResp, $txErr] = http_post_json($server . '/tx/commit?session=' . urlencode($activeSession), [], $apiToken);
  if ($txErr) toast('error', $txErr);
  elseif (!($txResp['ok'] ?? false)) toast('error', $txResp['error'] ?? 'Error');
  else {
    unset($_SESSION['active_session'], $_SESSION['active_session_db']);
    $activeSession = '';
    $activeSessionDb = '';
    toast('success', 'COMMIT — sesión cerrada');
  }
}

if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['tx_rollback']) && $activeSession !== '') {
  require_csrf_token();
  [$txResp, $txErr] = http_post_json($server . '/tx/rollback?session=' . urlencode($activeSession), [], $apiToken);
  if ($txErr) toast('error', $txErr);
  elseif (!($txResp['ok'] ?? false)) toast('error', $txResp['error'] ?? 'Error');
  else {
    unset($_SESSION['active_session'], $_SESSION['active_session_db']);
    $activeSession = '';
    $activeSessionDb = '';
    toast('warning', 'ROLLBACK — cambios descartados, sesión cerrada');
  }
}

// ---------------------------------------------------------------------------
// Action: run SQL (auto-commit OR dentro de sesión activa).
// ---------------------------------------------------------------------------
if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['run_sql'])) {
  require_csrf_token();
  // Si hay sesión activa, manda el header X-Gabysql-Session — el server
  // NO auto-commitea, mantiene el Pager para el próximo request.
  $sessionToUse = ($activeSession !== '' && (!$db || $db === $activeSessionDb)) ? $activeSession : '';
  [$execJson, $execErr] = http_post_json($server . '/exec', ['db' => $db, 'sql' => (string)$sql], $apiToken, $sessionToUse);
  if (!$execErr && !($execJson['ok'] ?? false)) $execErr = $execJson['error'] ?? 'Error';
}

// POST: create RLS policy (Push 5)
if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['create_policy'])) {
  require_csrf_token();
  $p_name   = trim((string)($_POST['p_name']   ?? ''));
  $p_table  = trim((string)($_POST['p_table']  ?? ''));
  $p_action = trim((string)($_POST['p_action'] ?? 'SELECT'));
  $p_role   = trim((string)($_POST['p_role']   ?? ''));
  $p_using  = trim((string)($_POST['p_using']  ?? ''));
  $p_check  = trim((string)($_POST['p_check']  ?? ''));
  $polErr = '';
  if (!preg_match('/^[A-Za-z_][A-Za-z0-9_]*$/', $p_name))  $polErr = 'Nombre inválido.';
  elseif (!preg_match('/^[A-Za-z_][A-Za-z0-9_]*$/', $p_table)) $polErr = 'Tabla inválida.';
  elseif (!in_array($p_action, ['SELECT','INSERT','UPDATE','DELETE','ALL'], true)) $polErr = 'Acción inválida.';
  elseif ($p_using === '' && $p_check === '') $polErr = 'USING o WITH CHECK requerido.';
  elseif ($p_role !== '' && !preg_match('/^[A-Za-z_][A-Za-z0-9_]*$/', $p_role)) $polErr = 'Rol inválido.';
  if ($polErr === '') {
    $sqlGen = "CREATE POLICY {$p_name} ON {$p_table} FOR {$p_action}";
    if ($p_role !== '') $sqlGen .= " TO {$p_role}";
    if ($p_using !== '') $sqlGen .= " USING ({$p_using})";
    if ($p_check !== '') $sqlGen .= " WITH CHECK ({$p_check})";
    $sqlGen .= ";";
    [$polExecJson, $polExecErr] = http_post_json($server . '/exec', ['db' => $db, 'sql' => $sqlGen], $apiToken);
    if (!$polExecErr && !($polExecJson['ok'] ?? false)) $polExecErr = $polExecJson['error'] ?? 'Error';
    if ($polExecErr) toast('error', $polExecErr);
    else toast('success', 'Policy creada: ' . $p_name);
  } else {
    toast('error', $polErr);
  }
}
if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['drop_policy'])) {
  require_csrf_token();
  $dp_name = trim((string)($_POST['dp_name'] ?? ''));
  if (preg_match('/^[A-Za-z_][A-Za-z0-9_]*$/', $dp_name)) {
    [$dpJson, $dpErr] = http_post_json($server . '/exec', ['db' => $db, 'sql' => "DROP POLICY {$dp_name};"], $apiToken);
    if (!$dpErr && !($dpJson['ok'] ?? false)) $dpErr = $dpJson['error'] ?? 'Error';
    if ($dpErr) toast('error', $dpErr);
    else toast('success', 'Policy eliminada: ' . $dp_name);
  } else {
    toast('error', 'Nombre inválido.');
  }
}
?><!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>phpgabyadmin · gabysql</title>
  <link rel="preconnect" href="https://fonts.googleapis.com">
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
  <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap">
  <style>
    :root{
      --bg:#0a0e14; --surface:#11161d; --surface-2:#161c25; --surface-3:#1c2430;
      --border:#21262d; --border-strong:#30363d;
      --text:#e6edf3; --text-muted:#7d8590; --text-soft:#9ca3af;
      --accent:#58a6ff; --accent-hover:#79b8ff;
      --success:#7ee787; --warning:#f0883e; --danger:#ff7b72;
      --shadow-sm:0 1px 3px rgba(0,0,0,.3);
      --shadow:0 8px 24px rgba(0,0,0,.3);
      --shadow-lg:0 24px 64px rgba(0,0,0,.45);
    }
    *{box-sizing:border-box}
    body{margin:0;font-family:'Inter',-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif;
         background:var(--bg);color:var(--text);font-size:14px;line-height:1.5}
    code,pre,.mono{font-family:'JetBrains Mono',ui-monospace,SFMono-Regular,Consolas,monospace}

    /* ---------- App shell ---------- */
    .topbar{display:flex;align-items:center;gap:16px;padding:12px 20px;
            background:var(--surface);border-bottom:1px solid var(--border);
            position:sticky;top:0;z-index:50}
    .brand{display:flex;align-items:center;gap:10px;text-decoration:none;color:inherit}
    .brand-mark{width:32px;height:32px;background:linear-gradient(135deg,var(--accent) 0%,#3d7fc6 100%);
                border-radius:8px;display:flex;align-items:center;justify-content:center;
                font-weight:700;font-size:16px;color:#0a0e14}
    .brand-name{font-weight:600;font-size:16px}
    .topbar-spacer{flex:1}
    .server-pill{display:inline-flex;align-items:center;gap:8px;padding:6px 12px;
                 background:var(--surface-2);border:1px solid var(--border);border-radius:999px;
                 font-size:12px;color:var(--text-muted)}
    .server-pill .dot{width:8px;height:8px;border-radius:50%;background:var(--text-muted)}
    .server-pill.healthy .dot{background:var(--success)}
    .server-pill.down .dot{background:var(--danger)}
    .session-pill{display:inline-flex;align-items:center;gap:6px;padding:6px 12px;
                  background:rgba(255,123,114,.1);border:1px solid rgba(255,123,114,.3);
                  border-radius:999px;font-size:12px;color:var(--danger);font-weight:500}
    .session-pill .pulse{width:8px;height:8px;border-radius:50%;background:var(--danger);
                         animation:pulse 1.5s ease infinite}
    @keyframes pulse{0%,100%{opacity:1}50%{opacity:.4}}

    .layout{display:grid;grid-template-columns:280px 1fr;gap:0;min-height:calc(100vh - 57px)}
    @media(max-width:900px){.layout{grid-template-columns:1fr}}
    .sidebar{background:var(--surface);border-right:1px solid var(--border);padding:16px;
             overflow-y:auto;max-height:calc(100vh - 57px);position:sticky;top:57px}
    .main{padding:20px;min-width:0}

    /* ---------- Sidebar ---------- */
    .sidebar-section{margin-bottom:20px}
    .sidebar-label{font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.05em;
                   color:var(--text-muted);margin-bottom:8px}
    .db-select{width:100%;padding:8px 12px;background:var(--bg);border:1px solid var(--border-strong);
               border-radius:6px;color:var(--text);font-family:inherit;font-size:13px}
    .db-select:focus{outline:none;border-color:var(--accent)}
    .table-list{list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:2px}
    .table-list a{display:flex;align-items:center;gap:8px;padding:8px 10px;border-radius:6px;
                  text-decoration:none;color:var(--text-soft);font-size:13px;transition:all .12s ease}
    .table-list a:hover{background:var(--surface-2);color:var(--text)}
    .table-list a.active{background:rgba(88,166,255,.12);color:var(--accent);font-weight:500}
    .table-list .icon{font-size:11px;opacity:.6}
    .empty-state{color:var(--text-muted);font-size:13px;padding:10px;text-align:center;
                 background:var(--surface-2);border-radius:6px;border:1px dashed var(--border-strong)}

    /* ---------- Cards / panels ---------- */
    .card{background:var(--surface);border:1px solid var(--border);border-radius:10px;padding:18px;
          box-shadow:var(--shadow-sm)}
    .card-header{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-bottom:14px}
    .card-title{font-size:15px;font-weight:600;margin:0}
    .card + .card{margin-top:16px}

    /* ---------- Tabs ---------- */
    .tabs{display:flex;gap:2px;border-bottom:1px solid var(--border);margin-bottom:18px;
          overflow-x:auto;flex-wrap:nowrap}
    .tabs a{padding:10px 14px;font-size:13px;font-weight:500;text-decoration:none;color:var(--text-muted);
            border-bottom:2px solid transparent;margin-bottom:-1px;white-space:nowrap;
            transition:all .12s ease}
    .tabs a:hover{color:var(--text)}
    .tabs a.active{color:var(--accent);border-bottom-color:var(--accent)}

    /* ---------- Buttons ---------- */
    .btn{display:inline-flex;align-items:center;justify-content:center;gap:6px;
         padding:8px 14px;border-radius:6px;border:1px solid transparent;cursor:pointer;
         font-family:inherit;font-size:13px;font-weight:500;text-decoration:none;
         transition:all .12s ease;line-height:1.4;white-space:nowrap}
    .btn:disabled{opacity:.5;cursor:not-allowed}
    .btn-primary{background:var(--accent);color:#0a0e14;border-color:var(--accent)}
    .btn-primary:hover:not(:disabled){background:var(--accent-hover);border-color:var(--accent-hover)}
    .btn-ghost{background:transparent;color:var(--text);border-color:var(--border-strong)}
    .btn-ghost:hover{background:var(--surface-2)}
    .btn-danger{background:transparent;color:var(--danger);border-color:rgba(255,123,114,.4)}
    .btn-danger:hover{background:rgba(255,123,114,.1)}
    .btn-success{background:var(--success);color:#0a0e14;border-color:var(--success)}
    .btn-warning{background:transparent;color:var(--warning);border-color:rgba(240,136,62,.4)}
    .btn-warning:hover{background:rgba(240,136,62,.1)}
    .btn-sm{padding:5px 10px;font-size:12px}

    /* ---------- Inputs ---------- */
    .input{padding:8px 12px;border-radius:6px;border:1px solid var(--border-strong);
           background:var(--bg);color:var(--text);font-family:inherit;font-size:13px;width:100%;
           transition:border-color .15s ease}
    .input:focus{outline:none;border-color:var(--accent);box-shadow:0 0 0 3px rgba(88,166,255,.15)}
    .input-label{display:block;font-size:12px;font-weight:500;color:var(--text-soft);margin-bottom:5px}
    .input-help{font-size:12px;color:var(--text-muted);margin-top:4px}
    textarea.input{font-family:'JetBrains Mono',monospace;min-height:160px;resize:vertical;line-height:1.6}

    /* ---------- Tables ---------- */
    .table-wrapper{overflow-x:auto;border:1px solid var(--border);border-radius:8px}
    table.data{width:100%;border-collapse:collapse;font-size:13px}
    table.data th{background:var(--surface-2);color:var(--text-soft);font-weight:600;
                  text-align:left;padding:10px 14px;font-size:12px;text-transform:uppercase;
                  letter-spacing:.03em;border-bottom:1px solid var(--border)}
    table.data td{padding:10px 14px;border-bottom:1px solid var(--border);color:var(--text)}
    table.data tbody tr:nth-child(even){background:var(--surface-2)}
    table.data tbody tr:hover{background:var(--surface-3)}
    table.data tbody tr:last-child td{border-bottom:none}
    .col-pk{color:var(--success);font-weight:600}
    .col-mono{font-family:'JetBrains Mono',monospace;font-size:12px;color:var(--text-soft)}

    /* ---------- Tags / pills ---------- */
    .tag{display:inline-flex;align-items:center;padding:2px 8px;border-radius:4px;
         font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.03em}
    .tag-pk{background:rgba(126,231,135,.12);color:var(--success);border:1px solid rgba(126,231,135,.3)}
    .tag-idx{background:rgba(88,166,255,.12);color:var(--accent);border:1px solid rgba(88,166,255,.3)}
    .tag-fk{background:rgba(240,136,62,.12);color:var(--warning);border:1px solid rgba(240,136,62,.3)}
    .tag-nn{background:rgba(125,133,144,.12);color:var(--text-soft);border:1px solid var(--border-strong)}
    .tag-unique{background:rgba(176,89,236,.12);color:#b059ec;border:1px solid rgba(176,89,236,.3)}

    /* ---------- Toasts ---------- */
    #toasts{position:fixed;bottom:20px;right:20px;display:flex;flex-direction:column;gap:8px;
            z-index:9999;max-width:380px}
    .toast{padding:12px 16px;border-radius:8px;background:var(--surface);border:1px solid var(--border);
           box-shadow:var(--shadow);display:flex;align-items:center;gap:10px;font-size:13px;
           animation:slideIn .25s ease-out}
    @keyframes slideIn{from{transform:translateX(20px);opacity:0}to{transform:none;opacity:1}}
    .toast-success{border-left:3px solid var(--success)}
    .toast-error{border-left:3px solid var(--danger)}
    .toast-warning{border-left:3px solid var(--warning)}
    .toast-info{border-left:3px solid var(--accent)}
    .toast .ico{font-size:16px}

    /* ---------- Alerts ---------- */
    .alert{padding:12px 14px;border-radius:8px;font-size:13px;margin-bottom:14px;
           display:flex;align-items:flex-start;gap:10px}
    .alert-error{background:rgba(255,123,114,.08);border:1px solid rgba(255,123,114,.25);color:#ffd1cd}
    .alert-success{background:rgba(126,231,135,.08);border:1px solid rgba(126,231,135,.25);color:#d4f7d4}
    .alert-warning{background:rgba(240,136,62,.08);border:1px solid rgba(240,136,62,.25);color:#fae0ce}
    .alert-info{background:rgba(88,166,255,.08);border:1px solid rgba(88,166,255,.25);color:#cde3ff}

    /* ---------- Bias colors for EXPLAIN ANALYZE ---------- */
    .bias-good{color:var(--success)}
    .bias-mild{color:var(--warning)}
    .bias-high{color:var(--danger);font-weight:600}
    .bias-match{color:var(--text-muted)}

    /* ---------- Utilities ---------- */
    .muted{color:var(--text-muted)}
    .row{display:flex;gap:10px;align-items:center;flex-wrap:wrap}
    .row-tight{display:flex;gap:6px;align-items:center;flex-wrap:wrap}
    .stack-sm > * + *{margin-top:8px}
    .stack > * + *{margin-top:14px}
    .grid-2{display:grid;grid-template-columns:1fr 1fr;gap:14px}
    @media(max-width:700px){.grid-2{grid-template-columns:1fr}}
    hr.sep{border:0;border-top:1px solid var(--border);margin:16px 0}
    details summary{cursor:pointer;color:var(--text-muted);font-size:13px;padding:8px 0}
    details[open] summary{color:var(--text)}
    .snippet-bar{display:flex;gap:6px;flex-wrap:wrap;margin-bottom:8px}
    .snippet{padding:5px 10px;font-size:12px;background:var(--surface-2);border:1px solid var(--border);
             border-radius:6px;text-decoration:none;color:var(--text-soft);transition:all .12s ease}
    .snippet:hover{background:var(--surface-3);color:var(--text);border-color:var(--border-strong)}
    pre.code{background:var(--bg);border:1px solid var(--border);border-radius:8px;
             padding:14px;overflow:auto;font-size:12px;line-height:1.5}
    .pager{display:flex;justify-content:space-between;align-items:center;margin-top:14px;
           padding:10px 0;font-size:13px;color:var(--text-muted)}
  </style>
</head>
<body>

<header class="topbar">
  <a class="brand" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>">
    <div class="brand-mark">▣</div>
    <div class="brand-name">phpgabyadmin</div>
  </a>
  <span class="muted" style="font-size:12px">v2 · gabysql · admin</span>
  <div class="topbar-spacer"></div>
  <?php if ($activeSession): ?>
    <span class="session-pill" title="Sesión cross-request M13 abierta sobre <?= htmlspecialchars($activeSessionDb) ?>">
      <span class="pulse"></span>
      Session <?= htmlspecialchars(substr($activeSession, 0, 8)) ?>… activa
    </span>
  <?php endif; ?>
  <span class="server-pill <?= $serverHealthy ? 'healthy' : ($serverErr || $dbsErr ? 'down' : '') ?>"
        title="<?= $serverHealthy ? 'Server respondió a /health' : 'Server no responde a /health' ?>">
    <span class="dot"></span>
    <?= htmlspecialchars($server) ?>
  </span>
  <?php if ($uiToken): ?>
    <form method="post" style="display:inline">
      <?= csrf_field() ?>
      <button class="btn btn-ghost btn-sm" name="logout" type="submit">Salir</button>
    </form>
  <?php endif; ?>
</header>

<?php if ($serverErr): ?>
  <div style="padding:14px 20px">
    <div class="alert alert-error"><span>⚠️</span> <?= htmlspecialchars($serverErr) ?></div>
  </div>
<?php endif; ?>

<div class="layout">

  <!-- ============ SIDEBAR ============ -->
  <aside class="sidebar">
    <div class="sidebar-section">
      <div class="sidebar-label">Conexión</div>
      <form method="get" class="stack-sm">
        <div>
          <label class="input-label">Server</label>
          <input class="input" type="text" name="server" value="<?= htmlspecialchars($server) ?>"/>
        </div>
        <div>
          <label class="input-label">API token (opcional)</label>
          <input class="input" type="text" name="token" value="<?= htmlspecialchars($apiToken) ?>"/>
        </div>
        <button class="btn btn-ghost" style="width:100%" type="submit">Aplicar</button>
      </form>
    </div>

    <div class="sidebar-section">
      <div class="sidebar-label">Bases de datos</div>
      <?php if ($dbsErr): ?>
        <div class="alert alert-error" style="margin:0"><?= htmlspecialchars($dbsErr) ?></div>
        <pre class="code" style="margin-top:10px;font-size:11px">cargo run --release --bin gabysql-server -- -db demo.db -addr :8080</pre>
      <?php else: ?>
        <form method="get" class="stack-sm">
          <input type="hidden" name="server" value="<?= htmlspecialchars($server) ?>"/>
          <input type="hidden" name="token" value="<?= htmlspecialchars($apiToken) ?>"/>
          <select class="db-select" name="db" onchange="this.form.submit()">
            <?php foreach ($dbs as $dbName): ?>
              <option value="<?= htmlspecialchars((string)$dbName) ?>" <?= (string)$dbName === $db ? 'selected' : '' ?>><?= htmlspecialchars((string)$dbName) ?></option>
            <?php endforeach; ?>
          </select>
        </form>
        <?php if ($serverMode === 'multi-db'): ?>
          <form method="post" class="row" style="margin-top:10px">
            <?= csrf_field() ?>
            <input class="input" type="text" name="new_db" placeholder="nueva.db" style="flex:1"/>
            <button class="btn btn-ghost btn-sm" type="submit">+ DB</button>
          </form>
        <?php endif; ?>
      <?php endif; ?>
    </div>

    <div class="sidebar-section">
      <div class="sidebar-label">Tablas <?= !empty($tables) ? '<span class="muted">· ' . count($tables) . '</span>' : '' ?></div>
      <?php if ($tablesErr): ?>
        <div class="alert alert-error" style="margin:0"><?= htmlspecialchars($tablesErr) ?></div>
      <?php elseif (empty($tables)): ?>
        <div class="empty-state">Sin tablas</div>
      <?php else: ?>
        <ul class="table-list">
          <?php foreach ($tables as $tableInfo):
            $tableName = (string)($tableInfo['name'] ?? 'tabla');
            $isActive = $tableName === $table;
          ?>
            <li><a href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($tableName) ?>&tab=browse" class="<?= $isActive ? 'active' : '' ?>">
              <span class="icon">▦</span><?= htmlspecialchars($tableName) ?>
            </a></li>
          <?php endforeach; ?>
        </ul>
      <?php endif; ?>
    </div>
  </aside>

  <!-- ============ MAIN ============ -->
  <main class="main">

    <?php if ($db === ''): ?>
      <div class="card">
        <div class="card-header"><h2 class="card-title">Bienvenido a phpgabyadmin v2</h2></div>
        <p class="muted">Seleccioná una base de datos en la izquierda para empezar. Las tabs disponibles son:</p>
        <ul class="muted" style="line-height:1.9">
          <li><b>Browse</b> · explorar datos con paginación + export CSV</li>
          <li><b>Structure</b> · columnas, índices, FKs, CHECK constraints</li>
          <li><b>SQL</b> · editor con snippets, dentro o fuera de sesión</li>
          <li><b>Sessions</b> · abrir/cerrar transacciones cross-request HTTP (M13)</li>
          <li><b>Explain</b> · correr <code>EXPLAIN ANALYZE</code> con bias coloreado (M6)</li>
        </ul>
      </div>
    <?php else: ?>

      <!-- Top tabs -->
      <nav class="tabs">
        <?php
          $tabLinks = [
            'browse'    => ['Browse', '▦', $table !== ''],
            'structure' => ['Structure', '⛁', $table !== ''],
            'sql'       => ['SQL editor', '➤', true],
            'sessions'  => ['Sessions (M13)', '↻', true],
            'explain'   => ['Explain (M6)', '⊕', true],
            'stats'     => ['Stats', '📊', true],
            'policies'  => ['Policies (RLS)', '🔒', true],
          ];
          foreach ($tabLinks as $k => $info):
            [$label, $icon, $enabled] = $info;
            if (!$enabled) continue;
            $active = $tab === $k ? 'active' : '';
            $href = "?server=" . urlencode($server) . "&token=" . urlencode($apiToken) . "&db=" . urlencode($db);
            if ($table !== '') $href .= "&table=" . urlencode($table);
            $href .= "&tab=" . $k;
        ?>
          <a class="<?= $active ?>" href="<?= $href ?>"><span style="opacity:.6;margin-right:4px"><?= $icon ?></span><?= $label ?></a>
        <?php endforeach; ?>
      </nav>

      <?php if ($table === '' && in_array($tab, ['browse','structure'], true)): ?>
        <div class="card"><div class="muted">Seleccioná una tabla en la izquierda.</div></div>

      <?php elseif ($tab === 'structure'): ?>
        <?php [$schemaResp, $schemaErr] = http_get_json($server . '/schema?db=' . urlencode($db) . '&table=' . urlencode($table), $apiToken); ?>
        <?php if ($schemaErr || !($schemaResp['ok'] ?? false)): ?>
          <div class="alert alert-error"><?= htmlspecialchars($schemaErr ?: ($schemaResp['error'] ?? 'Error')) ?></div>
        <?php else: $meta = $schemaResp['table'] ?? []; ?>
          <div class="card">
            <div class="card-header">
              <h2 class="card-title">Columnas · <code><?= htmlspecialchars($table) ?></code></h2>
              <span class="muted"><?= count($meta['columns'] ?? []) ?> columnas</span>
            </div>
            <?php
              $indexedCols = [];
              foreach (($meta['indexes'] ?? []) as $idx) {
                $indexedCols[strtolower((string)($idx['column'] ?? ''))] = (string)($idx['name'] ?? '');
              }
            ?>
            <div class="table-wrapper">
              <table class="data">
                <thead><tr><th>Columna</th><th>Tipo</th><th>Constraints</th><th>Índice</th></tr></thead>
                <tbody>
                  <?php foreach (($meta['columns'] ?? []) as $column):
                    $colName = (string)($column['name'] ?? '');
                    $colKey = strtolower($colName);
                    $colType = (string)($column['type'] ?? '');
                  ?>
                    <tr>
                      <td class="<?= !empty($column['pk']) ? 'col-pk' : '' ?>"><?= htmlspecialchars($colName) ?></td>
                      <td class="col-mono"><?= htmlspecialchars($colType) ?></td>
                      <td>
                        <div class="row-tight">
                          <?php if (!empty($column['pk'])): ?><span class="tag tag-pk">PK</span><?php endif; ?>
                          <?php if (!empty($column['notNull']) && empty($column['pk'])): ?><span class="tag tag-nn">NOT NULL</span><?php endif; ?>
                          <?php if (!empty($column['unique'])): ?><span class="tag tag-unique">UNIQUE</span><?php endif; ?>
                          <?php if (!empty($column['references'])): ?><span class="tag tag-fk">FK</span><?php endif; ?>
                          <?php if (isset($column['default'])): ?><span class="muted" style="font-size:11px">DEFAULT <?= htmlspecialchars((string)$column['default']) ?></span><?php endif; ?>
                        </div>
                      </td>
                      <td><?= isset($indexedCols[$colKey]) ? '<code style="font-size:11px;color:var(--accent)">' . htmlspecialchars($indexedCols[$colKey]) . '</code>' : '<span class="muted">—</span>' ?></td>
                    </tr>
                  <?php endforeach; ?>
                </tbody>
              </table>
            </div>
          </div>

          <div class="card">
            <div class="card-header">
              <h2 class="card-title">Índices secundarios</h2>
              <span class="muted"><?= count($meta['indexes'] ?? []) ?></span>
            </div>
            <?php $indexes = $meta['indexes'] ?? []; ?>
            <?php if (empty($indexes)): ?>
              <div class="empty-state">Esta tabla no tiene índices secundarios.</div>
            <?php else: ?>
              <div class="table-wrapper">
                <table class="data">
                  <thead><tr><th>Nombre</th><th>Columna</th><th>Root page</th><th>Tipo</th><th></th></tr></thead>
                  <tbody>
                    <?php foreach ($indexes as $idx): $idxName = (string)($idx['name'] ?? ''); ?>
                      <tr>
                        <td class="col-mono"><?= htmlspecialchars($idxName) ?></td>
                        <td><?= htmlspecialchars((string)($idx['column'] ?? '')) ?></td>
                        <td class="muted col-mono"><?= htmlspecialchars((string)($idx['rootPage'] ?? '')) ?></td>
                        <td><span class="tag tag-idx"><?= htmlspecialchars((string)($idx['kind'] ?? 'Hash')) ?></span></td>
                        <td>
                          <form method="post" style="display:inline" onsubmit="return confirm('Eliminar índice <?= htmlspecialchars($idxName, ENT_QUOTES) ?>?');">
                            <?= csrf_field() ?>
                            <input type="hidden" name="sql" value="DROP INDEX <?= htmlspecialchars($idxName, ENT_QUOTES) ?>;"/>
                            <button class="btn btn-danger btn-sm" name="run_sql" type="submit">DROP</button>
                          </form>
                        </td>
                      </tr>
                    <?php endforeach; ?>
                  </tbody>
                </table>
              </div>
            <?php endif; ?>

            <hr class="sep"/>
            <h3 style="margin:0 0 10px;font-size:13px;font-weight:600">Crear nuevo índice</h3>
            <form method="post" class="row">
              <?= csrf_field() ?>
              <input type="hidden" name="create_index_table" value="<?= htmlspecialchars((string)$table) ?>"/>
              <input class="input" name="create_index_name" placeholder="idx_<?= htmlspecialchars((string)$table) ?>_col" required style="flex:1;min-width:180px"/>
              <select class="input" name="create_index_column" style="flex:1;min-width:160px">
                <?php foreach (($meta['columns'] ?? []) as $column):
                  if (!empty($column['pk'])) continue;
                  $cName = (string)($column['name'] ?? ''); $cType = (string)($column['type'] ?? '');
                  if (strcasecmp($cType, 'JSON') === 0) continue;
                ?>
                  <option value="<?= htmlspecialchars($cName) ?>"><?= htmlspecialchars($cName . ' (' . $cType . ')') ?></option>
                <?php endforeach; ?>
              </select>
              <button class="btn btn-primary" name="create_index" type="submit">CREATE INDEX</button>
            </form>
            <div class="input-help">Una columna por índice (single-col). PK y JSON no se indexan secundario.</div>
          </div>
        <?php endif; ?>

      <?php elseif ($tab === 'browse'): ?>
        <?php
          $limit = max(1, min(200, intval($_GET['limit'] ?? 25)));
          $offset = max(0, intval($_GET['offset'] ?? 0));
          [$rowsResp, $rowsErr] = http_get_json($server . '/rows?db=' . urlencode($db) . '&table=' . urlencode($table) . '&limit=' . $limit . '&offset=' . $offset, $apiToken);
        ?>
        <?php if ($rowsErr || !($rowsResp['ok'] ?? false)): ?>
          <div class="alert alert-error"><?= htmlspecialchars($rowsErr ?: ($rowsResp['error'] ?? 'Error')) ?></div>
        <?php else: $cols = $rowsResp['columns'] ?? []; $rows = $rowsResp['rows'] ?? []; $total = intval($rowsResp['total'] ?? 0); ?>
          <div class="card">
            <div class="card-header">
              <h2 class="card-title"><?= htmlspecialchars($table) ?> <span class="muted" style="font-weight:400">· <?= $total ?> filas</span></h2>
              <div class="row-tight">
                <a class="btn btn-ghost btn-sm" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($table) ?>&export=1">⬇ Export CSV</a>
              </div>
            </div>

            <?php if (empty($rows)): ?>
              <div class="empty-state">Tabla vacía.</div>
            <?php else: ?>
              <div class="table-wrapper">
                <table class="data">
                  <thead><tr><?php foreach ($cols as $col): ?><th><?= htmlspecialchars((string)$col) ?></th><?php endforeach; ?></tr></thead>
                  <tbody>
                    <?php foreach ($rows as $row): ?>
                      <tr><?php foreach ($row as $cell): ?><td class="col-mono"><?= htmlspecialchars((string)$cell) ?></td><?php endforeach; ?></tr>
                    <?php endforeach; ?>
                  </tbody>
                </table>
              </div>
            <?php endif; ?>

            <div class="pager">
              <span>Filas <?= $offset + 1 ?>–<?= min($offset + $limit, $total) ?> de <?= $total ?> · limit <?= $limit ?></span>
              <div class="row-tight">
                <?php $prev = max(0, $offset - $limit); $next = $offset + $limit; ?>
                <a class="btn btn-ghost btn-sm" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($table) ?>&tab=browse&limit=<?= $limit ?>&offset=<?= $prev ?>">← Prev</a>
                <a class="btn btn-ghost btn-sm" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($table) ?>&tab=browse&limit=<?= $limit ?>&offset=<?= $next ?>">Next →</a>
              </div>
            </div>
          </div>

          <div class="card">
            <div class="card-header"><h2 class="card-title">Import CSV</h2></div>
            <p class="muted" style="font-size:13px;margin:0 0 10px">El header del CSV se usa como nombres de columna. Los inserts se ejecutan en una sola transacción.</p>
            <form method="post" enctype="multipart/form-data" class="row">
              <?= csrf_field() ?>
              <input type="file" name="csv" accept=".csv" class="input" style="flex:1;padding:6px 8px"/>
              <button class="btn btn-primary" name="import_csv" type="submit">↑ Import</button>
            </form>
          </div>
        <?php endif; ?>

      <?php elseif ($tab === 'sessions'): ?>
        <div class="card">
          <div class="card-header">
            <h2 class="card-title">Sessions cross-request (M13)</h2>
            <span class="muted" style="font-size:12px">ANSI SQL:2003 · ADR-0090</span>
          </div>
          <p class="muted" style="font-size:13px">Una sesión mantiene la transacción abierta entre requests HTTP. Mientras esté activa, el SQL editor mandará el header <code>X-Gabysql-Session</code> y el server NO auto-commitea. ORMs (SQLAlchemy/Diesel/Hibernate) pueden usar este patrón.</p>

          <?php if ($activeSession): ?>
            <div class="alert alert-warning">
              <span>↻</span>
              <div style="flex:1">
                <b>Sesión activa</b> sobre <code><?= htmlspecialchars($activeSessionDb) ?></code><br>
                <span class="mono muted" style="font-size:11px">ID: <?= htmlspecialchars($activeSession) ?></span>
              </div>
            </div>
            <div class="row">
              <form method="post" style="display:inline">
                <?= csrf_field() ?>
                <button class="btn btn-success" name="tx_commit" type="submit">✓ COMMIT (persistir)</button>
              </form>
              <form method="post" style="display:inline" onsubmit="return confirm('Esto descarta los cambios desde el BEGIN. Continuar?');">
                <?= csrf_field() ?>
                <button class="btn btn-danger" name="tx_rollback" type="submit">⨯ ROLLBACK (descartar)</button>
              </form>
            </div>
          <?php else: ?>
            <div class="alert alert-info"><span>ⓘ</span> No hay sesión activa. Cualquier <code>/exec</code> es auto-commit clásico.</div>
            <form method="post" style="display:inline">
              <?= csrf_field() ?>
              <button class="btn btn-primary" name="tx_begin" type="submit">↻ Abrir sesión sobre <code><?= htmlspecialchars($db) ?></code></button>
            </form>
          <?php endif; ?>

          <hr class="sep"/>
          <h3 style="margin:0 0 10px;font-size:13px;font-weight:600">Workflow típico con SAVEPOINT (M12)</h3>
          <pre class="code">BEGIN  <span style="color:var(--text-muted)">-- implícito al abrir la sesión</span>
INSERT INTO ledger VALUES (1, 100);
SAVEPOINT before_risky_import;
INSERT INTO ledger SELECT * FROM external_csv;  <span style="color:var(--text-muted)">-- puede fallar</span>
ROLLBACK TO SAVEPOINT before_risky_import;       <span style="color:var(--text-muted)">-- vuelve al checkpoint, mantiene tx</span>
INSERT INTO ledger VALUES (2, 200);
COMMIT  <span style="color:var(--text-muted)">-- cierra la sesión</span></pre>
        </div>

      <?php elseif ($tab === 'explain'): ?>
        <?php
          // EXPLAIN ANALYZE handler
          $explainSql = (string)($_POST['explain_sql'] ?? $_GET['explain_sql'] ?? "SELECT * FROM users WHERE id = 1;");
          $explainJson = null;
          $explainErr = null;
          if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['run_explain'])) {
            require_csrf_token();
            $finalSql = 'EXPLAIN ANALYZE ' . ltrim($explainSql);
            [$explainJson, $explainErr] = http_post_json($server . '/exec', ['db' => $db, 'sql' => $finalSql], $apiToken);
            if (!$explainErr && !($explainJson['ok'] ?? false)) $explainErr = $explainJson['error'] ?? 'Error';
          }
        ?>
        <div class="card">
          <div class="card-header">
            <h2 class="card-title">EXPLAIN ANALYZE con bias del estimator (M6)</h2>
            <span class="muted" style="font-size:12px">ADR-0088</span>
          </div>
          <p class="muted" style="font-size:13px">Corre la query real + compara <code>est.match</code> (lo que el estimator calculó) vs <code>actual</code> (lo que devolvió). Banda <span class="bias-good">GOOD</span>, <span class="bias-mild">MILD</span>, <span class="bias-high">HIGH</span>, <span class="bias-match">MATCH</span>. Solo aplica a SELECTs scan-only (sin JOIN/GROUP BY/LIMIT).</p>
          <form method="post" class="stack-sm">
            <?= csrf_field() ?>
            <textarea class="input" name="explain_sql"><?= htmlspecialchars($explainSql) ?></textarea>
            <div class="row">
              <button class="btn btn-primary" name="run_explain" type="submit">▶ EXPLAIN ANALYZE</button>
              <span class="muted" style="font-size:12px">⚠️ side-effects PERSISTEN si el query muta — usá <code>EXPLAIN</code> sin ANALYZE para dry-run.</span>
            </div>
          </form>

          <?php if ($explainErr): ?>
            <div class="alert alert-error" style="margin-top:14px"><?= htmlspecialchars($explainErr) ?></div>
          <?php elseif ($explainJson && ($explainJson['ok'] ?? false)): ?>
            <hr class="sep"/>
            <?php foreach (($explainJson['results'] ?? []) as $result):
              $cols = $result['columns'] ?? [];
              $rows = $result['rows'] ?? [];
              $msg = $result['message'] ?? '';
            ?>
              <?php if ($msg): ?><div class="alert alert-info"><span>ⓘ</span> <?= htmlspecialchars($msg) ?></div><?php endif; ?>
              <?php if (count($cols) > 0): ?>
                <div class="table-wrapper">
                  <table class="data">
                    <thead><tr><?php foreach ($cols as $col): ?><th><?= htmlspecialchars((string)$col) ?></th><?php endforeach; ?></tr></thead>
                    <tbody>
                      <?php foreach ($rows as $row):
                        $step = (string)($row[0] ?? '');
                        $detail = (string)($row[1] ?? '');
                        $biasClass = '';
                        if ($step === 'actual.bias') {
                          if (strpos($detail, 'BIAS=GOOD') !== false) $biasClass = 'bias-good';
                          elseif (strpos($detail, 'BIAS=MILD') !== false) $biasClass = 'bias-mild';
                          elseif (strpos($detail, 'BIAS=HIGH') !== false) $biasClass = 'bias-high';
                          elseif (strpos($detail, 'BIAS=MATCH') !== false) $biasClass = 'bias-match';
                        }
                      ?>
                        <tr>
                          <td class="col-mono <?= $biasClass ?>"><?= htmlspecialchars($step) ?></td>
                          <td class="col-mono <?= $biasClass ?>" style="font-size:11px"><?= htmlspecialchars($detail) ?></td>
                        </tr>
                      <?php endforeach; ?>
                    </tbody>
                  </table>
                </div>
              <?php endif; ?>
            <?php endforeach; ?>
          <?php endif; ?>
        </div>

      <?php elseif ($tab === 'sql'): ?>
        <div class="card">
          <div class="card-header">
            <h2 class="card-title">SQL editor</h2>
            <?php if ($activeSession): ?>
              <span class="session-pill" style="font-size:11px">↻ Dentro de sesión</span>
            <?php else: ?>
              <span class="muted" style="font-size:12px">Auto-commit por request</span>
            <?php endif; ?>
          </div>

          <?php if ($table !== ''): ?>
            <div class="snippet-bar">
              <?php
                $snippets = [
                  'SELECT'        => "SELECT * FROM {$table} LIMIT 25;",
                  'PK lookup'     => "SELECT * FROM {$table} WHERE id = 1;",
                  'INSERT'        => "INSERT INTO {$table} (id) VALUES (1);",
                  'UPDATE PK'     => "UPDATE {$table} SET ... WHERE id = 1;",
                  'DELETE PK'     => "DELETE FROM {$table} WHERE id = 1;",
                  'CREATE INDEX'  => "CREATE INDEX idx_{$table}_col ON {$table} (col);",
                  'EXPLAIN'       => "EXPLAIN SELECT * FROM {$table} WHERE id = 1;",
                  'ANALYZE'       => "ANALYZE TABLE {$table};",
                  'SAVEPOINT'     => "SAVEPOINT sp1; -- ROLLBACK TO SAVEPOINT sp1; -- RELEASE SAVEPOINT sp1;",
                ];
              ?>
              <?php foreach ($snippets as $label => $tpl): ?>
                <a class="snippet" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($table) ?>&tab=sql&prefill=<?= urlencode($tpl) ?>"><?= htmlspecialchars($label) ?></a>
              <?php endforeach; ?>
            </div>
          <?php endif; ?>
          <?php if (isset($_GET['prefill'])) { $sql = (string)$_GET['prefill']; } ?>

          <form method="post" class="stack-sm">
            <?= csrf_field() ?>
            <textarea class="input" name="sql"><?= htmlspecialchars((string)$sql) ?></textarea>
            <div class="row">
              <button class="btn btn-primary" name="run_sql" type="submit">▶ Ejecutar</button>
              <?php if ($activeSession): ?>
                <span class="muted" style="font-size:12px">Va a la sesión activa · no auto-commit.</span>
              <?php else: ?>
                <span class="muted" style="font-size:12px">BEGIN → exec → COMMIT en un request. <a href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&tab=sessions">Abrir sesión cross-request →</a></span>
              <?php endif; ?>
            </div>
          </form>

          <?php if ($execErr): ?>
            <div class="alert alert-error" style="margin-top:14px"><span>⚠️</span><?= htmlspecialchars($execErr) ?></div>
          <?php elseif ($execJson && ($execJson['ok'] ?? false)): ?>
            <div class="alert alert-success" style="margin-top:14px"><span>✓</span>Ejecución OK</div>
            <?php foreach (($execJson['results'] ?? []) as $index => $result):
              $cols = $result['columns'] ?? [];
              $rows = $result['rows'] ?? [];
              $message = $result['message'] ?? '';
            ?>
              <?php if ($message): ?><div class="muted" style="font-size:13px;margin-top:10px"><?= htmlspecialchars((string)$message) ?></div><?php endif; ?>
              <?php if (is_array($cols) && count($cols) > 0): ?>
                <div class="table-wrapper" style="margin-top:10px">
                  <table class="data">
                    <thead><tr><?php foreach ($cols as $col): ?><th><?= htmlspecialchars((string)$col) ?></th><?php endforeach; ?></tr></thead>
                    <tbody>
                      <?php foreach ($rows as $row): ?>
                        <tr><?php foreach ($row as $cell): ?><td class="col-mono"><?= htmlspecialchars((string)$cell) ?></td><?php endforeach; ?></tr>
                      <?php endforeach; ?>
                    </tbody>
                  </table>
                </div>
              <?php endif; ?>
            <?php endforeach; ?>
            <details style="margin-top:12px">
              <summary>Ver JSON crudo</summary>
              <pre class="code"><?= htmlspecialchars(json_encode($execJson, JSON_PRETTY_PRINT | JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES)) ?></pre>
            </details>
          <?php endif; ?>
        </div>

      <?php elseif ($tab === 'stats'): ?>
        <?php
          // Counts derivados de $tables (ya cargado del sidebar via /tables).
          $statTotalTables  = count($tables);
          $statTotalCols    = 0;
          $statTotalIdx     = 0;
          $statTotalFks     = 0;
          $statTotalChecks  = 0;
          $statTotalPK      = 0;
          $perTable = [];
          foreach ($tables as $tInfo) {
            $tn   = $tInfo['name'] ?? '(?)';
            $cols = $tInfo['columns'] ?? [];
            $idxs = $tInfo['indexes'] ?? [];
            $nCol = count($cols);
            $nIdx = count($idxs);
            $nFk  = 0; $nChk = 0; $nPk = 0;
            foreach ($cols as $c) {
              if (!empty($c['references'])) $nFk++;
              if (!empty($c['check']))      $nChk++;
              if (!empty($c['pk']))         $nPk++;
            }
            $statTotalCols   += $nCol;
            $statTotalIdx    += $nIdx;
            $statTotalFks    += $nFk;
            $statTotalChecks += $nChk;
            $statTotalPK     += $nPk;
            $perTable[] = compact('tn','nCol','nIdx','nFk','nChk','nPk');
          }
          $totalDbs = is_array($dbsResp['databases'] ?? null) ? count($dbsResp['databases']) : 0;
        ?>
        <div class="card">
          <div class="card-header">
            <h2 class="card-title">📊 Estadísticas · <code><?= htmlspecialchars($db) ?></code></h2>
            <span class="muted">Derivado de <code>/tables</code> y <code>/dbs</code></span>
          </div>
          <div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:14px;margin-top:6px">
            <?php
              $kpis = [
                ['DBs (server)',  $totalDbs,        'var(--accent)'],
                ['Tablas',        $statTotalTables, 'var(--accent)'],
                ['Columnas',      $statTotalCols,   'var(--text)'],
                ['Índices',       $statTotalIdx,    'var(--accent)'],
                ['FKs',           $statTotalFks,    'var(--warning)'],
                ['CHECK',         $statTotalChecks, 'var(--success)'],
                ['PKs',           $statTotalPK,     'var(--success)'],
              ];
              foreach ($kpis as [$lbl, $val, $color]):
            ?>
              <div style="background:var(--surface-2);border:1px solid var(--border);border-radius:8px;padding:14px">
                <div style="font-size:11px;text-transform:uppercase;letter-spacing:.05em;color:var(--text-muted);font-weight:600"><?= htmlspecialchars($lbl) ?></div>
                <div style="font-family:'JetBrains Mono',monospace;font-size:24px;font-weight:600;color:<?= $color ?>;margin-top:4px"><?= (int)$val ?></div>
              </div>
            <?php endforeach; ?>
          </div>
        </div>

        <?php if (!empty($perTable)): ?>
          <div class="card">
            <div class="card-header"><h2 class="card-title">Breakdown por tabla</h2></div>
            <div class="table-wrapper">
              <table class="data">
                <thead><tr>
                  <th>Tabla</th><th>Cols</th><th>PK</th><th>Índices</th><th>FKs</th><th>CHECKs</th>
                </tr></thead>
                <tbody>
                  <?php foreach ($perTable as $r): ?>
                    <tr>
                      <td class="col-mono"><?= htmlspecialchars($r['tn']) ?></td>
                      <td class="col-mono"><?= (int)$r['nCol'] ?></td>
                      <td class="col-mono"><?= (int)$r['nPk'] ?></td>
                      <td class="col-mono"><?= (int)$r['nIdx'] ?></td>
                      <td class="col-mono"><?= (int)$r['nFk'] ?></td>
                      <td class="col-mono"><?= (int)$r['nChk'] ?></td>
                    </tr>
                  <?php endforeach; ?>
                </tbody>
              </table>
            </div>
          </div>
        <?php endif; ?>

        <?php [$metricsResp, $metricsErr] = http_get_json($server . '/metrics', $apiToken); ?>
        <?php if (!$metricsErr && is_array($metricsResp)): ?>
          <div class="card">
            <div class="card-header"><h2 class="card-title">Server metrics · <code>/metrics</code></h2></div>
            <pre class="code" style="max-height:320px;overflow:auto"><?= htmlspecialchars(json_encode($metricsResp, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE)) ?></pre>
          </div>
        <?php endif; ?>

      <?php elseif ($tab === 'policies'): ?>
        <div class="card">
          <div class="card-header">
            <h2 class="card-title">🔒 RLS Policies · <code><?= htmlspecialchars($db) ?></code></h2>
            <span class="muted">CREATE POLICY · DROP POLICY</span>
          </div>
          <p class="muted" style="font-size:13px;margin-bottom:14px">
            Las policies de Row-Level Security restringen qué filas ve cada rol.
            <code>USING</code> aplica al SELECT/UPDATE/DELETE; <code>WITH CHECK</code> aplica a INSERT/UPDATE.
          </p>
          <form method="post" class="stack-sm">
            <?= csrf_field() ?>
            <div style="display:grid;grid-template-columns:1fr 1fr;gap:12px">
              <div>
                <label class="muted" style="font-size:11px;text-transform:uppercase;font-weight:600">Nombre</label>
                <input class="input" type="text" name="p_name" placeholder="p_orders_self" required />
              </div>
              <div>
                <label class="muted" style="font-size:11px;text-transform:uppercase;font-weight:600">Tabla</label>
                <select class="input" name="p_table" required>
                  <?php foreach ($tables as $tInfo): ?>
                    <option value="<?= htmlspecialchars($tInfo['name']) ?>"><?= htmlspecialchars($tInfo['name']) ?></option>
                  <?php endforeach; ?>
                </select>
              </div>
              <div>
                <label class="muted" style="font-size:11px;text-transform:uppercase;font-weight:600">Acción</label>
                <select class="input" name="p_action">
                  <option value="SELECT">SELECT</option>
                  <option value="INSERT">INSERT</option>
                  <option value="UPDATE">UPDATE</option>
                  <option value="DELETE">DELETE</option>
                  <option value="ALL">ALL</option>
                </select>
              </div>
              <div>
                <label class="muted" style="font-size:11px;text-transform:uppercase;font-weight:600">Rol (opcional)</label>
                <input class="input" type="text" name="p_role" placeholder="bob" />
              </div>
            </div>
            <div>
              <label class="muted" style="font-size:11px;text-transform:uppercase;font-weight:600">USING (read-side; SELECT/UPDATE/DELETE)</label>
              <textarea class="input" name="p_using" rows="2" placeholder="user_id = current_user_id()"></textarea>
            </div>
            <div>
              <label class="muted" style="font-size:11px;text-transform:uppercase;font-weight:600">WITH CHECK (write-side; INSERT/UPDATE)</label>
              <textarea class="input" name="p_check" rows="2" placeholder="amount <= 100000.00"></textarea>
            </div>
            <div class="row">
              <button class="btn btn-primary" name="create_policy" type="submit">＋ Crear policy</button>
              <span class="muted" style="font-size:12px">Emite <code>CREATE POLICY name ON table FOR action [TO role] [USING (...)] [WITH CHECK (...)];</code></span>
            </div>
          </form>
        </div>

        <div class="card">
          <div class="card-header"><h2 class="card-title">Eliminar policy</h2></div>
          <form method="post" class="row" style="gap:10px;flex-wrap:wrap">
            <?= csrf_field() ?>
            <input class="input" type="text" name="dp_name" placeholder="nombre exacto de la policy" required style="flex:1;min-width:240px" />
            <button class="btn btn-danger" name="drop_policy" type="submit">🗑 DROP POLICY</button>
          </form>
          <p class="muted" style="font-size:12px;margin-top:10px">
            El listado completo de policies (GET /policies) se agrega cuando el server
            exponga el endpoint. Hoy podés crear, eliminar y consultarlas vía SQL.
          </p>
        </div>

      <?php endif; ?>

    <?php endif; ?>
  </main>
</div>

<!-- Toast notifications -->
<?php if (!empty($toasts)): ?>
  <div id="toasts">
    <?php foreach ($toasts as $t):
      $kind = $t['kind'];
      $ico = $kind === 'success' ? '✓' : ($kind === 'error' ? '✗' : ($kind === 'warning' ? '!' : 'ⓘ'));
    ?>
      <div class="toast toast-<?= htmlspecialchars($kind) ?>">
        <span class="ico"><?= $ico ?></span>
        <span><?= htmlspecialchars($t['msg']) ?></span>
      </div>
    <?php endforeach; ?>
  </div>
  <script>
    // Auto-dismiss toasts después de 5s
    setTimeout(() => {
      document.querySelectorAll('.toast').forEach(el => {
        el.style.transition = 'opacity .4s ease, transform .4s ease';
        el.style.opacity = '0';
        el.style.transform = 'translateX(20px)';
        setTimeout(() => el.remove(), 500);
      });
    }, 5000);
  </script>
<?php endif; ?>

</body>
</html>
