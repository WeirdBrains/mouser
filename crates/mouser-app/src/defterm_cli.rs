//! `mouser default-terminal status|register|unregister|set|unset`
//! (docs/windows.md §Default terminal). The registry is passed in so this is
//! tested on every OS with [`MemRegistry`](mouser_pty::defterm::MemRegistry).

use anyhow::Result;
use mouser_pty::defterm::{self, RegisterOutcome, Registry, WindowsTerminal};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Status,
    Register,
    Unregister,
    Set,
    Unset,
}

pub fn run(a: Action, reg: &mut dyn Registry, wt: Option<WindowsTerminal>, exe: &Path, out: &mut dyn Write) -> Result<()> {
    match a {
        Action::Status => {
            let s = defterm::status(reg, wt);
            let wt_name = match s.windows_terminal {
                Some(WindowsTerminal::Stable) => "installed",
                Some(WindowsTerminal::Preview) => "installed (Preview)",
                None => "not installed (needed for handoff)",
            };
            writeln!(out, "Windows Terminal: {wt_name}")?;
            writeln!(out, "Mouser handoff:   {}", if s.registered { "registered" } else { "not registered" })?;
            writeln!(out, "Default terminal: {}", defterm::describe(s.terminal.as_deref()))?;
            if !s.is_default && s.registered {
                writeln!(out, "Run `mouser default-terminal set` to make Mouser the default.")?;
            }
        }
        Action::Register => match defterm::register(reg, exe, wt)? {
            RegisterOutcome::Registered => writeln!(out, "Registered Mouser's terminal handoff (the default is unchanged).")?,
            RegisterOutcome::SkippedNoWindowsTerminal => writeln!(out, "Windows Terminal is not installed; nothing registered.")?,
        },
        Action::Unregister => {
            defterm::unregister(reg)?;
            writeln!(out, "Removed Mouser's terminal handoff.")?;
        }
        Action::Set => {
            if !defterm::is_registered(reg) && defterm::register(reg, exe, wt)? == RegisterOutcome::SkippedNoWindowsTerminal {
                anyhow::bail!("Windows Terminal is not installed; Mouser needs its console host (OpenConsole) for handoff");
            }
            let before = defterm::describe(reg.get(defterm::STARTUP_KEY, defterm::DELEGATION_TERMINAL).as_deref());
            defterm::set_default(reg, wt)?;
            writeln!(out, "Mouser is now the default terminal (was: {before}). `mouser default-terminal unset` puts it back.")?;
        }
        Action::Unset => {
            if defterm::restore_default(reg)? {
                let now = defterm::describe(reg.get(defterm::STARTUP_KEY, defterm::DELEGATION_TERMINAL).as_deref());
                writeln!(out, "Restored the previous default terminal: {now}.")?;
            } else {
                writeln!(out, "Mouser is not the default terminal; nothing changed.")?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouser_pty::defterm::{DELEGATION_CONSOLE, DELEGATION_TERMINAL, MOUSER_HANDOFF_CLSID, MemRegistry, STARTUP_KEY};

    fn go(a: Action, reg: &mut MemRegistry, wt: Option<WindowsTerminal>) -> Result<String> {
        let mut out = Vec::new();
        run(a, reg, wt, Path::new(r"C:\Program Files\Mouser\mouser.exe"), &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn set_then_unset_round_trips() {
        let wt = Some(WindowsTerminal::Stable);
        let mut reg = MemRegistry::default();
        reg.set(STARTUP_KEY, DELEGATION_CONSOLE, WindowsTerminal::Stable.console_clsid()).unwrap();
        reg.set(STARTUP_KEY, DELEGATION_TERMINAL, WindowsTerminal::Stable.terminal_clsid()).unwrap();
        let o = go(Action::Set, &mut reg, wt).unwrap();
        assert!(o.contains("was: Windows Terminal"), "{o}");
        assert_eq!(reg.get(STARTUP_KEY, DELEGATION_TERMINAL).as_deref(), Some(MOUSER_HANDOFF_CLSID));
        assert!(go(Action::Status, &mut reg, wt).unwrap().contains("Default terminal: Mouser"));
        let o = go(Action::Unset, &mut reg, wt).unwrap();
        assert!(o.contains("Restored the previous default terminal: Windows Terminal"), "{o}");
        assert_eq!(reg.get(STARTUP_KEY, DELEGATION_TERMINAL).as_deref(), Some(WindowsTerminal::Stable.terminal_clsid()));
        assert!(go(Action::Unset, &mut reg, wt).unwrap().contains("nothing changed"));
    }

    #[test]
    fn register_without_windows_terminal_is_a_no_op() {
        let mut reg = MemRegistry::default();
        assert!(go(Action::Register, &mut reg, None).unwrap().contains("nothing registered"));
        assert!(reg.values.is_empty());
        assert!(go(Action::Set, &mut reg, None).is_err());
        assert!(go(Action::Status, &mut reg, None).unwrap().contains("not installed"));
    }

    #[test]
    fn unregister_removes_everything() {
        let wt = Some(WindowsTerminal::Stable);
        let mut reg = MemRegistry::default();
        go(Action::Register, &mut reg, wt).unwrap();
        go(Action::Set, &mut reg, wt).unwrap();
        go(Action::Unregister, &mut reg, wt).unwrap();
        assert!(reg.values.is_empty(), "{:?}", reg.values);
    }
}
