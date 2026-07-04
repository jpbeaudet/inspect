//! K1 (v0.1.4): the runtime executor abstraction.
//!
//! Historically inspect assumed a single runtime — docker-over-SSH —
//! structurally: `discovery/probes.rs` builds `docker ps`/`docker
//! inspect` inline and the write/read verbs build `docker
//! logs|exec|restart|…` inline. There was no seam at which a different
//! runtime (kubernetes via `kubectl`) could be swapped in.
//!
//! This module introduces that seam. [`Runtime`] abstracts the three
//! runtime-specific concerns the surface map §2 enumerates — command
//! building, inventory, and failure classification — behind an
//! object-safe trait with two implementations:
//!
//! - [`DockerRuntime`] — reproduces the **exact** command strings the
//!   docker verbs already build today. It is a behavior-preserving
//!   extraction: the strings it emits are byte-identical to the inline
//!   `format!("docker …")` sites (see the `k1_docker_runtime_parity_*`
//!   acceptance tests, which pin that equality). Introducing it changes
//!   no docker behavior — the existing docker test suite is the
//!   regression gate.
//! - [`K8sRuntime`] — the kubernetes runtime, backed by `kubectl`
//!   shell-out (Q2). K1 lands the command-assembly skeleton + trait
//!   wiring; the individual k8s verbs land fully in later K-items
//!   (discovery/read in Wave B, native reads in Wave C, writes in Wave
//!   D). No user path reaches `K8sRuntime` in K1: runtime selection
//!   defaults to docker and the namespace `type = "k8s"` field that
//!   would select it is not introduced until K2. `K8sRuntime` is
//!   exercised only by unit tests here.
//!
//! **This is not the transport layer.** [`crate::verbs::runtime::
//! RemoteRunner`] remains the transport abstraction (how a command
//! string is *dispatched* — SSH master socket vs mock). `Runtime` is
//! the orthogonal *command-building* + *inventory* + *classification*
//! axis (what command string to build for a given runtime). The two
//! compose: a verb asks its `Runtime` for a command string, then hands
//! that string to a `RemoteRunner` (docker) or dispatches it locally
//! (k8s — no SSH; a later Wave-A item wires the k8s transport path).

// K1 introduces this seam; its non-test consumers land in the very next
// item (K2 wires `RuntimeKind` to the namespace `type` config field and
// selects the runtime at dispatch). In K1 the API is exercised only by
// the in-module unit tests, so the normal (non-test) build sees it as
// unused. This allowance is removed the moment K2 wires the call sites.
#![allow(dead_code)]

use crate::ssh::transport::{classify, TransportClass};
use crate::verbs::quote::shquote;

/// Which runtime backs a namespace. Selected from the namespace `type`
/// config field (wired in K2); defaults to [`RuntimeKind::Docker`] so
/// every existing configuration is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Docker,
    K8s,
}

impl RuntimeKind {
    /// Map a namespace `type` field to a runtime. Absent / `"docker"`
    /// → docker (the no-change default); `"k8s"` / `"kubernetes"` →
    /// k8s. Unknown values fall back to docker — K2 adds the
    /// validation that rejects an unknown `type` loudly at config
    /// parse time; this helper stays total so runtime selection can
    /// never panic on a malformed config.
    pub fn from_type(type_field: Option<&str>) -> Self {
        match type_field.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("k8s") | Some("kubernetes") => RuntimeKind::K8s,
            _ => RuntimeKind::Docker,
        }
    }

    /// Stable label for the `meta.runtime` envelope field and audit.
    pub fn label(self) -> &'static str {
        match self {
            RuntimeKind::Docker => "docker",
            RuntimeKind::K8s => "k8s",
        }
    }
}

/// Lifecycle action a runtime knows how to translate into a command.
/// Mirrors the docker `verbs/write/lifecycle.rs` `Action` set for the
/// container arm (systemd/host-listener arms stay in that verb — they
/// are not a runtime concern).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    Restart,
    Stop,
    Start,
    Reload,
}

