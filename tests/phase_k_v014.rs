//! Phase K (v0.1.4) — Kubernetes runtime medium acceptance tests.
//!
//! `inspect` is a **bin-only crate** (no `[lib]` target): the files in
//! `tests/` are black-box, driving the compiled binary via `assert_cmd`
//! rather than importing internal Rust APIs. K1 introduces the internal
//! `Runtime` executor abstraction (`src/exec/runtime.rs`) which has **no
//! user-facing surface yet** — the k8s verbs land in Waves B–D. Its
//! unit-level acceptance therefore lives in-module as
//! `#[cfg(test)] mod tests` in `src/exec/runtime.rs`, named to the K1
//! convention and run by `cargo test`:
//!
//!   * `k1_docker_runtime_parity_*` — the `DockerRuntime` builders emit
//!     byte-identical strings to the inline `docker …` sites (the
//!     behavior-preserving extraction; the whole docker suite is the
//!     broader regression gate).
//!   * `k1_runtime_selected_by_namespace_type` — selection routes a k8s
//!     `type` to `K8sRuntime`, docker/absent to `DockerRuntime`.
//!   * `k1_runtime_trait_object_safe` — `Box<dyn Runtime>` dispatch.
//!
//! This file holds the black-box K1 acceptance: introducing the runtime
//! seam must not change the binary's behavior. Later K-items add
//! black-box tests here as user-facing k8s verbs ship.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn inspect() -> Command {
    Command::cargo_bin("inspect").expect("inspect binary builds")
}

/// K1 additive-purity: the binary still builds and reports its version.
/// If the Runtime refactor broke the build or startup, this fails.
#[test]
fn k1_binary_still_builds_and_runs() {
    inspect().arg("--version").assert().success();
}

/// K1: no k8s user surface is exposed yet (the `type = "k8s"` namespace
/// field and every k8s verb land in K2+). The top-level help renders
/// without any half-wired k8s verb leaking in.
#[test]
fn k1_help_renders_without_k8s_half_verbs() {
    inspect()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("inspect").or(contains("Usage")));
}

// ---- K2 (v0.1.4): namespace `type` / kubeconfig config --------------

/// K2: `inspect show` on a k8s namespace renders the SSH-only fields as
/// `N/A (k8s)` (not `<unset>`, which would read as misconfigured) and
/// surfaces the k8s addressing. Driven from an env-only k8s namespace so
/// no on-disk config is needed.
#[test]
fn k2_show_renders_ssh_fields_na_for_k8s() {
    inspect()
        .env("INSPECT_STAGINGK8S_TYPE", "k8s")
        .env("INSPECT_STAGINGK8S_CONTEXT", "staging")
        .env("INSPECT_STAGINGK8S_NAMESPACE", "default")
        .args(["show", "stagingk8s"])
        .assert()
        .success()
        .stdout(contains("type:").and(contains("k8s")))
        .stdout(contains("N/A (k8s)"))
        .stdout(contains("staging"));
}

/// K2: an env-only k8s namespace with no host/user validates and shows
/// (the type-conditional validation drops the docker host+user gate). A
/// `context` is still required — the K5 anti-footgun invariant pins the
/// kubeconfig context explicitly, so it is part of a valid k8s config.
#[test]
fn k2_k8s_namespace_shows_without_host_user() {
    inspect()
        .env("INSPECT_KUBEONLY_TYPE", "kubernetes")
        .env("INSPECT_KUBEONLY_CONTEXT", "z2-maker")
        .args(["show", "kubeonly"])
        .assert()
        .success()
        .stdout(contains("type:").and(contains("k8s")));
}

/// K2: an unknown `type` is rejected loudly rather than silently run as
/// docker.
#[test]
fn k2_unknown_type_is_rejected() {
    inspect()
        .env("INSPECT_BADTYPE_TYPE", "kube")
        .args(["show", "badtype"])
        .assert()
        .failure()
        .stderr(contains("invalid runtime type").or(contains("kube")));
}

/// K2 (WA-2): the *negative* half of the type-conditional validate — a
/// `type = "docker"` namespace with no host/user must still FAIL, exactly
/// as before K2. The k8s branch drops the host+user gate; the docker
/// branch must keep it.
#[test]
fn k2_docker_namespace_still_requires_host_user() {
    inspect()
        .env("INSPECT_DOCKERBAD_TYPE", "docker")
        .args(["show", "dockerbad"])
        .assert()
        .failure()
        .stderr(contains("host").or(contains("user")));
}

