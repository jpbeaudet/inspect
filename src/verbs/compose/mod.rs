//! F6 (v0.1.3): first-class verbs over Docker Compose projects.
//!
//! Replaces the v0.1.2 pattern where the operator dropped back to
//! `inspect run <ns> -- 'cd <project_dir> && sudo docker compose …'`
//! to inspect compose state. Each sub-verb resolves the project's
//! working directory from the cached profile (populated at `inspect
//! setup` time by `discovery::probes::probe_compose_projects`) so the
//! operator never types the path.
//!
//! Sub-verbs:
//!
//! Read (no audit, no apply gate):
//!
//! - [`ls`] — list compose projects on the namespace.
//! - [`ps`] — per-service status table for one project.
//! - [`config`] — effective merged compose config (redacted).
//! - [`logs`] — aggregated logs for a project, or one service inside it.
//!
//! Write (audited; require `--apply`):
//!
//! - [`up`] — bring up a project. `verb=compose.up`.
//! - [`down`] — tear down a project. `verb=compose.down`. `--volumes` is destructive.
//! - [`pull`] — pull images. `verb=compose.pull`. Streams progress.
//! - [`build`] — build images. `verb=compose.build`. Streams progress.
//! - [`restart`] — restart a single service. `verb=compose.restart`.
//!
//! Inspect-run-style (no audit, no apply gate, redacted output):
//!
//! - [`exec`] — run a command inside a compose service container.
//!
//! Selector grammar:
//! - `<ns>` for `compose ls`.
//! - `<ns>/<project>` for `compose ps`, `compose config`, aggregated
//!   `compose logs`, `compose restart --all`, `compose up`,
//!   `compose down`, project-wide `compose pull` / `compose build`.
//! - `<ns>/<project>/<service>` for narrowed `compose logs`,
//!   per-service `compose pull` / `compose build`, the safe
//!   `compose restart`, and `compose exec`.

use anyhow::Result;

use crate::cli::{ComposeArgs, ComposeCommand};
use crate::error::ExitKind;

mod build;
mod config;
mod down;
mod exec;
mod logs;
mod ls;
mod ps;
mod pull;
mod resolve;
mod restart;
mod up;
// L8 (v0.1.3): exposed pub(crate) so the bundle's compose: step
// can reuse the command builders + audit-arg formatter.
pub(crate) mod write_common;

pub fn dispatch(args: ComposeArgs) -> Result<ExitKind> {
    match args.command {
        ComposeCommand::Ls(a) => ls::run(a),
        ComposeCommand::Ps(a) => ps::run(a),
        ComposeCommand::Config(a) => config::run(a),
        ComposeCommand::Logs(a) => logs::run(a),
        ComposeCommand::Restart(a) => restart::run(a),
        ComposeCommand::Up(a) => up::run(a),
        ComposeCommand::Down(a) => down::run(a),
        ComposeCommand::Pull(a) => pull::run(a),
        ComposeCommand::Build(a) => build::run(a),
        ComposeCommand::Exec(a) => exec::run(a),
    }
}
