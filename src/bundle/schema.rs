//! YAML schema for `inspect bundle` (B9, v0.1.2).
//!
//! The schema is intentionally narrow:
//!
//! * String interpolation only (`{{ vars.x }}`, `{{ matrix.k }}`) — no
//!   conditionals, loops, functions, or includes. Operators who need
//!   real logic write a shell script that calls `inspect bundle run`
//!   with different YAMLs.
//! * Five preflight/postflight check kinds: `disk_free`, `docker_running`,
//!   `services_healthy`, `http_ok`, `sql_returns`. Anything else falls
//!   back to a plain `exec:` step with an exit-code gate.
//! * Three step body kinds: `exec`, `run`, `watch`.
//! * Top-level `rollback:` is a flat list of compensating exec steps;
//!   per-step optional `rollback:` field carries that step's own
//!   compensator (executed in reverse order on failure).
//!
//! Any field not listed below is rejected by serde's
//! `deny_unknown_fields` so a typo (`max_paralllel:`) fails plan instead
//! of silently degrading.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Hard cap on `parallel: true` matrix concurrency. Eight is enough to
/// saturate most ssh ControlMaster sockets without melting them.
pub const MAX_PARALLEL_CAP: usize = 8;

/// Top-level bundle document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    /// Human-readable bundle name. Required.
    pub name: String,

    /// Default namespace (selector prefix) for steps that don't
    /// specify their own. Optional — every step can carry its own
    /// `target:`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,

    /// Free-form note propagated to every step's audit entry. Limited
    /// to 240 characters by the safety layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Variables visible to step interpolation as `{{ vars.<key> }}`.
    /// Values are arbitrary YAML scalars/sequences/maps; the
    /// interpolator stringifies them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, serde_yaml::Value>,

    /// Preflight gates. All must pass before any step runs. Empty by
    /// default — bundles without `preflight:` proceed straight to
    /// step execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preflight: Vec<Check>,

    /// Ordered list of steps. Required and non-empty.
    pub steps: Vec<Step>,

    /// Compensating actions run in REVERSE declaration order on any
    /// `on_failure: rollback` event (or the catch-all path when a
    /// step's per-step `rollback:` is unset).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rollback: Vec<Step>,

    /// Steps run after every primary step completes successfully. A
    /// failure here is reported but does NOT trigger rollback (the
    /// primary work has already landed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub postflight: Vec<Check>,
}

/// One executable step. Exactly one of `exec`, `run`, or `watch` must
/// be set; serde validation enforces this in [`Self::body_kind`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Stable step identifier (referenced by `requires:` and
    /// `rollback_to:`). Should be unique within a bundle.
    pub id: String,

    /// Override the bundle-level `host:` for this step. Same selector
    /// grammar as `inspect run`/`inspect exec`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,

    /// Audit-recorded command (state-changing). Mutually exclusive
    /// with `run` / `watch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec: Option<String>,

    /// Read-only command (no audit). Mutually exclusive with
    /// `exec` / `watch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,

    /// Inline watch step that delegates to the B10 engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<WatchStep>,

    /// L8 (v0.1.3): compose-aware step. Mutually exclusive with
    /// `exec` / `run` / `watch`. The `target:` (or bundle `host:`)
    /// supplies the namespace; the spec carries the project, action,
    /// optional service, and optional flag set. Validated at plan
    /// time against the namespace's cached profile (project must
    /// exist).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose: Option<ComposeStepSpec>,

    /// Steps that must complete (and pass) before this one starts.
    /// Validated up-front; cycles and forward refs fail plan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,

    /// Run this step concurrently across `matrix:` entries.
    #[serde(default, skip_serializing_if = "is_false")]
    pub parallel: bool,

    /// Matrix expansion. Each top-level key becomes a parallel branch
    /// keyed by `{{ matrix.<key> }}`. v0.1.2 supports a single key
    /// only — multiplexing across multiple matrix dimensions is
    /// deferred to v0.2.x to keep failure modes tractable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub matrix: BTreeMap<String, Vec<serde_yaml::Value>>,

    /// Concurrency cap for `parallel: true` matrix steps. Default =
    /// matrix size; hard-capped at [`MAX_PARALLEL_CAP`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,

    /// Failure routing. Defaults to `abort`.
    #[serde(default)]
    pub on_failure: OnFailure,

    /// `false` means "don't run any per-step rollback for this entry
    /// even on rollback paths" — useful for `pg_dump` and other write
    /// operations that are safe to leave behind.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub reversible: bool,

    /// Per-step `--apply` opt-out. When `false`, the step always runs
    /// even without a top-level `--apply`. Useful for read-only
    /// preflight probes interleaved with destructive ops.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub apply: bool,

    /// Per-step compensating action. Executed during rollback in
    /// reverse declaration order (subject to `reversible:`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback: Option<String>,

    /// Override the per-step timeout (seconds). Default 300s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,

    /// Step-level reason override; falls back to bundle-level reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Inline `watch:` body. Mirrors a subset of [`crate::cli::WatchArgs`]
