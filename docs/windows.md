# Windows

## Default terminal

Windows can hand console programs (cmd.exe, PowerShell, anything started
from Explorer or Run) to a *default terminal application* instead of the old
console host. Mouser can be that terminal.

How it works: when a console program starts, OpenConsole looks at
`HKCU\Console\%%Startup` (`DelegationConsole`, `DelegationTerminal`) and calls
the terminal's `ITerminalHandoff3::EstablishPtyHandoff` COM method with the
session's pipes. Mouser registers a per-user COM local server,
CLSID `{EE272979-892E-4723-9CE6-56F6C13F92EE}`, which Windows starts as
`mouser.exe -Embedding`; Mouser then opens the handed-off session as its
first tab.

Windows Terminal must be installed: its OpenConsole is the console half of the
handoff. Mouser can't appear in the Settings → "Default terminal application"
dropdown (that list only shows packaged apps), so use the CLI:

| Command | What it does |
| --- | --- |
| `mouser default-terminal status` | Windows Terminal present? Mouser registered? Current default. |
| `mouser default-terminal register` | Register Mouser's handoff for this user. The installer runs this. The default is not changed. |
| `mouser default-terminal set` | Make Mouser the default, saving the current `DelegationConsole`/`DelegationTerminal` values. |
| `mouser default-terminal unset` | Restore the saved values. |
| `mouser default-terminal unregister` | Restore the previous default if Mouser is it, then remove the registration. The uninstaller runs this. |

Everything is under `HKCU`; no administrator rights are needed.
