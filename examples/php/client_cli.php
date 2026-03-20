<?php
$db = "demo_cli.db";
echo shell_exec("./gabysql init $db");
echo shell_exec("./gabysql exec $db \"CREATE TABLE person (id INT PRIMARY KEY, name TEXT, active BOOL, score FLOAT, born DATE);\"");
echo shell_exec("./gabysql exec $db \"INSERT INTO person (id,name,active,score,born) VALUES (1,'Ana',TRUE,9.5,'1990-01-01');\"");
echo shell_exec("./gabysql exec $db \"SELECT * FROM person LIMIT 10;\"");