/// — only the fields that make sense inside a bundle (selector is
/// inherited from `Step::target`/`Bundle::host`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchStep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_log: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_sql: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_http: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lt: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub changes: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_for: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub regex: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psql_opts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "match")]
    pub match_expr: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub insecure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
}

/// First-class preflight/postflight checks. Untagged so YAML reads
/// naturally (`check: disk_free` ...).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case", deny_unknown_fields)]
pub enum Check {
    /// `df -P <path>` on the target; pass if available bytes ≥ min_gb·2³⁰.
    DiskFree {
        path: String,
        min_gb: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    /// `docker inspect -f '{{.State.Running}}' <name>` for each container;
    /// pass iff every name reports `true`.
    DockerRunning {
        services: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    /// `inspect health <selector>` style probe (uses the local
    /// service-defs cache); pass iff every named service is `ok`.
    ServicesHealthy {
        services: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        /// Wait up to this duration (e.g. `60s`) for healthy state.
        /// Default `0s` (single probe).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<String>,
    },
    /// `curl -fsS <url>` on the target; pass iff curl exits 0 (HTTP 2xx/3xx).
    HttpOk {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    /// `psql -tAc <sql>` inside the named container; pass iff stdout
    /// trims to t/true/1/yes (case-insensitive).
    SqlReturns {
        container: String,
        sql: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        psql_opts: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    /// Escape hatch: arbitrary command on a target. Pass iff exit 0.
    Exec {
        exec: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OnFailure {
    /// Stop the bundle without running any rollback.
    #[default]
    Abort,
    /// Run rollback for completed reversible steps in reverse order.
    Rollback,
    /// Run rollback for completed reversible steps from the failed
    /// step back to (but NOT including) the named checkpoint, in
    /// reverse order. The named id must appear earlier in `steps:`.
    RollbackTo(String),
    /// Log the failure and continue with the next step.
    Continue,
}

// Custom deserializer: accepts either a scalar string ("abort",
// "rollback", "continue") for unit variants, or a single-key map
// `{rollback_to: <id>}` for the newtype variant. serde_yaml 0.9's
// default externally-tagged form needs `!rollback_to <id>` (YAML
// tag) for newtype variants, which is awkward to write by hand.
impl<'de> serde::Deserialize<'de> for OnFailure {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error, MapAccess, Visitor};
        use std::fmt;

        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = OnFailure;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("`abort` | `rollback` | `continue` | { rollback_to: <step-id> }")
            }
            fn visit_str<E: Error>(self, s: &str) -> Result<OnFailure, E> {
                match s {
                    "abort" => Ok(OnFailure::Abort),
                    "rollback" => Ok(OnFailure::Rollback),
                    "continue" => Ok(OnFailure::Continue),
                    "rollback_to" => Err(E::custom(
                        "`rollback_to` requires a step id; use `rollback_to: <id>`",
                    )),
                    other => Err(E::custom(format!("unknown on_failure mode `{other}`"))),
                }
            }
            fn visit_string<E: Error>(self, s: String) -> Result<OnFailure, E> {
                self.visit_str(&s)
            }
            fn visit_map<M: MapAccess<'de>>(self, mut m: M) -> Result<OnFailure, M::Error> {
                let key: Option<String> = m.next_key()?;
                let key = key.ok_or_else(|| M::Error::custom("empty on_failure map"))?;
                if key != "rollback_to" {
                    return Err(M::Error::custom(format!(
                        "unknown on_failure key `{key}` (expected `rollback_to`)"
                    )));
                }
                let id: String = m.next_value()?;
                if let Some(extra) = m.next_key::<String>()? {
                    return Err(M::Error::custom(format!(
                        "unexpected extra on_failure key `{extra}`"
                    )));
                }
                Ok(OnFailure::RollbackTo(id))
            }
        }
        d.deserialize_any(V)
    }
}

/// Resolved body kind. Set by [`Step::body_kind`] after parse so
/// downstream code branches on a closed enum instead of inspecting
/// `Option`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepBodyKind {
    Exec,
    Run,
    Watch,
    /// L8 (v0.1.3): structured compose action. Renders to a
    /// `cd <wd> && docker compose -p <p> <action> [flags] [<svc>]`
    /// shell command at execution time, plus an audit entry with
    /// `verb=compose.<action>` and `[project=…] [service=…]
    /// [compose_file_hash=…]` arg tags.
    Compose,
}

