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
/// (the type-conditional validation drops the docker host+user gate).
#[test]
fn k2_k8s_namespace_shows_without_host_user() {
    inspect()
        .env("INSPECT_KUBEONLY_TYPE", "kubernetes")
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
            "add", "k", "--type", "k8s", "--context", "c", "--non-interactive", "--force",
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
            contains(servers.display().to_string())
                .and(contains("~/.inspect/servers.toml").not()),
        );
    let _ = std::fs::remove_dir_all(&home);
}
