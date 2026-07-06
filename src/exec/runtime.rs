//! The runtime executor abstraction.
//!
//! Historically inspect assumed a single runtime — docker-over-SSH —
//! structurally: `discovery/probes.rs` builds `docker ps`/`docker
//! inspect` inline and the write/read verbs build `docker
//! logs|exec|restart|…` inline. There was no seam at which a different
//! runtime (kubernetes via `kubectl`) could be swapped in.
//!
//! This module introduces that seam for the **docker** command-string
//! builders. [`Runtime`] abstracts the runtime-specific command-building +
//! inventory concerns the surface map §2 enumerates, behind an object-safe
//! trait. It provides the byte-clean docker builders (`inventory_cmd`,
//! `build_read_exec`, `build_write_exec`, `build_lifecycle`), used concretely
//! via `DockerRuntime` at a few sites.
//!
//! **What actually became the k8s seam (corrected 2026-07-06, exit-gate
//! audit):** the later-wave k8s concerns did NOT grow this trait. The k8s
//! stderr→`failure_class` classifier, the `logs`/exec builders, and per-verb
//! dispatch all landed as the free-function family in
//! [`crate::exec::kubectl`] + a per-verb `runtime_kind()` branch. The
//! `runtime_kind()` discriminator lives on [`crate::config::namespace::
//! NamespaceConfig`], not as a trait method. This trait carries the
//! docker builders; it is not the k8s dispatch path. See the [`K8sRuntime`]
//! note below.
//!
//! - [`DockerRuntime`] — reproduces the **exact** command strings the
//!   docker verbs already build today. It is a behavior-preserving
//!   extraction: the strings it emits are byte-identical to the inline
//!   `format!("docker …")` sites (see the `k1_docker_runtime_parity_*`
//!   acceptance tests, which pin that equality). Introducing it changes
//!   no docker behavior — the existing docker test suite is the
//!   regression gate.
//! - [`K8sRuntime`] — a **dormant** command-string builder for the
//!   kubernetes runtime, exercised only by the `k1_*` parity unit tests.
//!   **It is NOT the live k8s backend** (corrected 2026-07-06, exit-gate
//!   audit). It landed as the seed of a trait-carried k8s runtime,
//!   but Waves B–D wired the actual k8s verbs to the shared kubectl helper
//!   family in [`crate::exec::kubectl`] (`kubectl_base` / `exec_base` /
//!   `exec_in_pod` / `classify_kubectl_failure` / `revert_kubectl_prefix`),
//!   dispatched by a per-verb `runtime_kind()` branch — NOT through
//!   `Box<dyn Runtime>`. So `K8sRuntime` never went live: no real path
//!   constructs it, and `runtime_for(RuntimeKind::K8s)` is called only in
//!   tests. It is retained as the **explicitly deferred seed** (v0.1.5+, JP
//!   2026-07-06) for a future trait-as-seam refactor or a `kube-rs` swap; if
//!   that decision lands, the per-verb fork promotes into `Runtime` methods
//!   and this impl becomes live. Until then it is dormant-by-design, not a
//!   half-wired verb. **When adding a k8s verb, wire it to the
//!   `crate::exec::kubectl` helpers + a `runtime_kind()` branch — do NOT add
//!   a method here expecting dispatch to reach it.**
//!
//! **This is not the transport layer.** [`crate::verbs::runtime::
//! RemoteRunner`] remains the transport abstraction (how a command
//! string is *dispatched* — SSH master socket vs mock). `Runtime` is
//! the orthogonal *command-building* + *inventory* axis (what command
//! string to build for a given runtime). The two
//! compose: a verb asks its `Runtime` for a command string, then hands
//! that string to a `RemoteRunner` (docker) or dispatches it locally
//! (k8s — no SSH; a later Wave-A item wires the k8s transport path).

use crate::verbs::quote::shquote;