/// Options for a logs invocation, runtime-agnostic. The docker impl
/// maps these onto `docker logs [-f] [--since X] [--tail N]`; the k8s
/// impl maps them onto `kubectl logs [-f] [--since X] [--tail N]`
/// (the `-c` / `--previous` / `--merged` flags land in K8).
#[derive(Debug, Clone, Default)]
pub struct LogsSpec {
    /// `-f` / `--follow`.
    pub follow: bool,
    /// `--since <value>` (docker duration or absolute per today's
    /// logs verb); `None` omits the flag.
    pub since: Option<String>,
    /// `--tail <n>`; `None` omits the flag.
    pub tail: Option<usize>,
    /// When `true`, prefix `stdbuf -oL -eL` so `-f` output is
    /// line-buffered — the docker follow path does this (see
    /// `verbs/logs.rs` field pitfall §5.1). Ignored by the k8s impl.
    pub line_buffered: bool,
}

/// The runtime executor seam. Object-safe (`Box<dyn Runtime>`), so
/// selection is a runtime value, not a generic parameter.
pub trait Runtime: Send + Sync {
    /// Which runtime this is.
    fn kind(&self) -> RuntimeKind;

    /// The inventory command — the "what is running here" probe that
    /// discovery runs to populate the cached profile. Docker →
    /// `docker ps …`; k8s → `kubectl get pods,services,deployments,
    /// configmaps -o json` (one round-trip, no per-object fan-out).
    fn inventory_cmd(&self) -> String;

    /// Build a **read** in-container/in-pod exec: run `cmd` inside
    /// `target` for a read-only verb (`cat`/`ls`/`grep`/`run`).
    fn build_read_exec(&self, target: &str, cmd: &str) -> String;

    /// Build a **write** in-container/in-pod exec: same shape, used by
    /// the audited write path (`exec --apply`). Docker does not
    /// distinguish read vs write exec; k8s keeps them separate so the
    /// write path can be audited/gated independently (K19).
    fn build_write_exec(&self, target: &str, cmd: &str) -> String;

    /// Build the logs command for `target` per `spec`.
    fn build_logs(&self, target: &str, spec: &LogsSpec) -> String;

    /// Build a lifecycle command (`restart`/`stop`/`start`/`reload`)
    /// for `target`.
    fn build_lifecycle(&self, action: LifecycleAction, target: &str) -> String;

    /// Classify a failure from its stderr + exit code into a
    /// transport/failure class an agent can branch on. The docker impl
    /// delegates to the existing SSH transport classifier; the k8s
    /// body (RBAC-forbidden / not-found / no-shell / metrics-
    /// unavailable / k8s transport) lands in K4 — K1 only defines the
    /// trait method + the docker impl.
    fn classify_failure(&self, stderr: &str, exit_code: i32) -> Option<TransportClass>;
}

/// Docker runtime — the behavior-preserving extraction of today's
/// inline `docker …` command building.
#[derive(Debug, Default, Clone, Copy)]
pub struct DockerRuntime;

impl Runtime for DockerRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Docker
    }

    fn inventory_cmd(&self) -> String {
        // Reproduces the discovery ps probe (see
        // `discovery/drift.rs`). Container inventory batches a
        // follow-up `docker inspect` per id in `discovery/probes.rs`;
        // that fan-out stays in the probe layer.
        "docker ps --format '{{.ID}}\\t{{.Names}}\\t{{.Image}}\\t{{.Ports}}' 2>/dev/null".to_string()
    }

    fn build_read_exec(&self, target: &str, cmd: &str) -> String {
        // Matches `bundle/exec.rs` / in-container reader exec shape.
        format!("docker exec {} sh -c {}", shquote(target), shquote(cmd))
    }

    fn build_write_exec(&self, target: &str, cmd: &str) -> String {
        // Docker has no read/write exec distinction.
        format!("docker exec {} sh -c {}", shquote(target), shquote(cmd))
    }

    fn build_logs(&self, target: &str, spec: &LogsSpec) -> String {
        // Reproduces `verbs/logs.rs` one-invocation shape: optional
        // stdbuf line-buffer prefix, then --since/--tail/-f, then the
        // quoted container.
        let mut s = if spec.line_buffered {
            String::from("stdbuf -oL -eL docker logs")
        } else {
            String::from("docker logs")
        };
        if let Some(since) = &spec.since {
            s.push_str(&format!(" --since {}", shquote(since)));
        }
        if let Some(tail) = spec.tail {
            s.push_str(&format!(" --tail {tail}"));
        }
        if spec.follow {
            s.push_str(" -f");
        }
        s.push(' ');
        s.push_str(&shquote(target));
        s
    }

    fn build_lifecycle(&self, action: LifecycleAction, target: &str) -> String {
        // Byte-identical to the container arm of
        // `verbs/write/lifecycle.rs::build_cmd`.
        let q = shquote(target);
        match action {
            LifecycleAction::Restart => format!("docker restart {q}"),
            LifecycleAction::Stop => format!("docker stop {q}"),
            LifecycleAction::Start => format!("docker start {q}"),
            LifecycleAction::Reload => format!("docker kill -s HUP {q}"),
        }
    }

    fn classify_failure(&self, stderr: &str, _exit_code: i32) -> Option<TransportClass> {
        // Today's docker path classifies purely on stderr text; the
        // exit code is not consulted. Preserve that.
        classify(stderr)
    }
}