impl Step {
    /// Determine the step's body kind, returning a parse-style error
    /// if zero or more than one body field is set. Run during plan
    /// validation so a malformed bundle never reaches the executor.
    pub fn body_kind(&self) -> Result<StepBodyKind, String> {
        let mut count = 0;
        let mut kind = None;
        if self.exec.is_some() {
            count += 1;
            kind = Some(StepBodyKind::Exec);
        }
        if self.run.is_some() {
            count += 1;
            kind = Some(StepBodyKind::Run);
        }
        if self.watch.is_some() {
            count += 1;
            kind = Some(StepBodyKind::Watch);
        }
        if self.compose.is_some() {
            count += 1;
            kind = Some(StepBodyKind::Compose);
        }
        match (count, kind) {
            (1, Some(k)) => Ok(k),
            (0, _) => Err(format!(
                "step '{}' has no body — set exactly one of \
                 `exec:`, `run:`, `watch:`, or `compose:`",
                self.id
            )),
            _ => Err(format!(
                "step '{}' has multiple bodies — set exactly one of \
                 `exec:`, `run:`, `watch:`, or `compose:`",
                self.id
            )),
        }
    }
}

/// L8 (v0.1.3): structured compose-step spec. Mirrors the standalone
/// `inspect compose <action>` verbs so a bundle's compose step
/// inherits the same audit shape (`verb=compose.<action>`,
/// `[project=…] [service=…] [compose_file_hash=…]`) and the same
/// revert taxonomy (command_pair for up/down/restart, unsupported
/// for pull, command_pair `down --rmi local` for build).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeStepSpec {
    /// Compose project name. Plan-time validation requires this
    /// project to exist in the target namespace's cached profile.
    pub project: String,
    /// One of `up`, `down`, `pull`, `build`, `restart`.
    pub action: ComposeAction,
    /// Optional service narrowing. When set, the action runs against
    /// only that service (project's other services unaffected).
    /// Per-service `down` rejects `flags.volumes` / `flags.rmi` —
    /// both are project-scoped operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Per-action passthrough flags. Recognized keys depend on
    /// `action`:
    ///   up    : force_recreate (bool), no_detach (bool)
    ///   down  : volumes (bool), rmi (bool)
    ///   pull  : ignore_pull_failures (bool)
    ///   build : no_cache (bool), pull (bool)
    ///   restart : (none currently)
    /// Unknown keys are rejected at plan time so a typo doesn't
    /// silently no-op.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, serde_yaml::Value>,
}

/// L8 (v0.1.3): closed enum of compose actions a bundle step can
/// drive. Mirrors the sub-verb taxonomy. Serialized lowercase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposeAction {
    Up,
    Down,
    Pull,
    Build,
    Restart,
}