/// Which runtime backs a namespace. Selected from the namespace `type`
/// config field (wired in the config layer); defaults to [`RuntimeKind::Docker`] so
/// every existing configuration is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Docker,
    K8s,
}

impl RuntimeKind {
    /// Map a namespace `type` field to a runtime. Absent / `"docker"`
    /// → docker (the no-change default); `"k8s"` / `"kubernetes"` →
    /// k8s. Unknown values fall back to docker — config parsing adds the
    /// validation that rejects an unknown `type` loudly at config
    /// parse time; this helper stays total so runtime selection can
    /// never panic on a malformed config.
    pub fn from_type(type_field: Option<&str>) -> Self {
        match type_field
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("k8s") | Some("kubernetes") => RuntimeKind::K8s,
            _ => RuntimeKind::Docker,
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

/// The docker command-builder seam. Object-safe (`Box<dyn Runtime>`), but
/// used **concretely** as `DockerRuntime` at the docker call sites — NOT as a
/// dispatch seam (corrected 2026-07-06, exit-gate audit).
///
/// This trait's byte-clean command builders migrate off inline
/// `docker …` construction with zero behavior change: `inventory_cmd`,
/// `build_read_exec`, `build_write_exec`, `build_lifecycle` — each wired to
/// its real docker call site.
///
/// The later-wave k8s concerns did **not** grow this trait, contrary to its
/// original intent: the k8s `logs` builder, the stderr→`failure_class`
/// classifier, and the `runtime_kind()` discriminator + target
/// resolution all landed as the free-function family in
/// [`crate::exec::kubectl`] + `NamespaceConfig::runtime_kind()` + a per-verb
/// dispatch fork — not as trait methods. So this trait carries only the docker
/// builders; a future trait-as-seam refactor (v0.1.5+, JP-authorized) would
/// add the k8s methods and revive [`K8sRuntime`]. See the module doc above.
pub trait Runtime: Send + Sync {
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
    /// write path can be audited/gated independently.
    fn build_write_exec(&self, target: &str, cmd: &str) -> String;

    /// Build a lifecycle command (`restart`/`stop`/`start`/`reload`)
    /// for `target`.
    fn build_lifecycle(&self, action: LifecycleAction, target: &str) -> String;
}

/// Docker runtime — the behavior-preserving extraction of today's
/// inline `docker …` command building.
#[derive(Debug, Default, Clone, Copy)]
pub struct DockerRuntime;

impl Runtime for DockerRuntime {
    fn inventory_cmd(&self) -> String {
        // Reproduces the discovery ps probe (see
        // `discovery/drift.rs`). Container inventory batches a
        // follow-up `docker inspect` per id in `discovery/probes.rs`;
        // that fan-out stays in the probe layer.
        "docker ps --format '{{.ID}}\\t{{.Names}}\\t{{.Image}}\\t{{.Ports}}' 2>/dev/null"
            .to_string()
    }

    fn build_read_exec(&self, target: &str, cmd: &str) -> String {
        // Matches `bundle/exec.rs` / in-container reader exec shape.
        format!("docker exec {} sh -c {}", shquote(target), shquote(cmd))
    }

    fn build_write_exec(&self, target: &str, cmd: &str) -> String {
        // Docker has no read/write exec distinction.
        format!("docker exec {} sh -c {}", shquote(target), shquote(cmd))
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
}

/// Kubernetes runtime — kubectl shell-out backend.
///
/// Carries the resolved kubeconfig `context` and k8s `namespace` so
/// every command it builds pins `--context` explicitly and never reads
/// the ambient `current-context` (the anti-footgun invariant; the
/// full enforcement + resolved-target echo land with the write verbs).
/// This ships the command-assembly skeleton; the verbs that consume it
/// land in later waves. No user path constructs a `K8sRuntime` yet.
#[derive(Debug, Default, Clone)]
pub struct K8sRuntime {
    /// kubeconfig context to pin on every call (`--context`). When
    /// `None`, no `--context` is emitted — config validation makes this required
    /// for a real k8s namespace; the field is `Option` here only so
    /// the unit-test scaffold can construct a bare runtime.
    pub context: Option<String>,
    /// k8s namespace to scope reads/writes (`-n`). `None` → the
    /// kubectl default namespace (the `-n`/`-A` override lands in the
    /// verbs).
    pub namespace: Option<String>,
}

impl K8sRuntime {
    /// Emit the `--context <ctx> -n <ns>` scoping flags this runtime
    /// pins on every kubectl invocation (anti-footgun invariant seed).
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
    fn inventory_cmd(&self) -> String {
        // Consumed for discovery; one API round-trip.
        format!(
            "kubectl {}get pods,services,deployments,configmaps -o json",
            self.scope_flags()
        )
    }

    fn build_read_exec(&self, target: &str, cmd: &str) -> String {
        // No `/bin/bash` wrapper — avoids the k9s Alpine-no-bash trap.
        // `--` separates kubectl flags from the in-pod command.
        // Container selection (`-c`) is refined later.
        format!(
            "kubectl {}exec {} -- {}",
            self.scope_flags(),
            shquote(target),
            cmd
        )
    }

    fn build_write_exec(&self, target: &str, cmd: &str) -> String {
        // Gated/audited under `--apply`.
        format!(
            "kubectl {}exec {} -- {}",
            self.scope_flags(),
            shquote(target),
            cmd
        )
    }

    fn build_lifecycle(&self, action: LifecycleAction, target: &str) -> String {
        // The community-correct idioms: restart →
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
}

/// Factory: the runtime-selection mechanism. Returns a boxed trait
/// object for `kind`. Wiring [`RuntimeKind`] to the namespace `type`
/// config field happens in the config layer; this factory makes selection testable via an explicit
/// [`RuntimeKind`] and defaults callers to docker so docker behavior
/// is unchanged.
pub fn runtime_for(kind: RuntimeKind) -> Box<dyn Runtime> {
    match kind {
        RuntimeKind::Docker => Box::new(DockerRuntime),
        RuntimeKind::K8s => Box::new(K8sRuntime::default()),
    }
}

// Runtime command-builder acceptance tests. These live in-module because
// the crate is bin-only (no `[lib]` target): the `tests/phase_k_v014.rs`
// integration file is black-box (`assert_cmd`-driven) and cannot import
// these internal Rust APIs. Names follow the `k1_*` convention so they
// are greppable as the acceptance set.
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
    fn k1_docker_runtime_parity_inventory_is_docker_ps() {
        let cmd = DockerRuntime.inventory_cmd();
        // Byte-identical to the discovery ps probe (`discovery/drift.rs`).
        assert_eq!(
            cmd,
            "docker ps --format '{{.ID}}\\t{{.Names}}\\t{{.Image}}\\t{{.Ports}}' 2>/dev/null"
        );
    }

    // ---- k1_runtime_selected_by_namespace_type
    //
    // Discriminates by the runtime's *observable behavior* (the command
    // string it builds) rather than a label — a stronger check that the
    // factory routed to the right impl. `from_type` maps the namespace
    // `type` field to a runtime; None/"docker" → docker,
    // "k8s"/"kubernetes" → k8s, unknown → docker (selection stays total).

    fn restart_cmd(type_field: Option<&str>) -> String {
        runtime_for(RuntimeKind::from_type(type_field))
            .build_lifecycle(LifecycleAction::Restart, "w")
    }

    #[test]
    fn k1_runtime_selected_by_namespace_type() {
        // k8s `type` routes to K8sRuntime (kubectl rollout restart).
        for t in ["k8s", "kubernetes", "K8S", " k8s "] {
            let cmd = restart_cmd(Some(t));
            assert!(
                cmd.starts_with("kubectl") && cmd.contains("rollout restart deploy/"),
                "type {t:?} should route to k8s, got: {cmd}"
            );
        }
        // docker / absent / empty / unknown all route to DockerRuntime.
        for t in [Some("docker"), None, Some(""), Some("nope")] {
            let cmd = restart_cmd(t);
            assert_eq!(
                cmd, "docker restart 'w'",
                "type {t:?} should route to docker"
            );
        }
    }

    // ---- k1_runtime_trait_object_safe

    #[test]
    fn k1_runtime_trait_object_safe() {
        // If `Runtime` were not object-safe none of this would compile:
        // it is held behind `Box<dyn Runtime>` and dispatched virtually.
        let runtimes: Vec<Box<dyn Runtime>> = vec![
            Box::new(DockerRuntime),
            Box::new(K8sRuntime::default()),
            runtime_for(RuntimeKind::Docker),
            runtime_for(RuntimeKind::K8s),
        ];
        let cmds: Vec<String> = runtimes
            .iter()
            .map(|r| r.build_lifecycle(LifecycleAction::Restart, "w"))
            .collect();
        assert_eq!(cmds[0], "docker restart 'w'");
        assert!(cmds[1].starts_with("kubectl"));
        assert_eq!(cmds[2], "docker restart 'w'");
        assert!(cmds[3].starts_with("kubectl"));
    }

    // ---- k8s runtime scaffold (unit-level; no user path reaches it yet)

    #[test]
    fn k1_k8s_runtime_pins_context_and_namespace() {
        // Anti-footgun invariant seed: every kubectl command carries an explicit
        // --context and -n; the ambient current-context is never used.
        let rt = K8sRuntime {
            context: Some("prod-eks".to_string()),
            namespace: Some("payments".to_string()),
        };
        let inv = rt.inventory_cmd();
        assert!(inv.contains("--context 'prod-eks'"), "got: {inv}");
        assert!(inv.contains("-n 'payments'"), "got: {inv}");
        assert!(inv.contains("kubectl"));
        // restart maps to the community-correct rollout restart, not delete-pod.
        let restart = rt.build_lifecycle(LifecycleAction::Restart, "api");
        assert!(
            restart.contains("rollout restart deploy/"),
            "got: {restart}"
        );
        assert!(restart.contains("--context 'prod-eks'"));
        // in-pod exec pins scope + uses `--` (no /bin/bash wrapper).
        let ex = rt.build_read_exec("pod-x", "cat /etc/hostname");
        assert!(
            ex.contains("--context 'prod-eks'") && ex.contains(" -- cat"),
            "got: {ex}"
        );
    }

    #[test]
    fn k5_every_kubectl_command_carries_explicit_context() {
        // Anti-footgun invariant: EVERY command the k8s runtime assembles
        // pins --context explicitly — none may fall through to the ambient
        // current-context (the #1 kubectl destruction class). Exhaustive over
        // all build methods, including the ones the seed test omitted.
        let rt = K8sRuntime {
            context: Some("prod-eks".to_string()),
            namespace: Some("payments".to_string()),
        };
        let mut cmds = vec![
            rt.inventory_cmd(),
            rt.build_read_exec("pod-x", "ls"),
            rt.build_write_exec("pod-x", "rm /tmp/x"),
        ];
        for action in [
            LifecycleAction::Restart,
            LifecycleAction::Reload,
            LifecycleAction::Stop,
            LifecycleAction::Start,
        ] {
            cmds.push(rt.build_lifecycle(action, "api"));
        }
        for c in &cmds {
            assert!(
                c.starts_with("kubectl") && c.contains("--context 'prod-eks'"),
                "k8s command must pin --context, got: {c}"
            );
        }
    }

    #[test]
    fn k5_context_pinning_holds_regardless_of_namespace() {
        // Even with no -n scope, --context is still pinned (the invariant is
        // about the cluster, not the namespace).
        let rt = K8sRuntime {
            context: Some("staging".to_string()),
            namespace: None,
        };
        assert!(rt.inventory_cmd().contains("--context 'staging'"));
        assert!(!rt.inventory_cmd().contains("-n ")); // no namespace scope
    }
}