/// Kubernetes runtime — kubectl shell-out backend (Q2).
///
/// Carries the resolved kubeconfig `context` and k8s `namespace` so
/// every command it builds pins `--context` explicitly and never reads
/// the ambient `current-context` (the K5 anti-footgun invariant; the
/// full enforcement + resolved-target echo land in K5). K1 ships the
/// command-assembly skeleton; the verbs that consume it land in Waves
/// B–D. No user path constructs a `K8sRuntime` in K1.
#[derive(Debug, Default, Clone)]
pub struct K8sRuntime {
    /// kubeconfig context to pin on every call (`--context`). When
    /// `None`, no `--context` is emitted — K2/K5 make this required
    /// for a real k8s namespace; the field is `Option` here only so
    /// the unit-test scaffold can construct a bare runtime.
    pub context: Option<String>,
    /// k8s namespace to scope reads/writes (`-n`). `None` → the
    /// kubectl default namespace (the `-n`/`-A` override lands in the
    /// verbs, K7+).
    pub namespace: Option<String>,
}

impl K8sRuntime {
    /// Construct with an explicit context + namespace.
    pub fn new(context: Option<String>, namespace: Option<String>) -> Self {
        Self { context, namespace }
    }

    /// Emit the `--context <ctx> -n <ns>` scoping flags this runtime
    /// pins on every kubectl invocation (K5 invariant seed).
    fn scope_flags(&self) -> String {
        let mut s = String::new();
        if let Some(ctx) = &self.context {
            s.push_str(&format!("--context {} ", shquote(ctx)));
        }
        if let Some(ns) = &self.namespace {
            s.push_str(&format!("-n {} ", shquote(ns)));
        }
        s
    }
}

impl Runtime for K8sRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::K8s
    }

    fn inventory_cmd(&self) -> String {
        // K6 consumes this for discovery; one API round-trip.
        format!(
            "kubectl {}get pods,services,deployments,configmaps -o json",
            self.scope_flags()
        )
    }

    fn build_read_exec(&self, target: &str, cmd: &str) -> String {
        // No `/bin/bash` wrapper — avoids the k9s Alpine-no-bash trap
        // (research w1-D8). `--` separates kubectl flags from the
        // in-pod command. K9 refines container selection (`-c`).
        format!(
            "kubectl {}exec {} -- {}",
            self.scope_flags(),
            shquote(target),
            cmd
        )
    }

    fn build_write_exec(&self, target: &str, cmd: &str) -> String {
        // K19 gates/audits this under `--apply`.
        format!(
            "kubectl {}exec {} -- {}",
            self.scope_flags(),
            shquote(target),
            cmd
        )
    }

    fn build_logs(&self, target: &str, spec: &LogsSpec) -> String {
        // K8 adds `-c` auto-pick / `--previous` / `--merged`.
        let mut s = format!("kubectl {}logs", self.scope_flags());
        if let Some(since) = &spec.since {
            s.push_str(&format!(" --since={}", shquote(since)));
        }
        if let Some(tail) = spec.tail {
            s.push_str(&format!(" --tail={tail}"));
        }
        if spec.follow {
            s.push_str(" -f");
        }
        s.push(' ');
        s.push_str(&shquote(target));
        s
    }

    fn build_lifecycle(&self, action: LifecycleAction, target: &str) -> String {
        // The community-correct idioms (K15/K16): restart →
        // rollout restart; stop/start map to scale (refined in the
        // write wave with the outage guard). `reload` folds into
        // rollout restart.
        let scope = self.scope_flags();
        let q = shquote(target);
        match action {
            LifecycleAction::Restart | LifecycleAction::Reload => {
                format!("kubectl {scope}rollout restart deploy/{q}")
            }
            LifecycleAction::Stop => format!("kubectl {scope}scale deploy/{q} --replicas=0"),
            LifecycleAction::Start => format!("kubectl {scope}scale deploy/{q} --replicas=1"),
        }
    }

    fn classify_failure(&self, _stderr: &str, _exit_code: i32) -> Option<TransportClass> {
        // K4 fills the k8s stderr classifier (rbac_forbidden /
        // not_found / no_shell_in_container / metrics_unavailable / the
        // k8s transport classes). K1 leaves it unclassified — no user
        // path reaches it yet.
        None
    }
}

