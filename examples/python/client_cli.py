import subprocess

DB = "demo_cli.db"

def run(*args):
    return subprocess.check_output(args, text=True)

print(run("./gabysql", "init", DB).strip())

run("./gabysql", "exec", DB, "CREATE TABLE person (id INT PRIMARY KEY, name TEXT, active BOOL, score FLOAT, born DATE);")
run("./gabysql", "exec", DB, "INSERT INTO person (id,name,active,score,born) VALUES (1,'Ana',TRUE,9.5,'1990-01-01');")
run("./gabysql", "exec", DB, "INSERT INTO person (id,name,active,score,born) VALUES (2,'Beto',FALSE,7.25,'1992-12-31');")

print(run("./gabysql", "exec", DB, "SELECT * FROM person LIMIT 10;"))
