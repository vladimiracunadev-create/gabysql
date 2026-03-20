<?php
session_start();

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

function http_get_json(string $url, string $apiToken): array {
  $headers = "Accept: application/json\r\n";
  if ($apiToken !== '') {
    $headers .= "X-Gabysql-Token: " . $apiToken . "\r\n";
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

function http_post_json(string $url, array $payload, string $apiToken): array {
  $headers = "Content-Type: application/json\r\nAccept: application/json\r\n";
  if ($apiToken !== '') {
    $headers .= "X-Gabysql-Token: " . $apiToken . "\r\n";
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

$uiToken = getenv('GABYADMIN_TOKEN');
$cookieName = 'gabyadmin_auth';
if ($uiToken) {
  if (isset($_POST['logout'])) {
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
      <title>phpgabyadmin - login</title>
      <style>
        body{font-family:Georgia,"Times New Roman",serif;margin:0;background:#0d1117;color:#edf3ff}
        .wrap{max-width:520px;margin:0 auto;padding:36px 20px}
        .card{background:#141b24;border:1px solid #243244;border-radius:18px;padding:22px;box-shadow:0 18px 48px rgba(0,0,0,.28)}
        input{width:100%;padding:12px;border-radius:12px;border:1px solid #2d425d;background:#0f151d;color:#edf3ff}
        button{margin-top:12px;background:#2f8fdd;border:0;color:#08111b;padding:10px 14px;border-radius:12px;font-weight:700;cursor:pointer}
        .error{margin-top:12px;background:#31131c;border:1px solid #6d3044;color:#ffd7df;padding:10px;border-radius:12px}
        .muted{font-size:13px;color:#aebddb}
      </style>
    </head>
    <body>
      <div class="wrap">
        <div class="card">
          <h1 style="margin:0 0 10px;font-size:22px">phpgabyadmin</h1>
          <div class="muted">Protegido por <code>GABYADMIN_TOKEN</code></div>
          <form method="post">
            <input name="ui_token" type="password" placeholder="Token" autocomplete="current-password"/>
            <button type="submit">Entrar</button>
          </form>
          <?php if ($err): ?><div class="error"><?= htmlspecialchars($err) ?></div><?php endif; ?>
        </div>
      </div>
    </body>
    </html><?php
    exit;
  }
}

[$server, $serverErr] = normalize_server((string)($_GET['server'] ?? ''));
$apiToken = trim((string)($_GET['token'] ?? ''));
$db = trim((string)($_GET['db'] ?? ''));
$table = trim((string)($_GET['table'] ?? ''));
$tab = $_GET['tab'] ?? 'browse';
$sql = $_POST['sql'] ?? "SELECT * FROM users LIMIT 10;";

[$dbsResp, $dbsErr] = http_get_json($server . '/dbs', $apiToken);
$dbs = [];
if (!$dbsErr && ($dbsResp['ok'] ?? false)) {
  $dbs = $dbsResp['dbs'] ?? [];
  if ($db === '' && count($dbs) > 0) {
    $db = (string)$dbs[0];
  }
}

$tables = [];
$tablesErr = null;
if ($db !== '') {
  [$tablesResp, $tablesErr] = http_get_json($server . '/tables?db=' . urlencode($db), $apiToken);
  if (!$tablesErr && ($tablesResp['ok'] ?? false)) {
    $tables = $tablesResp['tables'] ?? [];
  }
}

$createDbMsg = null;
if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['new_db'])) {
  [$createResp, $createErr] = http_post_json($server . '/dbs', ['db' => trim((string)$_POST['new_db'])], $apiToken);
  if ($createErr) {
    $createDbMsg = $createErr;
  } elseif (!($createResp['ok'] ?? false)) {
    $createDbMsg = $createResp['error'] ?? 'Error';
  } else {
    $createDbMsg = 'DB creada: ' . ($createResp['db'] ?? '');
  }
}

$importMsg = null;
$execJson = null;
$execErr = null;
if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['import_csv']) && $db !== '' && $table !== '' && isset($_FILES['csv'])) {
  $tmp = $_FILES['csv']['tmp_name'] ?? '';
  if (!$tmp || !is_uploaded_file($tmp)) {
    $importMsg = 'No se recibió archivo';
  } else {
    $rows = array_map('str_getcsv', file($tmp));
    if (count($rows) < 2) {
      $importMsg = 'CSV debe tener header + filas';
    } else {
      $header = $rows[0];
      $stmts = [];
      for ($i = 1; $i < count($rows); $i++) {
        $record = $rows[$i];
        $vals = [];
        for ($c = 0; $c < count($header); $c++) {
          $raw = trim((string)($record[$c] ?? ''));
          if ($raw === '' || strtoupper($raw) === 'NULL') {
            $vals[] = 'NULL';
          } elseif (strtoupper($raw) === 'TRUE' || strtoupper($raw) === 'FALSE') {
            $vals[] = strtoupper($raw);
          } elseif (is_numeric($raw)) {
            $vals[] = $raw;
          } else {
            $vals[] = "'" . str_replace("'", "''", $raw) . "'";
          }
        }
        $stmts[] = 'INSERT INTO ' . $table . ' (' . implode(',', $header) . ') VALUES (' . implode(',', $vals) . ')';
      }
      [$execJson, $execErr] = http_post_json($server . '/exec', ['db' => $db, 'sql' => implode(";\n", $stmts) . ';'], $apiToken);
      if ($execErr) {
        $importMsg = $execErr;
      } elseif (!($execJson['ok'] ?? false)) {
        $importMsg = $execJson['error'] ?? 'Error';
      } else {
        $importMsg = 'Import OK: ' . count($stmts) . ' filas';
      }
    }
  }
}

if (isset($_GET['export']) && $db !== '' && $table !== '') {
  [$rowsResp, $rowsErr] = http_get_json($server . '/rows?db=' . urlencode($db) . '&table=' . urlencode($table) . '&limit=1000&offset=0', $apiToken);
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
  foreach ($rows as $row) {
    fputcsv($out, $row);
  }
  fclose($out);
  exit;
}

if ($_SERVER['REQUEST_METHOD'] === 'POST' && isset($_POST['run_sql'])) {
  [$execJson, $execErr] = http_post_json($server . '/exec', ['db' => $db, 'sql' => (string)$sql], $apiToken);
  if (!$execErr && !($execJson['ok'] ?? false)) {
    $execErr = $execJson['error'] ?? 'Error';
  }
}
?><!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>phpgabyadmin - gabysql</title>
  <style>
    body{font-family:Georgia,"Times New Roman",serif;margin:0;background:radial-gradient(circle at top,#172131 0%,#0d1117 48%,#090c12 100%);color:#edf3ff}
    .wrap{max-width:1220px;margin:0 auto;padding:24px 18px 40px}
    .top{display:flex;gap:12px;flex-wrap:wrap;align-items:flex-start;justify-content:space-between;margin-bottom:16px}
    .card{background:rgba(20,27,36,.94);border:1px solid #243244;border-radius:18px;padding:16px;box-shadow:0 18px 48px rgba(0,0,0,.28)}
    .grid{display:grid;grid-template-columns:1fr;gap:16px}
    @media(min-width:1020px){.grid{grid-template-columns:360px 1fr}}
    h1,h2{margin:0 0 10px}
    h1{font-size:22px}
    h2{font-size:17px}
    .muted{font-size:13px;color:#aebddb}
    .error{background:#31131c;border:1px solid #6d3044;color:#ffd7df;padding:10px;border-radius:12px}
    .ok{background:#10231a;border:1px solid #2f6e4b;color:#d7ffe8;padding:10px;border-radius:12px}
    input[type=text],textarea,select{width:100%;padding:11px;border-radius:12px;border:1px solid #2d425d;background:#0f151d;color:#edf3ff}
    textarea{min-height:150px;font-family:"Cascadia Code","Consolas",monospace}
    button{background:#2f8fdd;border:0;color:#08111b;padding:10px 14px;border-radius:12px;font-weight:700;cursor:pointer}
    table{width:100%;border-collapse:collapse;margin-top:12px}
    th,td{padding:8px 10px;border-bottom:1px solid #243244;text-align:left;color:#d7e6ff}
    th{color:#edf3ff}
    a{color:#9cd1ff}
    .pill{display:inline-block;padding:6px 10px;border-radius:999px;background:#102033;border:1px solid #35506f;color:#d7e6ff;margin:6px 8px 0 0;text-decoration:none}
    .tabs a{display:inline-block;padding:8px 10px;border:1px solid #35506f;border-radius:10px;text-decoration:none;margin-right:8px}
    .tabs a.active{background:#0f151d}
    code,pre{font-family:"Cascadia Code","Consolas",monospace}
    pre{background:#0f151d;border:1px solid #243244;border-radius:14px;padding:12px;overflow:auto}
  </style>
</head>
<body>
  <div class="wrap">
    <div class="top">
      <div>
        <h1>phpgabyadmin</h1>
        <div class="muted">Admin ligero para <b>gabysql</b> sobre el API HTTP en Rust.</div>
      </div>
      <div class="card" style="padding:12px 14px;min-width:320px">
        <form method="get" style="display:grid;gap:10px">
          <div>
            <div class="muted">Servidor</div>
            <input type="text" name="server" value="<?= htmlspecialchars($server) ?>"/>
          </div>
          <div>
            <div class="muted">API token (opcional)</div>
            <input type="text" name="token" value="<?= htmlspecialchars($apiToken) ?>"/>
          </div>
          <button type="submit">Aplicar</button>
        </form>
        <div class="muted" style="margin-top:8px">
          <a href="/index.php">Volver a la portada</a>
          <?php if ($uiToken): ?>
            · <form method="post" style="display:inline"><button name="logout" style="background:transparent;border:0;color:#9cd1ff;padding:0">Salir</button></form>
          <?php endif; ?>
        </div>
      </div>
    </div>

    <?php if ($serverErr): ?><div class="error" style="margin-bottom:14px"><?= htmlspecialchars($serverErr) ?></div><?php endif; ?>

    <div class="grid">
      <div class="card">
        <h2>DBs</h2>
        <?php if ($dbsErr): ?>
          <div class="error"><?= htmlspecialchars($dbsErr) ?></div>
          <div class="muted" style="margin-top:10px">Levanta el server:</div>
          <pre><code>cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
# o multi-db:
cargo run --release --bin gabysql-server -- -dir .\dbs -addr :8080</code></pre>
        <?php else: ?>
          <form method="get" style="margin-bottom:12px">
            <input type="hidden" name="server" value="<?= htmlspecialchars($server) ?>"/>
            <input type="hidden" name="token" value="<?= htmlspecialchars($apiToken) ?>"/>
            <div class="muted">Selecciona DB</div>
            <select name="db">
              <?php foreach ($dbs as $dbName): ?>
                <option value="<?= htmlspecialchars((string)$dbName) ?>" <?= (string)$dbName === $db ? 'selected' : '' ?>><?= htmlspecialchars((string)$dbName) ?></option>
              <?php endforeach; ?>
            </select>
            <button type="submit" style="margin-top:10px;width:100%">Abrir</button>
          </form>

          <?php if (($dbsResp['mode'] ?? '') === 'multi-db'): ?>
            <hr style="border:0;border-top:1px solid #243244;margin:14px 0"/>
            <div class="muted">Crear nueva DB</div>
            <form method="post" style="display:flex;gap:10px;margin-top:8px">
              <input type="text" name="new_db" placeholder="nueva.db" style="flex:1"/>
              <button type="submit">Crear</button>
            </form>
            <?php if ($createDbMsg): ?><div class="muted" style="margin-top:8px"><?= htmlspecialchars($createDbMsg) ?></div><?php endif; ?>
          <?php endif; ?>
        <?php endif; ?>

        <hr style="border:0;border-top:1px solid #243244;margin:14px 0"/>

        <h2>Tablas</h2>
        <?php if ($tablesErr): ?>
          <div class="error"><?= htmlspecialchars($tablesErr) ?></div>
        <?php elseif (count($tables) === 0): ?>
          <div class="muted">No hay tablas.</div>
        <?php else: ?>
          <?php foreach ($tables as $tableInfo): $tableName = (string)($tableInfo['name'] ?? 'tabla'); ?>
            <a class="pill" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($tableName) ?>&tab=browse">
              <b><?= htmlspecialchars($tableName) ?></b> · PK: <?= htmlspecialchars((string)($tableInfo['primaryKey'] ?? '-')) ?>
            </a>
          <?php endforeach; ?>
        <?php endif; ?>
      </div>

      <div class="card">
        <div style="display:flex;justify-content:space-between;gap:10px;flex-wrap:wrap">
          <div>
            <h2>Workspace</h2>
            <div class="muted">DB: <code><?= htmlspecialchars($db ?: '-') ?></code> · Tabla: <code><?= htmlspecialchars($table ?: '-') ?></code></div>
          </div>
          <?php if ($db !== '' && $table !== ''): ?>
            <div class="tabs">
              <a class="<?= $tab === 'browse' ? 'active' : '' ?>" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($table) ?>&tab=browse">Browse</a>
              <a class="<?= $tab === 'structure' ? 'active' : '' ?>" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($table) ?>&tab=structure">Structure</a>
              <a class="<?= $tab === 'sql' ? 'active' : '' ?>" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($table) ?>&tab=sql">SQL</a>
            </div>
          <?php endif; ?>
        </div>

        <?php if ($db === ''): ?>
          <div class="muted" style="margin-top:12px">Selecciona una DB.</div>
        <?php elseif ($table === ''): ?>
          <div class="muted" style="margin-top:12px">Selecciona una tabla.</div>
        <?php elseif ($tab === 'structure'): ?>
          <?php [$schemaResp, $schemaErr] = http_get_json($server . '/schema?db=' . urlencode($db) . '&table=' . urlencode($table), $apiToken); ?>
          <?php if ($schemaErr || !($schemaResp['ok'] ?? false)): ?>
            <div class="error" style="margin-top:12px"><?= htmlspecialchars($schemaErr ?: ($schemaResp['error'] ?? 'Error')) ?></div>
          <?php else: $meta = $schemaResp['table'] ?? []; ?>
            <table>
              <thead><tr><th>Columna</th><th>Tipo</th><th>PK</th></tr></thead>
              <tbody>
                <?php foreach (($meta['columns'] ?? []) as $column): ?>
                  <tr>
                    <td><?= htmlspecialchars((string)($column['name'] ?? '')) ?></td>
                    <td><?= htmlspecialchars((string)($column['type'] ?? '')) ?></td>
                    <td><?= !empty($column['pk']) ? 'si' : '' ?></td>
                  </tr>
                <?php endforeach; ?>
              </tbody>
            </table>
          <?php endif; ?>

        <?php elseif ($tab === 'browse'): ?>
          <?php
            $limit = max(1, min(200, intval($_GET['limit'] ?? 25)));
            $offset = max(0, intval($_GET['offset'] ?? 0));
            [$rowsResp, $rowsErr] = http_get_json($server . '/rows?db=' . urlencode($db) . '&table=' . urlencode($table) . '&limit=' . $limit . '&offset=' . $offset, $apiToken);
          ?>
          <?php if ($rowsErr || !($rowsResp['ok'] ?? false)): ?>
            <div class="error" style="margin-top:12px"><?= htmlspecialchars($rowsErr ?: ($rowsResp['error'] ?? 'Error')) ?></div>
          <?php else: $cols = $rowsResp['columns'] ?? []; $rows = $rowsResp['rows'] ?? []; $total = intval($rowsResp['total'] ?? 0); ?>
            <div class="muted" style="margin-top:10px">
              Total: <b><?= $total ?></b> · limit: <?= $limit ?> · offset: <?= $offset ?> ·
              <a href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($table) ?>&export=1">Export CSV</a>
            </div>

            <table>
              <thead><tr><?php foreach ($cols as $col): ?><th><?= htmlspecialchars((string)$col) ?></th><?php endforeach; ?></tr></thead>
              <tbody>
                <?php foreach ($rows as $row): ?>
                  <tr><?php foreach ($row as $cell): ?><td><?= htmlspecialchars((string)$cell) ?></td><?php endforeach; ?></tr>
                <?php endforeach; ?>
              </tbody>
            </table>

            <div style="display:flex;gap:10px;margin-top:12px;flex-wrap:wrap">
              <?php $prev = max(0, $offset - $limit); $next = $offset + $limit; ?>
              <a class="pill" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($table) ?>&tab=browse&limit=<?= $limit ?>&offset=<?= $prev ?>">← Prev</a>
              <a class="pill" href="?server=<?= urlencode($server) ?>&token=<?= urlencode($apiToken) ?>&db=<?= urlencode($db) ?>&table=<?= urlencode($table) ?>&tab=browse&limit=<?= $limit ?>&offset=<?= $next ?>">Next →</a>
            </div>

            <hr style="border:0;border-top:1px solid #243244;margin:14px 0"/>
            <div class="muted">Import CSV (header = columnas)</div>
            <form method="post" enctype="multipart/form-data" style="display:flex;gap:10px;align-items:center;margin-top:8px;flex-wrap:wrap">
              <input type="file" name="csv" accept=".csv" style="color:#d7e6ff"/>
              <button name="import_csv" type="submit">Import</button>
            </form>
            <?php if ($importMsg): ?><div class="muted" style="margin-top:8px"><?= htmlspecialchars($importMsg) ?></div><?php endif; ?>
          <?php endif; ?>

        <?php else: ?>
          <form method="post" style="margin-top:12px">
            <textarea name="sql"><?= htmlspecialchars((string)$sql) ?></textarea>
            <div style="display:flex;gap:10px;align-items:center;margin-top:10px;flex-wrap:wrap">
              <button name="run_sql" type="submit">Ejecutar</button>
              <span class="muted">Cada ejecución pasa por Begin → Exec → Commit</span>
            </div>
          </form>

          <div style="margin-top:14px">
            <?php if ($execErr): ?>
              <div class="error"><b>Error:</b> <?= htmlspecialchars($execErr) ?></div>
            <?php elseif ($execJson && ($execJson['ok'] ?? false)): ?>
              <div class="ok"><b>OK</b></div>
              <?php foreach (($execJson['results'] ?? []) as $index => $result):
                $cols = $result['columns'] ?? [];
                $rows = $result['rows'] ?? [];
                $message = $result['message'] ?? '';
              ?>
                <div style="margin-top:12px">
                  <?php if ($message): ?><div class="muted"><b>Mensaje:</b> <?= htmlspecialchars((string)$message) ?></div><?php endif; ?>
                  <?php if (is_array($cols) && count($cols) > 0): ?>
                    <table>
                      <thead><tr><?php foreach ($cols as $col): ?><th><?= htmlspecialchars((string)$col) ?></th><?php endforeach; ?></tr></thead>
                      <tbody>
                        <?php foreach ($rows as $row): ?>
                          <tr><?php foreach ($row as $cell): ?><td><?= htmlspecialchars((string)$cell) ?></td><?php endforeach; ?></tr>
                        <?php endforeach; ?>
                      </tbody>
                    </table>
                  <?php else: ?>
                    <div class="muted">Resultado #<?= $index + 1 ?>: operación sin tabla de salida.</div>
                  <?php endif; ?>
                </div>
              <?php endforeach; ?>

              <details style="margin-top:12px">
                <summary class="muted">Ver JSON crudo</summary>
                <pre><code><?= htmlspecialchars(json_encode($execJson, JSON_PRETTY_PRINT | JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES)) ?></code></pre>
              </details>
            <?php endif; ?>
          </div>
        <?php endif; ?>
      </div>
    </div>
  </div>
</body>
</html>
