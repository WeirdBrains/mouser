# Fuzzball `p`: plugins

A program running *in* the terminal (locally, over ssh, inside tmux) asks the
terminal to accept it as a plugin. The terminal never decides on its own: it
asks the user with its own UI and answers later. Grants are per tab and end
with the tab. Capability names are listed in docs/plugins.md.

## Hello (`p h`)

```
ESC P = F z p h ; id=1,name=hello-fuzzball,kind=tool-provider,caps=notify+screen.read ESC \
```

| key | meaning |
|---|---|
| `id` | required (the answer comes back with it). Without one the message is dropped. |
| `name` | `a-z 0-9 - _`, at most 64. |
| `kind` | `tool-provider` (default), `mode`, `model-provider` or `command`. |
| `caps` | capabilities joined with `+` (`:` starts a payload in Fuzzball framing). |
| `v` | optional protocol version. |

Nothing is sent back until the user answers. Then:

```
ESC P = F z p r ; id=1,ok=1,granted=notify+screen.read ESC \
ESC P = F z p r ; id=1,ok=0,err=EPERM,msg=denied by the user ESC \
```

Bad requests are refused at once with `EINVAL` (bad name, kind or capability),
`EVERSION`, or `EPERM` when four requests are already waiting in the tab.

## Query (`p q`) and bye (`p b`)

```
ESC P = F z p q ; id=2,name=hello-fuzzball ESC \      -> ok=1,granted=notify+screen.read (empty: none)
ESC P = F z p b ; id=3,name=hello-fuzzball ESC \      -> ok=1 (drops the tab's grant)
```

## Trust

- In Mouser the approval popup is drawn by Mouser above the terminal and owns
  the keyboard and mouse while it is open, so output from the program can't
  draw, cover or answer it. Answers are ignored for the first 400 ms.
- A program on another host (an ssh/mosh session in the tab, or a remote place)
  is **untrusted**: the popup offers only *Allow once* and *Deny*, nothing is
  remembered, and a saved `allow` doesn't apply. A saved `never` still does.
- Locally, *Always* and *Never* are saved as `fuzzball:<name>`.
