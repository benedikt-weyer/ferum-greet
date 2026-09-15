//! Shutdown/restart actions, triggered from the greeter before login.
//!
//! Shells out to `systemctl` rather than talking to logind directly: the
//! greeter runs as an unprivileged `greeter` user with no seatd/logind
//! integration of its own (see the README), but `systemctl poweroff`/
//! `reboot` go through the same logind + polkit path every other greeter
//! uses, and polkit's default rules allow the active local session to power
//! off/reboot without authentication.

use std::process::Command;

use anyhow::{Context, Result};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Shutdown,
    Reboot,
}

impl PowerAction {
    pub fn confirm_label(self) -> &'static str {
        match self {
            PowerAction::Shutdown => "Shut down now? Click the icon again or press Enter to confirm, Esc to cancel",
            PowerAction::Reboot => "Restart now? Click the icon again or press Enter to confirm, Esc to cancel",
        }
    }

    fn systemctl_verb(self) -> &'static str {
        match self {
            PowerAction::Shutdown => "poweroff",
            PowerAction::Reboot => "reboot",
        }
    }

    pub fn execute(self) -> Result<()> {
        let verb = self.systemctl_verb();
        let status = Command::new("systemctl")
            .arg(verb)
            .status()
            .with_context(|| format!("running systemctl {verb}"))?;
        if !status.success() {
            anyhow::bail!("systemctl {verb} exited with {status}");
        }
        Ok(())
    }
}
