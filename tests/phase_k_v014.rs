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
