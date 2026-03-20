import json
import urllib.request

BASE = "http://localhost:8080"
DB = "demo.db"  # en modo -dir, debe existir dentro de ./dbs

def post(path, payload):
    req = urllib.request.Request(
        BASE + path,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req) as resp:
        return resp.read().decode("utf-8")

print(post("/exec", {"db": DB, "sql": "SELECT * FROM users LIMIT 10;"}))
