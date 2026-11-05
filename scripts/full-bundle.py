#!/usr/bin/env python3
"""Fetch the plugins.lock archives for one target into a folder, for the -full
offline bundle (docs/downloads.md). Each file must match the lock's sha256 and
size; its .minisig is saved beside it. Usage: full-bundle.py <lock> <target> <dir>"""
import hashlib, os, sys, tomllib, urllib.request

lock, target, out = sys.argv[1:4]
os.makedirs(out, exist_ok=True)
items = tomllib.load(open(lock, "rb"))["item"]
for it in items:
    a = it.get("assets", {}).get(target)
    if a is None:
        sys.exit(f"{it['name']}: no {target} asset in {lock}")
    name = a["url"].rsplit("/", 1)[1]
    path = os.path.join(out, name)
    for attempt in range(3):
        try:
            with urllib.request.urlopen(a["url"], timeout=120) as r:
                data = r.read()
            break
        except OSError as e:
            if attempt == 2:
                raise
            print(f"{name}: {e}, retrying", file=sys.stderr)
    if len(data) != a["size"] or hashlib.sha256(data).hexdigest() != a["sha256"]:
        sys.exit(f"{name}: size or sha256 does not match {lock}")
    open(path, "wb").write(data)
    open(path + ".minisig", "w").write(a["minisig"])
    print(f"{it['kind']} {it['name']} {it['version']}: {name}")
