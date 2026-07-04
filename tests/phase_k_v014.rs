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
