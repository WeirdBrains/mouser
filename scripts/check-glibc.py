#!/usr/bin/env python3
"""The newest glibc symbol version any ELF file under the given paths needs.

    check-glibc.py [--max 2.35] PATH...

Archives (.tar.gz) are unpacked and their files checked. Prints `GLIBC_<x.y>` (the floor every Linux artifact must run on) and the
file that needs it, and exits 1 when that is newer than --max: a release
built on a too-new runner (v1.3.0 needed GLIBC_2.43, issue #2) fails here
instead of on users' machines. Needs `objdump` (binutils). Python 3.8+.
"""
import os, re, subprocess, sys, tarfile, tempfile

def ver(s):
    return tuple(int(x) for x in s.split("."))

def elves(paths, tmp, shown=None):
    for p in paths:
        files = [p] if os.path.isfile(p) else [os.path.join(d, f) for d, _, fs in os.walk(p) for f in fs]
        for f in files:
            # Plugin and engine archives (the -full bundle's plugins/) are
            # unpacked and checked too.
            if f.endswith((".tar.gz", ".tgz")):
                d = tempfile.mkdtemp(dir=tmp)
                with tarfile.open(f) as t:
                    ok = [m for m in t.getmembers() if m.isfile() and not os.path.isabs(m.name) and ".." not in m.name.split("/")]
                    if hasattr(tarfile, "data_filter"):
                        t.extractall(d, members=ok, filter="data")
                    else:
                        t.extractall(d, members=ok)
                for g, _ in elves([d], tmp):
                    yield g, f + "!" + os.path.relpath(g, d)
                continue
            if os.path.islink(f):
                continue
            try:
                with open(f, "rb") as h:
                    if h.read(4) == b"\x7fELF":
                        yield f, f
            except OSError:
                pass

def main(argv):
    limit = None
    if argv[:1] == ["--max"]:
        limit, argv = ver(argv[1]), argv[2:]
    if not argv:
        sys.exit(__doc__)
    worst, who, n = (0, 0), None, 0
    tmp = tempfile.mkdtemp()
    for f, name in elves(argv, tmp):
        n += 1
        out = subprocess.run(["objdump", "-T", f], capture_output=True, text=True).stdout
        for v in set(re.findall(r"GLIBC_([0-9]+(?:\.[0-9]+)+)", out)):
            if ver(v) > worst:
                worst, who = ver(v), name
    if n == 0:
        sys.exit("check-glibc: no ELF files under " + " ".join(argv))
    w = ".".join(map(str, worst))
    print(f"GLIBC_{w} ({who}; {n} ELF files)")
    if limit and worst > limit:
        sys.exit(f"check-glibc: {who} needs GLIBC_{w}, newer than the {'.'.join(map(str, limit))} baseline (Ubuntu 22.04); build on ubuntu-22.04")

if __name__ == "__main__":
    main(sys.argv[1:])
