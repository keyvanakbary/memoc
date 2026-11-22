"""Hash native Cargo build inputs that survive source edits and Memoc releases."""
import hashlib
import json
from pathlib import Path
import tomllib

manifest = tomllib.loads(Path("Cargo.toml").read_text())
lock = tomllib.loads(Path("Cargo.lock").read_text())
package = manifest["package"]
del package["version"]
for entry in lock["package"]:
    if entry["name"] == package["name"] and "source" not in entry:
        del entry["version"]
inputs = {
    "manifest": manifest,
    "lock": lock,
    "build": Path("scripts/ci-build.sh").read_text(),
    "config": {str(p): p.read_text() for p in sorted(Path(".cargo").glob("config*"))},
}
print(hashlib.sha256(json.dumps(inputs, sort_keys=True).encode()).hexdigest())