/// K2 (WA-2): an absent `type` field resolves to the docker runtime — the
/// no-change guarantee for existing configs. A namespace with host/user
/// and no `type` renders as docker.
#[test]
fn k2_type_defaults_to_docker_when_absent() {
    inspect()
        .env("INSPECT_DOCKDEFAULT_HOST", "h.example.internal")
        .env("INSPECT_DOCKDEFAULT_USER", "u")
        .args(["show", "dockdefault"])
        .assert()
        .success()
        .stdout(contains("docker"));
}

/// K2 (WA-2): the servers.toml schema is bumped to 2 for the k8s fields.
/// Black-box acceptance: a freshly-written config records
/// `schema_version = 2`. (INSPECT_HOME isolates the write to a tempdir.)
#[test]
fn k2_schema_version_bumped() {
    let home = std::env::temp_dir().join(format!("inspect-k2schema-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    inspect()
        .env("INSPECT_HOME", &home)
        .args([
            "add",
            "k",
            "--type",
            "k8s",
            "--context",
            "c",
            "--non-interactive",
            "--force",
        ])
        .assert()
        .success();
    let contents =
        std::fs::read_to_string(home.join("servers.toml")).expect("servers.toml was written");
    assert!(
        contents.contains("schema_version = 2"),
        "expected `schema_version = 2` in written config, got:\n{contents}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

// ---- K3 (v0.1.4): kubectl backend probe + preflight ------------------
//
// WA-3 (JP-2026-07-05) splits read from enforce: `show <k8s-ns>` REPORTS
// kubectl readiness (probing the LOCAL kubectl — surface map §10, k8s
// transport is local, not SSH — but never hard-failing), while the ACTION
// verbs (`setup` / read / write) ENFORCE presence and fail loud with the
// four-question error when kubectl is absent. The pure
// version-floor + parse logic is exercised by the in-module unit tests in
// `src/exec/kubectl.rs` (`k3_kubectl_version_floor_enforced`,
// `k3_parse_*`, `k3_not_found_message_answers_four_questions`) — the crate
// is bin-only so black-box tests cannot import those APIs (same precedent
// as the in-module K1 tests). These three cover the user-facing surface.

/// K3: with kubectl on PATH, `show` on a k8s namespace detects it and
/// reports the client version on a `kubectl:` line (never `ABSENT`).
/// Skips when kubectl is not installed in the test environment.
#[test]
fn k3_kubectl_probe_detects_presence_and_version() {
    let kubectl_present = std::process::Command::new("kubectl")
        .args(["version", "--client"])
        .output()
        .is_ok();
    if !kubectl_present {
        eprintln!("skipping k3_kubectl_probe_detects_presence_and_version: kubectl not on PATH");
        return;
    }
    inspect()
        .env("INSPECT_K3PRESENT_TYPE", "k8s")
        .env("INSPECT_K3PRESENT_CONTEXT", "z2-maker")
        .args(["show", "k3present"])
        .assert()
        .success()
        .stdout(contains("kubectl:").and(contains("ABSENT").not()));
}

/// K3/WA-3: with kubectl NOT findable (empty PATH), a k8s ACTION verb
/// (`setup` — it must shell out to kubectl to discover) fails with the
/// four-question preflight error (what / where / why / fix) and exits 2 —
/// NOT a raw OS error, NOT a silent success. Enforcement lives in the
/// action verbs, not in `show` (which only REPORTS kubectl readiness per
/// the WA-3 read/enforce split).
#[test]
fn k3_k8s_verb_fails_loud_when_kubectl_absent() {
    inspect()
        .env("PATH", "/nonexistent-inspect-k3-probe")
        .env("INSPECT_K3ABSENT_TYPE", "k8s")
        .env("INSPECT_K3ABSENT_CONTEXT", "z2-maker")
        .args(["setup", "k3absent"])
        .assert()
        .failure()
        .code(2)
        .stderr(
            contains("kubectl not found")
                .and(contains("what:"))
                .and(contains("where:"))
                .and(contains("why:"))
                .and(contains("fix:")),
        );
}

/// K3: a docker namespace is unaffected by kubectl absence — it never
/// probes kubectl, so with an empty PATH `show` still succeeds and emits
/// no kubectl preflight error.
#[test]
fn k3_docker_namespace_unaffected_by_kubectl_absence() {
    inspect()
        .env("PATH", "/nonexistent-inspect-k3-probe")
        .env("INSPECT_K3DOCK_HOST", "h.example.internal")
        .env("INSPECT_K3DOCK_USER", "u")
        .args(["show", "k3dock"])
        .assert()
        .success()
        .stdout(contains("docker"))
        .stderr(contains("kubectl not found").not());
}

// ---- WA-1 (v0.1.4): resolved-config-path anti-mindtrap ---------------

/// WA-1: `inspect add` must report the RESOLVED servers.toml path, not a
/// hardcoded `~/.inspect/servers.toml`. When INSPECT_HOME relocates
/// config, the old message lied about where the write landed — an agent
/// following the reported path would find nothing there. The success
/// output must name the real path and must NOT contain the literal
/// `~/.inspect/servers.toml`.
#[test]
fn wa1_add_reports_resolved_config_path_under_inspect_home() {
    let home = std::env::temp_dir().join(format!("inspect-wa1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let servers = home.join("servers.toml");
    inspect()
        .env("INSPECT_HOME", &home)
        .args([
            "add",
            "foo",
            "--type",
            "k8s",
            "--context",
            "z2-maker",
            "--namespace",
            "inspect-livetest",
            "--non-interactive",
            "--force",
        ])
        .assert()
        .success()
        .stdout(
            contains(servers.display().to_string()).and(contains("~/.inspect/servers.toml").not()),
        );
    let _ = std::fs::remove_dir_all(&home);
}

/// K5 anti-footgun invariant (source scan): inspect must NEVER read or mutate
/// the ambient kubeconfig `current-context`. It pins `--context` explicitly on
/// every kubectl call instead (see `K8sRuntime::scope_flags`), so a switch in
/// some other shell can never redirect an inspect verb at the wrong cluster —
/// this is the structural immunity to the "context-pong" destruction class.
#[test]
fn k5_ambient_current_context_never_read_or_mutated() {
    fn scan(dir: &std::path::Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                scan(&p, hits);
            } else if p.extension().map(|e| e == "rs").unwrap_or(false) {
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                for line in src.lines() {
                    let l = line.trim_start();
                    if l.starts_with("//") || l.contains("k5_ambient") {
                        continue; // skip comments + this test's own name
                    }
                    // The forbidden forms are the kubectl subcommands that READ
                    // or SET the ambient current-context. Pinning `--context` is
                    // fine; `config current-context` / `config use-context` are not.
                    if l.contains("config current-context") || l.contains("config use-context") {
                        hits.push(format!("{}: {}", p.display(), line.trim()));
                    }
                }
            }
        }
    }
    let mut hits = Vec::new();
    scan(std::path::Path::new("src"), &mut hits);
    assert!(
        hits.is_empty(),
        "inspect must never read/mutate the ambient current-context (K5). Offenders:\n  {}",
        hits.join("\n  ")
    );
}

/// WA-3 (JP-2026-07-05): `inspect show <k8s-ns> --json` is a pure config read
/// — it must ALWAYS emit valid JSON and exit 0 even when kubectl is absent
/// (an agent doing `inspect show maker --json | jq .context` must never get a
/// non-JSON error). Enforcement of kubectl presence lives in the action verbs.
#[test]
fn wa3_show_json_contract_holds_without_kubectl() {
    use assert_cmd::Command;
    use std::io::Write;
    let home = std::env::temp_dir().join(format!("inspect-wa3-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&home);
    let servers = home.join("servers.toml");
    let mut f = std::fs::File::create(&servers).unwrap();
    writeln!(
        f,
        "schema_version = 2\n[namespaces.k]\ntype = \"k8s\"\ncontext = \"c\"\nnamespace = \"ns\""
    )
    .unwrap();
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&servers, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    // PATH without kubectl (guaranteed absent).
    let out = Command::cargo_bin("inspect")
        .unwrap()
        .env("INSPECT_HOME", &home)
        .env("PATH", "/nonexistent-dir")
        .args(["show", "k", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "show --json must exit 0 without kubectl"
    );
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("show --json must emit valid JSON");
    assert_eq!(v.get("context").and_then(|c| c.as_str()), Some("c"));
    assert_eq!(
        v.get("kubectl_available").and_then(|b| b.as_bool()),
        Some(false)
    );
    let _ = std::fs::remove_dir_all(&home);
}

// ---- K15–K19 (v0.1.4): k8s write verbs — acceptance ------------------
//
// The mutating verbs (scale / restart=rollout-restart / rollout undo /
// delete pod / exec --apply). The live apply+revert round-trips run in the
// release smoke (SMOKE_v0.1.4 P9, WD-1). These black-box tests pin the
// cluster-independent contract: the docker-namespace REFUSAL, the
// missing-target error, the exec `--no-revert` interlock, and the dry-run
// resolved-target echo (H3: the REAL {context, k8s_namespace, workload}).
// Dry-run tests that reach a verb's own `kubectl` capture call are guarded on
// kubectl being present (they never touch a cluster — the capture fails
// cleanly against the bogus context and the dry run still prints).

fn kubectl_present() -> bool {
    std::process::Command::new("kubectl")
        .args(["version", "--client"])
        .output()
        .is_ok()
}

/// K15: `scale` refuses a docker namespace with a chained pointer — a raw
/// runtime error must never reach the agent.
#[test]
fn k15_scale_refuses_docker_namespace() {
    inspect()
        .env("INSPECT_K15DOCK_HOST", "h.example.internal")
        .env("INSPECT_K15DOCK_USER", "u")
        .args(["scale", "k15dock/web", "--replicas", "2"])
        .assert()
        .failure()
        .stderr(contains("Kubernetes verb"));
}

/// K15: `scale` with no workload segment errors before any kubectl call.
#[test]
fn k15_scale_errors_on_missing_workload() {
    inspect()
        .env("INSPECT_K15NW_TYPE", "k8s")
        .env("INSPECT_K15NW_CONTEXT", "inspect-test-ctx")
        .args(["scale", "k15nw", "--replicas", "2"])
        .assert()
        .failure()
        .stderr(contains("specify a workload"));
}

/// K15: dry-run (no `--apply`) echoes the resolved {namespace, workload,
/// context} (H3) + the command, exits 0, and mutates nothing. The resolved
/// namespace comes from config (`effective_namespace` short-circuits, no
/// kubectl), so the echo is deterministic; the replica-capture `kubectl get`
/// fails cleanly against the bogus context and the dry run still prints.
#[test]
fn k15_scale_dry_run_echoes_resolved_target() {
    if !kubectl_present() {
        eprintln!("skip k15_scale_dry_run_echoes_resolved_target: kubectl absent");
        return;
    }
    inspect()
        .env("INSPECT_K15DR_TYPE", "k8s")
        .env("INSPECT_K15DR_CONTEXT", "inspect-test-ctx")
        .env("INSPECT_K15DR_NAMESPACE", "inspect-livetest")
        .args(["scale", "k15dr/web", "--replicas", "3"])
        .assert()
        .success()
        .stdout(
            contains("DRY RUN")
                .and(contains("inspect-livetest"))
                .and(contains("web"))
                .and(contains("inspect-test-ctx")),
        );
}

/// K17: `rollout` refuses a docker namespace.
#[test]
fn k17_rollout_refuses_docker_namespace() {
    inspect()
        .env("INSPECT_K17DOCK_HOST", "h.example.internal")
        .env("INSPECT_K17DOCK_USER", "u")
        .args(["rollout", "k17dock/web"])
        .assert()
        .failure()
        .stderr(contains("Kubernetes verb"));
}

/// K17: `rollout` with no workload errors before any kubectl call.
#[test]
fn k17_rollout_errors_on_missing_workload() {
    inspect()
        .env("INSPECT_K17NW_TYPE", "k8s")
        .env("INSPECT_K17NW_CONTEXT", "inspect-test-ctx")
        .args(["rollout", "k17nw"])
        .assert()
        .failure()
        .stderr(contains("specify a workload"));
}

/// K18: `delete` refuses a docker namespace with a pointer to `stop`/compose.
#[test]
fn k18_delete_refuses_docker_namespace() {
    inspect()
        .env("INSPECT_K18DOCK_HOST", "h.example.internal")
        .env("INSPECT_K18DOCK_USER", "u")
        .args(["delete", "k18dock/some-pod"])
        .assert()
        .failure()
        .stderr(contains("Kubernetes verb"));
}

/// K18: `delete` with no pod segment errors before any kubectl call.
#[test]
fn k18_delete_errors_on_missing_pod() {
    inspect()
        .env("INSPECT_K18NP_TYPE", "k8s")
        .env("INSPECT_K18NP_CONTEXT", "inspect-test-ctx")
        .args(["delete", "k18np"])
        .assert()
        .failure()
        .stderr(contains("specify a pod").or(contains("pod")));
}

/// K18: dry-run echoes the resolved target + exits 0 without deleting.
#[test]
fn k18_delete_dry_run_echoes_resolved_target() {
    if !kubectl_present() {
        eprintln!("skip k18_delete_dry_run_echoes_resolved_target: kubectl absent");
        return;
    }
    inspect()
        .env("INSPECT_K18DR_TYPE", "k8s")
        .env("INSPECT_K18DR_CONTEXT", "inspect-test-ctx")
        .env("INSPECT_K18DR_NAMESPACE", "inspect-livetest")
        .args(["delete", "k18dr/web-abc123"])
        .assert()
        .success()
        .stdout(
            contains("DRY RUN")
                .and(contains("inspect-livetest"))
                .and(contains("inspect-test-ctx")),
        );
}

/// K19: `exec --apply` on a pod requires `--no-revert` (in-pod fs mutation has
/// no synthesisable inverse) — the interlock fires before any kubectl call.
#[test]
fn k19_exec_apply_requires_no_revert() {
    inspect()
        .env("INSPECT_K19NR_TYPE", "k8s")
        .env("INSPECT_K19NR_CONTEXT", "inspect-test-ctx")
        .args([
            "exec",
            "k19nr/web-abc123",
            "--apply",
            "--",
            "rm",
            "-rf",
            "/tmp/x",
        ])
        .assert()
        .failure()
        .stderr(contains("--no-revert"));
}

/// K19: dry-run (no `--apply`) previews the exec + exits 0 without running it.
#[test]
fn k19_exec_dry_run_previews_without_running() {
    inspect()
        .env("INSPECT_K19DR_TYPE", "k8s")
        .env("INSPECT_K19DR_CONTEXT", "inspect-test-ctx")
        .env("INSPECT_K19DR_NAMESPACE", "inspect-livetest")
        .args(["exec", "k19dr/web-abc123", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout(contains("DRY RUN").and(contains("inspect-livetest")));
}

/// K16: `restart` on a k8s namespace is `kubectl rollout restart`. Its dry-run
/// echoes the resolved rollout-restart target (H3) and exits 0 without any
/// kubectl call (the revert is captured at --apply time), so it is fully
/// deterministic. `restart` serves both runtimes — this pins the k8s branch.
#[test]
fn k16_restart_k8s_dry_run_echoes_rollout_restart_target() {
    inspect()
        .env("INSPECT_K16DR_TYPE", "k8s")
        .env("INSPECT_K16DR_CONTEXT", "inspect-test-ctx")
        .env("INSPECT_K16DR_NAMESPACE", "inspect-livetest")
        .args(["restart", "k16dr/web"])
        .assert()
        .success()
        .stdout(
            contains("DRY RUN")
                .and(contains("rollout-restart"))
                .and(contains("inspect-livetest"))
                .and(contains("web")),
        );
}

// ---- K11–K13 (v0.1.4): k8s-only read verbs — contract edges ----------
//
// describe / events / top are Kubernetes-only reads: they REFUSE a docker
// namespace with a chained pointer (never a raw runtime error) and error on a
// missing target before any kubectl call. Their happy-path output parsing +
// envelope shape is exercised against a live cluster in the release smoke
// (reads are non-destructive); `describe`'s secret scrub is unit-tested in
// `src/verbs/describe.rs` (k11_scrub_*). These pin the cluster-independent
// contract edges so the read surface is not wholly untested (audit G2).

/// K11: `describe` refuses a docker namespace.
#[test]
fn k11_describe_refuses_docker_namespace() {
    inspect()
        .env("INSPECT_K11DOCK_HOST", "h.example.internal")
        .env("INSPECT_K11DOCK_USER", "u")
        .args(["describe", "k11dock/some-pod"])
        .assert()
        .failure()
        .stderr(contains("Kubernetes verb"));
}

/// K11: `describe` on a k8s namespace with no pod errors before any kubectl.
#[test]
fn k11_describe_errors_on_missing_pod() {
    inspect()
        .env("INSPECT_K11NP_TYPE", "k8s")
        .env("INSPECT_K11NP_CONTEXT", "inspect-test-ctx")
        .args(["describe", "k11np"])
        .assert()
        .failure()
        .stderr(contains("specify a pod"));
}

/// K12: `events` refuses a docker namespace.
#[test]
fn k12_events_refuses_docker_namespace() {
    inspect()
        .env("INSPECT_K12DOCK_HOST", "h.example.internal")
        .env("INSPECT_K12DOCK_USER", "u")
        .args(["events", "k12dock"])
        .assert()
        .failure()
        .stderr(contains("Kubernetes verb"));
}

/// K13: `top` refuses a docker namespace with a pointer to status/health.
#[test]
fn k13_top_refuses_docker_namespace() {
    inspect()
        .env("INSPECT_K13DOCK_HOST", "h.example.internal")
        .env("INSPECT_K13DOCK_USER", "u")
        .args(["top", "k13dock"])
        .assert()
        .failure()
        .stderr(contains("Kubernetes verb"));
}