/// Factory: the runtime-selection mechanism. Returns a boxed trait
/// object for `kind`. Wiring `kind` to the namespace `type` config
/// field is K2; K1 makes selection testable via an explicit
/// [`RuntimeKind`] and defaults callers to docker so docker behavior
/// is unchanged.
pub fn runtime_for(kind: RuntimeKind) -> Box<dyn Runtime> {
    match kind {
        RuntimeKind::Docker => Box::new(DockerRuntime),
        RuntimeKind::K8s => Box::new(K8sRuntime::default()),
    }
}

// K1 acceptance tests (v0.1.4). These live in-module because the crate
// is bin-only (no `[lib]` target): the `tests/phase_k_v014.rs`
// integration file is black-box (`assert_cmd`-driven) and cannot import
// these internal Rust APIs. Names follow the `k1_*` convention so they
// are greppable as the K1 acceptance set.
#[cfg(test)]
mod tests {
    use super::*;

    // ---- k1_docker_runtime_parity_* — byte-identical to the inline
    // `docker …` construction the verbs already use.

    #[test]
    fn k1_docker_runtime_parity_lifecycle() {
        let rt = DockerRuntime;
        let c = "luminary-api";
        let q = shquote(c);
        // Byte-identical to `verbs/write/lifecycle.rs::build_cmd` container arm.
        assert_eq!(
            rt.build_lifecycle(LifecycleAction::Restart, c),
            format!("docker restart {q}")
        );
        assert_eq!(
            rt.build_lifecycle(LifecycleAction::Stop, c),
            format!("docker stop {q}")
        );
        assert_eq!(
            rt.build_lifecycle(LifecycleAction::Start, c),
            format!("docker start {q}")
        );
        assert_eq!(
            rt.build_lifecycle(LifecycleAction::Reload, c),
            format!("docker kill -s HUP {q}")
        );
    }

    #[test]
    fn k1_docker_runtime_parity_read_exec() {
        let rt = DockerRuntime;
        // Matches `bundle/exec.rs` in-container exec shape.
        let got = rt.build_read_exec("atlas-pg", "cat /etc/hostname");
        assert_eq!(
            got,
            format!(
                "docker exec {} sh -c {}",
                shquote("atlas-pg"),
                shquote("cat /etc/hostname")
            )
        );
        // Read and write exec are identical for docker (no distinction).
        assert_eq!(rt.build_write_exec("atlas-pg", "cat /etc/hostname"), got);
    }

    #[test]
    fn k1_docker_runtime_parity_logs() {
        let rt = DockerRuntime;
        assert_eq!(
            rt.build_logs("api", &LogsSpec::default()),
            format!("docker logs {}", shquote("api"))
        );
        let spec = LogsSpec {
            follow: true,
            since: Some("30m".to_string()),
            tail: Some(100),
            line_buffered: true,
        };
        assert_eq!(
            rt.build_logs("api", &spec),
            format!(
                "stdbuf -oL -eL docker logs --since {} --tail 100 -f {}",
                shquote("30m"),
                shquote("api")
            )
        );
    }

