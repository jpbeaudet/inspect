//! Safety gate: enforces dry-run-by-default + interactive confirmation.
//!
//! Bible §8.2:
//! - Without `--apply`, every write verb shows what would happen and exits 0.
//! - `rm`, `chmod`, `chown` prompt interactively when applying (skip with `--yes`).
//! - Selectors expanding to >10 targets prompt even with `--apply`
//!   (skip with `--yes-all`).

use std::io::{self, BufRead, IsTerminal, Write};

/// Confirmation policy decided per verb and per invocation.
#[derive(Debug, Clone, Copy)]
pub enum Confirm {
    /// Prompt unconditionally on `--apply` unless `--yes` is passed.
    Always,
    /// Only prompt on the large-fanout interlock.
    LargeFanout,
}

#[derive(Debug, Clone)]
pub struct SafetyGate {
    pub apply: bool,
    pub yes: bool,
    pub yes_all: bool,
    pub fanout_threshold: usize,
    /// Disable interactive prompts entirely — non-tty / CI / test envs.
    pub non_interactive: bool,
}

impl SafetyGate {
    pub fn new(apply: bool, yes: bool, yes_all: bool) -> Self {
        let non_interactive = std::env::var("INSPECT_NON_INTERACTIVE").is_ok()
            || !io::stdin().is_terminal();
        Self {
            apply,
            yes,
            yes_all,
            fanout_threshold: 10,
            non_interactive,
        }
    }

    /// Returns true iff the verb should actually mutate state. False means
    /// "render preview / dry-run, exit zero".
    pub fn should_apply(&self) -> bool {
        self.apply
    }

    /// Decide whether to proceed given `policy` and `target_count`. May
    /// emit interactive prompts; honors `--yes` / `--yes-all`.
    pub fn confirm(
        &self,
        policy: Confirm,
        target_count: usize,
        prompt: &str,
    ) -> ConfirmResult {
        if !self.apply {
            return ConfirmResult::DryRun;
        }
        // Large fanout interlock fires on every verb when target count > N.
        if target_count > self.fanout_threshold && !self.yes_all {
            if self.non_interactive {
                return ConfirmResult::Aborted(format!(
                    "selector matched {target_count} targets (>{thresh}); pass --yes-all to override",
                    thresh = self.fanout_threshold
                ));
            }
            if !ask(&format!(
                "About to apply to {target_count} targets (>{thresh}). Continue?",
                thresh = self.fanout_threshold
            )) {
                return ConfirmResult::Aborted("declined large-fanout prompt".into());
            }
        }
        match policy {
            Confirm::LargeFanout => ConfirmResult::Apply,
            Confirm::Always => {
                if self.yes || self.yes_all {
                    return ConfirmResult::Apply;
                }
                if self.non_interactive {
                    return ConfirmResult::Aborted(format!(
                        "{prompt} requires interactive confirmation; pass --yes to override"
                    ));
                }
                if ask(prompt) {
                    ConfirmResult::Apply
                } else {
                    ConfirmResult::Aborted("declined confirmation prompt".into())
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum ConfirmResult {
    DryRun,
    Apply,
    Aborted(String),
}

fn ask(prompt: &str) -> bool {
    let mut out = io::stderr();
    let _ = write!(out, "{prompt} [y/N] ");
    let _ = out.flush();
    let mut line = String::new();
    let stdin = io::stdin();
    if stdin.lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_default() {
        let g = SafetyGate::new(false, false, false);
        assert!(!g.should_apply());
        assert!(matches!(g.confirm(Confirm::Always, 1, "ok?"), ConfirmResult::DryRun));
    }

    #[test]
    fn large_fanout_blocks_without_yes_all() {
        let mut g = SafetyGate::new(true, false, false);
        g.non_interactive = true; // simulate CI
        let r = g.confirm(Confirm::LargeFanout, 25, "ok?");
        match r {
            ConfirmResult::Aborted(_) => {}
            other => panic!("expected Aborted, got {other:?}"),
        }
    }

    #[test]
    fn yes_all_skips_fanout_and_always() {
        let mut g = SafetyGate::new(true, false, true);
        g.non_interactive = true;
        assert!(matches!(g.confirm(Confirm::LargeFanout, 25, "ok?"), ConfirmResult::Apply));
        assert!(matches!(g.confirm(Confirm::Always, 1, "ok?"), ConfirmResult::Apply));
    }

    #[test]
    fn yes_skips_always_but_not_fanout() {
        let mut g = SafetyGate::new(true, true, false);
        g.non_interactive = true;
        assert!(matches!(g.confirm(Confirm::Always, 1, "ok?"), ConfirmResult::Apply));
        match g.confirm(Confirm::LargeFanout, 25, "ok?") {
            ConfirmResult::Aborted(_) => {}
            other => panic!("expected Aborted, got {other:?}"),
        }
    }
}