impl ComposeAction {
    pub fn as_str(self) -> &'static str {
        match self {
            ComposeAction::Up => "up",
            ComposeAction::Down => "down",
            ComposeAction::Pull => "pull",
            ComposeAction::Build => "build",
            ComposeAction::Restart => "restart",
        }
    }

    /// L8 (v0.1.3): which boolean flags this action recognizes. Any
    /// other key in the `flags:` map is a plan-time error.
    pub fn allowed_flags(self) -> &'static [&'static str] {
        match self {
            ComposeAction::Up => &["force_recreate", "no_detach"],
            ComposeAction::Down => &["volumes", "rmi"],
            ComposeAction::Pull => &["ignore_pull_failures"],
            ComposeAction::Build => &["no_cache", "pull"],
            ComposeAction::Restart => &[],
        }
    }
}

fn default_true() -> bool {
    true
}
fn is_false(b: &bool) -> bool {
    !*b
}
fn is_true(b: &bool) -> bool {
    *b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Bundle {
        serde_yaml::from_str(yaml).expect("parse")
    }

    #[test]
    fn minimal_bundle_parses() {
        let b = parse(
            r#"
name: hello
steps:
  - id: a
    exec: echo hi
"#,
        );
        assert_eq!(b.name, "hello");
        assert_eq!(b.steps.len(), 1);
        assert_eq!(b.steps[0].body_kind().unwrap(), StepBodyKind::Exec);
        assert_eq!(b.steps[0].on_failure, OnFailure::Abort);
        assert!(b.steps[0].reversible);
        assert!(b.steps[0].apply);
    }

    #[test]
    fn unknown_top_level_field_is_rejected() {
        let r: Result<Bundle, _> = serde_yaml::from_str(
            r#"
name: x
typo_field: yes
steps:
  - id: a
    exec: true
"#,
        );
        assert!(r.is_err(), "deny_unknown_fields must catch typos");
    }

    #[test]
    fn unknown_step_field_is_rejected() {
        let r: Result<Bundle, _> = serde_yaml::from_str(
            r#"
name: x
steps:
  - id: a
    exec: true
    max_paralllel: 2
"#,
        );
        assert!(r.is_err(), "deny_unknown_fields catches step typos");
    }

    #[test]
    fn missing_body_is_caught_by_body_kind() {
        let b = parse(
            r#"
name: x
steps:
  - id: a
"#,
        );
        let err = b.steps[0].body_kind().unwrap_err();
        assert!(err.contains("has no body"));
    }

    #[test]
    fn multiple_bodies_are_caught() {
        let b = parse(
            r#"
name: x
steps:
  - id: a
    exec: echo
    run: echo
"#,
        );
        let err = b.steps[0].body_kind().unwrap_err();
        assert!(err.contains("multiple bodies"));
    }

    #[test]
    fn on_failure_enum_round_trips() {
        let b = parse(
            r#"
name: x
steps:
  - id: a
    exec: echo
    on_failure: rollback
  - id: b
    exec: echo
    on_failure:
      rollback_to: a
  - id: c
    exec: echo
    on_failure: continue
"#,
        );
        assert_eq!(b.steps[0].on_failure, OnFailure::Rollback);
        assert_eq!(
            b.steps[1].on_failure,
            OnFailure::RollbackTo("a".to_string())
        );
        assert_eq!(b.steps[2].on_failure, OnFailure::Continue);
    }

    #[test]
    fn checks_round_trip_all_kinds() {
        let b = parse(
            r#"
name: x
preflight:
  - check: disk_free
    path: /srv
    min_gb: 50
  - check: docker_running
    services: [a, b]
  - check: services_healthy
    services: [a]
    timeout: 60s
  - check: http_ok
    url: http://x/y
  - check: sql_returns
    container: pg
    sql: "SELECT 1=1"
  - check: exec
    exec: "true"
steps:
  - id: a
    exec: echo
"#,
        );
        assert_eq!(b.preflight.len(), 6);
    }

    #[test]
    fn matrix_and_parallel_parse() {
        let b = parse(
            r#"
name: x
steps:
  - id: tar
    parallel: true
    max_parallel: 4
    matrix:
      volume: [a, b, c]
    exec: tar {{ matrix.volume }}
"#,
        );
        let s = &b.steps[0];
        assert!(s.parallel);
        assert_eq!(s.max_parallel, Some(4));
        assert_eq!(s.matrix.get("volume").unwrap().len(), 3);
    }
}
