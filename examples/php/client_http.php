<?php
$base = "http://localhost:8080";
$db = "demo.db";

$payload = json_encode(["db"=>$db, "sql"=>"SELECT * FROM users LIMIT 10;"]);
$ch = curl_init($base."/exec");
curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
curl_setopt($ch, CURLOPT_POST, true);
curl_setopt($ch, CURLOPT_HTTPHEADER, ["Content-Type: application/json"]);
curl_setopt($ch, CURLOPT_POSTFIELDS, $payload);

$response = curl_exec($ch);
if ($response === false) { echo "curl error: ".curl_error($ch).PHP_EOL; exit(1); }
curl_close($ch);

echo $response.PHP_EOL;