    #[test]
    fn k1_docker_runtime_parity_inventory_is_docker_ps() {
        let cmd = DockerRuntime.inventory_cmd();
        assert!(cmd.starts_with("docker ps"), "got: {cmd}");
        assert!(cmd.contains("--format"));
    }

    #[test]
    fn k1_docker_runtime_parity_kind_and_label() {
        assert_eq!(DockerRuntime.kind(), RuntimeKind::Docker);
        assert_eq!(RuntimeKind::Docker.label(), "docker");
        assert_eq!(RuntimeKind::K8s.label(), "k8s");
    }

    // ---- k1_runtime_selected_by_namespace_type

    #[test]
    fn k1_runtime_selected_by_namespace_type() {
        assert_eq!(
            runtime_for(RuntimeKind::from_type(Some("k8s"))).kind(),
            RuntimeKind::K8s
        );
        assert_eq!(
            runtime_for(RuntimeKind::from_type(Some("kubernetes"))).kind(),
            RuntimeKind::K8s
        );
        assert_eq!(
            runtime_for(RuntimeKind::from_type(Some("K8S"))).kind(),
            RuntimeKind::K8s
        );
        assert_eq!(
            runtime_for(RuntimeKind::from_type(Some(" k8s "))).kind(),
            RuntimeKind::K8s
        );
        // docker / absent / unknown all default to docker — the
        // no-change guarantee; selection stays total.
        assert_eq!(
            runtime_for(RuntimeKind::from_type(Some("docker"))).kind(),
            RuntimeKind::Docker
        );
        assert_eq!(
            runtime_for(RuntimeKind::from_type(None)).kind(),
            RuntimeKind::Docker
        );
        assert_eq!(
            runtime_for(RuntimeKind::from_type(Some(""))).kind(),
            RuntimeKind::Docker
        );
        assert_eq!(
            runtime_for(RuntimeKind::from_type(Some("nope"))).kind(),
            RuntimeKind::Docker
        );
    }

    // ---- k1_runtime_trait_object_safe

    #[test]
    fn k1_runtime_trait_object_safe() {
        // If `Runtime` were not object-safe this would not compile.
        let runtimes: Vec<Box<dyn Runtime>> = vec![
            Box::new(DockerRuntime),
            Box::new(K8sRuntime::default()),
            runtime_for(RuntimeKind::Docker),
            runtime_for(RuntimeKind::K8s),
        ];
        let kinds: Vec<RuntimeKind> = runtimes.iter().map(|r| r.kind()).collect();
        assert_eq!(
            kinds,
            vec![
                RuntimeKind::Docker,
                RuntimeKind::K8s,
                RuntimeKind::Docker,
                RuntimeKind::K8s
            ]
        );
        let rt: Box<dyn Runtime> = Box::new(DockerRuntime);
        assert_eq!(
            rt.build_lifecycle(LifecycleAction::Restart, "c"),
            "docker restart 'c'"
        );
    }

    // ---- k8s runtime scaffold (unit-level; no user path reaches it in K1)

    #[test]
    fn k1_k8s_runtime_pins_context_and_namespace() {
        // K5 invariant seed: every kubectl command carries an explicit
        // --context and -n; the ambient current-context is never used.
        let rt = K8sRuntime::new(Some("prod-eks".to_string()), Some("payments".to_string()));
        let inv = rt.inventory_cmd();
        assert!(inv.contains("--context 'prod-eks'"), "got: {inv}");
        assert!(inv.contains("-n 'payments'"), "got: {inv}");
        assert!(inv.contains("kubectl"));
        // restart maps to the community-correct rollout restart, not delete-pod.
        let restart = rt.build_lifecycle(LifecycleAction::Restart, "api");
        assert!(restart.contains("rollout restart deploy/"), "got: {restart}");
        assert!(restart.contains("--context 'prod-eks'"));
    }

    #[test]
    fn k1_k8s_runtime_classify_is_unwired_in_k1() {
        // K4 fills the k8s classifier; K1 leaves it None (no user path).
        assert_eq!(K8sRuntime::default().classify_failure("Forbidden", 1), None);
    }
}
