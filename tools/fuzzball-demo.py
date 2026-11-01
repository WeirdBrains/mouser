#!/usr/bin/env python3
"""Fuzzball demo client: negotiate, upload and place an image, then echo
Fuzzball key events until Ctrl+C or q. Run inside Mouser (or any Fuzzball
terminal). Usage: tools/fuzzball-demo.py [--no-keys]
Set FUZZBALL_DEMO_LOG=<file> to also append each key event to a file."""
import base64, os, re, select, sys, termios, tty

ESC = "\x1b"
def dcs(body): return f"{ESC}P=Fz{body}{ESC}\\"
def out(s): os.write(1, s.encode())

def read_reply(fd, timeout=1.0):
    buf = b""
    while True:
        r, _, _ = select.select([fd], [], [], timeout)
        if not r: return buf.decode(errors="replace")
        buf += os.read(fd, 4096)
        if buf.endswith(b"\x1b\\") or buf.endswith(b"c"): return buf.decode(errors="replace")

def gradient(w, h):
    px = bytearray()
    for y in range(h):
        for x in range(w):
            px += bytes((255 * x // (w - 1), 255 * y // (h - 1), 255 - 255 * x // (w - 1), 255))
    return bytes(px)

LOG = open(os.environ["FUZZBALL_DEMO_LOG"], "a") if os.environ.get("FUZZBALL_DEMO_LOG") else None

def main():
    fd = sys.stdin.fileno()
    old = termios.tcgetattr(fd)
    tty.setraw(fd)
    try:
        out(dcs("cq;id=1") + f"{ESC}[c")
        rep = read_reply(fd)
        m = re.search(r"Fzcr;id=1,([^\x1b]*)", rep)
        if not m:
            out("No Fuzzball support (no cq reply).\r\n"); return
        caps = m.group(1)
        out(f"\x1b[2J\x1b[H\x1b[1mFuzzball demo\x1b[0m  caps: {caps}\r\n\r\n")
        w, h = 64, 32
        data = base64.b64encode(gradient(w, h)).decode()
        chunk = 4096
        parts = [data[i:i + chunk] for i in range(0, len(data), chunk)]
        for i, p in enumerate(parts):
            more = 1 if i < len(parts) - 1 else 0
            head = f"id=7,fmt=rgba,w={w},h={h},size={w*h*4},len={len(p)},more={more}" if i == 0 else f"id=7,len={len(p)},more={more}"
            out(dcs(f"gu;{head}:{p}"))
        rep = read_reply(fd)
        out(f"upload: {rep.replace(ESC, 'ESC')}\r\n")
        out(dcs("gp;id=7,pid=1,cols=24,rows=6,quiet=1") + "\r\n")
        out(dcs("gq;id=7"))
        out(f"query:  {read_reply(fd).replace(ESC, 'ESC')}\r\n\r\n")
        if "--no-keys" in sys.argv: return
        out(dcs("ks;id=2,on=1"))
        read_reply(fd, 0.3)
        out("Keyboard mode on. Press keys (q quits):\r\n")
        while True:
            ev = read_reply(fd, 30.0)
            if not ev: break
            for e in re.findall(r"Fzke;([^\x1b]*)", ev):
                out(f"  key {e}\r\n")
                if LOG: LOG.write(e + "\n"); LOG.flush()
                if "key=U+0071" in e and "t=p" in e and "mods=0" in e: return
    finally:
        out(dcs("ks;on=0"))
        termios.tcsetattr(fd, termios.TCSADRAIN, old)

if __name__ == "__main__":
    main()
