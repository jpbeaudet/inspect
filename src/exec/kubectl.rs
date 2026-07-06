//! Local `kubectl` backend probe + preflight.
//!
//! The kubectl shell-out backend is only as reliable as the
//! `kubectl` binary being present on the **local** machine's PATH. k8s
//! namespaces run kubectl locally against the kubeconfig context —
//! surface map §10: the k8s transport is local, not SSH — so this probe
//! is a plain local process spawn. It is deliberately NOT routed through
//! the SSH/remote path that the docker tool-probe
//! ([`crate::discovery::probes::probe_remote_tooling`]) uses: kubectl
//! lives on this box, not on a remote host.
//!
//! The most-cited k8s-tool operational failure (k9s) is a raw
//! `executable file not found` when kubectl is absent. inspect must
//! instead fail **loud, specific, actionable** per the bible
//! CI-gate-quality rule — the four-question error ([`not_found_message`]):
//! WHAT was checked, WHERE it looked, WHY it matters, and the FIX.

use std::process::Command;

/// Documented minimum kubectl version floor. Below this, inspect **warns**
/// (it does not fail): every k8s operation inspect drives — `get -o json`,
/// `rollout restart` (kubectl ≥1.15), `scale`, `auth can-i` — is supported
/// well below this floor, so an older kubectl is a "you are on something
/// ancient, expect rough edges" nudge, not a hard gate. Enforced as a
/// warning by [`floor_warning`]. (Warn-not-fail is deliberate:
/// "pick warn unless the plan says fail, and document it.")
pub const KUBECTL_MIN_MAJOR: u32 = 1;
/// See [`KUBECTL_MIN_MAJOR`].
pub const KUBECTL_MIN_MINOR: u32 = 19;

/// Wall-clock cap on a single k8s READ round-trip (`get`/`top`/`events`/
/// `describe`/discovery). One documented home for the flag instead of the
/// literal scattered across every read verb (O2). The `test` API-reachability
/// preflight uses its own shorter probe timeout — a distinct concern, kept
/// separate on purpose.
pub const READ_REQUEST_TIMEOUT: &str = "--request-timeout=10s";

/// Outcome of the local kubectl probe.
#[derive(Debug, Clone)]
pub struct KubectlProbe {
    /// Whether a `kubectl` binary was found + spawnable on PATH.
    pub available: bool,
    /// Parsed client `gitVersion` (e.g. `"v1.29.3"`) when detectable.
    /// `None` when kubectl is absent or its version output is unparseable.
    pub version: Option<String>,
    /// Parsed `(major, minor)` when detectable — drives the floor check.
    pub semver: Option<(u32, u32)>,
    /// The PATH value the OS searched, captured for the four-question
    /// "where" line so an absent-kubectl error is self-diagnosing.
    pub path_searched: String,
}

impl KubectlProbe {
    /// The below-floor warning line, when the version is known and older
    /// than the documented floor. `None` when the version is unknown or
    /// at/above the floor.
    pub fn floor_warning(&self) -> Option<String> {
        floor_warning(self.semver)
    }
}

/// Run the local kubectl probe: presence + client version.
///
/// A local spawn — never over SSH. `--client` avoids a live API round-trip
/// (we probe the binary, not a cluster); `-o json` gives a stable,
/// parseable shape across modern kubectl. A spawn error (binary not found,
/// or not executable) is reported as `available = false` so the caller can
/// raise the four-question preflight error rather than leaking a raw OS
/// error to the agent.
pub fn probe_kubectl() -> KubectlProbe {
    let path_searched = std::env::var("PATH").unwrap_or_default();
    match Command::new("kubectl")
        .args(["version", "--client", "-o", "json"])
        .output()
    {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let version = parse_git_version(&stdout);
            let semver = version.as_deref().and_then(parse_semver);
            KubectlProbe {
                available: true,
                version,
                semver,
                path_searched,
            }
        }
        Err(_) => KubectlProbe {
            available: false,
            version: None,
            semver: None,
            path_searched,
        },
    }
}

/// Pull `clientVersion.gitVersion` out of `kubectl version --client -o
/// json` output. A manual scan rather than a serde_json round-trip —
/// one field, no need to model kubectl's whole version document.
pub fn parse_git_version(json: &str) -> Option<String> {
    let key = "\"gitVersion\"";
    let idx = json.find(key)?;
    let after_key = &json[idx + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = &after_key[colon + 1..];
    let q1 = after_colon.find('"')?;
    let after_q1 = &after_colon[q1 + 1..];
    let q2 = after_q1.find('"')?;
    let v = after_q1[..q2].trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// Parse `(major, minor)` from a kubectl `gitVersion` such as `"v1.29.3"`
/// or a vendor build like `"v1.27.9-eks-1234"`. Tolerates a leading `v`
/// and a non-numeric suffix on the minor component (`"29+"`, `"27-eks…"`).
pub fn parse_semver(gitversion: &str) -> Option<(u32, u32)> {
    let s = gitversion.trim().trim_start_matches('v');
    let mut it = s.split('.');
    let major = it.next()?.parse::<u32>().ok()?;
    let minor_raw = it.next()?;
    let minor_digits: String = minor_raw
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let minor = minor_digits.parse::<u32>().ok()?;
    Some((major, minor))
}

/// Whether a `(major, minor)` meets the documented floor.
pub fn version_meets_floor(semver: (u32, u32)) -> bool {
    semver >= (KUBECTL_MIN_MAJOR, KUBECTL_MIN_MINOR)
}

/// The below-floor warning line — `Some` only when the version is known
/// and older than the floor. Names both the detected version and the
/// floor so the operator knows exactly how far behind they are.
pub fn floor_warning(semver: Option<(u32, u32)>) -> Option<String> {
    match semver {
        Some(v) if !version_meets_floor(v) => Some(format!(
            "kubectl {}.{} is below the recommended floor v{}.{}; k8s verbs may \
             behave unexpectedly — upgrade kubectl when you can",
            v.0, v.1, KUBECTL_MIN_MAJOR, KUBECTL_MIN_MINOR
        )),
        _ => None,
    }
}

/// The four-question (what / where / why / fix) preflight error raised
/// when a k8s namespace's verb runs but `kubectl` is absent. Loud,
/// specific, and actionable per the bible CI-gate-quality rule — never a
/// raw OS `executable file not found`.
///
/// Note: this deliberately does not append a `see: inspect help
/// kubernetes` pointer — the `fix:` line already carries the actionable
/// install pointer directly, so the error is self-contained.
pub fn not_found_message(namespace: &str, path_searched: &str) -> String {
    let path = if path_searched.trim().is_empty() {
        "(empty PATH)"
    } else {
        path_searched
    };
    format!(
        "kubectl not found on PATH — k8s namespace '{ns}' cannot run without it\n  \
         what:  the `kubectl` binary was not found; k8s namespaces use the kubectl \
         shell-out backend\n  \
         where: searched PATH = {path}\n  \
         why:   inspect drives Kubernetes namespaces (type = \"k8s\") by shelling out \
         to kubectl locally; no k8s verb can run without it\n  \
         fix:   install kubectl (https://kubernetes.io/docs/tasks/tools/) and ensure \
         it is on PATH, then re-run — verify with `kubectl version --client`",
        ns = namespace,
        path = path,
    )
}

// ---------------------------------------------------------------------------
// k8s failure-class taxonomy + stderr classifier.
//
// kubectl collapses NotFound / Forbidden / Unreachable / metrics-absent all
// into a single non-zero exit with the distinction only in stderr *prose*.
// inspect's job is to parse that prose into a stable, agent-branchable
// `failure_class` string + a CI-gate-quality hint, so an agent never has to
// scrape kubectl's English. The exit code stays the coarse class (transport
// reuses the 12–14 band by semantic class); `failure_class` is the fine
// detail. Fixtures in the test module are REAL strings harvested from the
// live maker/hub clusters (the harvest commands are recorded there).
// ---------------------------------------------------------------------------

/// A classified k8s failure. The `failure_class()` string is the stable
/// agent-branch discriminator; `exit_code()` is the coarse band. See
/// `exit_code()` for the full documented exit-code table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KubectlFailure {
    /// RBAC denied the action (API reachable, authenticated, but not
    /// authorized). kubectl: `Error from server (Forbidden): ...`.
    RbacForbidden,
    /// The addressed object does not exist. kubectl: `(NotFound)`.
    NotFound,
    /// exec landed in a container with no shell / coreutils (distroless).
    NoShellInContainer,
    /// `kubectl top` with no metrics-server (or it is still warming up).
    MetricsUnavailable,
    /// The API server is unreachable (DNS / route / refused / bad context).
    TransportUnreachable,
    /// Token / client-cert expired or auth rejected at the API.
    TransportAuthExpired,
    /// Unrecognized stderr — never misclassified into a wrong branch.
    Unknown,
}

impl KubectlFailure {
    /// The stable `failure_class` JSON/SUMMARY string. Transport classes
    /// reuse the `TransportClass` strings so the value space is uniform
    /// across mediums.
    pub fn failure_class(self) -> &'static str {
        match self {
            KubectlFailure::RbacForbidden => "rbac_forbidden",
            KubectlFailure::NotFound => "not_found",
            KubectlFailure::NoShellInContainer => "no_shell_in_container",
            KubectlFailure::MetricsUnavailable => "metrics_unavailable",
            KubectlFailure::TransportUnreachable => "transport_unreachable",
            KubectlFailure::TransportAuthExpired => "transport_auth_failed",
            KubectlFailure::Unknown => "unknown",
        }
    }

    /// The coarse exit code an agent branches on (JP-2026-07-05). Two
    /// documented bands adjacent to each other; `failure_class()` carries the
    /// fine detail. **Transport band** (parallel to the SSH band 12–14):
    /// `transport_unreachable` → 13, `transport_auth_failed` → 14,
    /// `rbac_forbidden` → 14 (an authorization failure; `failure_class`
    /// distinguishes it from a credential expiry). **Operational-degradation
    /// band** (15–16, JP-assigned — distinct consumer remediations, so distinct
    /// codes rather than coarse exit-1): `metrics_unavailable` → 15
    /// (skip-metrics path), `no_shell_in_container` → 16 (skip-exec path).
    /// `not_found` / `unknown` → 1 (no-match, inspect's existing semantics).
    pub fn exit_code(self) -> u8 {
        match self {
            KubectlFailure::TransportUnreachable => 13,
            KubectlFailure::TransportAuthExpired | KubectlFailure::RbacForbidden => 14,
            KubectlFailure::MetricsUnavailable => 15,
            KubectlFailure::NoShellInContainer => 16,
            KubectlFailure::NotFound | KubectlFailure::Unknown => 1,
        }
    }

    /// The chained, CI-gate-quality hint (what went wrong + the next action).
    /// `resource_hint` is a short `<verb> <resource> -n <ns>` fragment the
    /// caller can supply so the RBAC hint embeds the literal `auth can-i`.
    pub fn hint(self, resource_hint: &str) -> String {
        match self {
            KubectlFailure::RbacForbidden => format!(
                "RBAC denied this action. Check the exact grant with \
                 `kubectl auth can-i {rh}` (add `--as <user>` to test another \
                 identity); ask your cluster admin for the missing Role/Binding.",
                rh = if resource_hint.is_empty() {
                    "<verb> <resource>"
                } else {
                    resource_hint
                }
            ),
            KubectlFailure::NotFound => {
                "the addressed object does not exist in this namespace/context — \
                 check the name and `-n`/`--namespace`."
                    .into()
            }
            KubectlFailure::NoShellInContainer => {
                "this container has no shell / coreutils (distroless or minimal image); \
                 in-pod exec cannot run. Use an ephemeral debug container \
                 (`kubectl debug`) — a first-class debug verb is planned for v0.1.5+."
                    .into()
            }
            KubectlFailure::MetricsUnavailable => {
                "metrics are unavailable — metrics-server is not installed (or still \
                 warming up, ~60s after install). Install metrics-server, or retry \
                 shortly; this is a cluster-component gap, not a workload failure."
                    .into()
            }
            KubectlFailure::TransportUnreachable => {
                "the Kubernetes API server is unreachable — check the context, the \
                 kubeconfig, and network reachability to the cluster."
                    .into()
            }
            KubectlFailure::TransportAuthExpired => {
                "authentication to the API server failed — your token or client \
                 certificate may be expired; refresh your kubeconfig credentials \
                 (`kubectl` handles re-auth; inspect does not cache k8s credentials)."
                    .into()
            }
            KubectlFailure::Unknown => {
                "kubectl returned an error inspect did not recognize — see the raw \
                 stderr below and `kubectl` docs."
                    .into()
            }
        }
    }
}

/// Result of running a command inside a pod via `kubectl exec`. On
/// failure, `failure` carries the classified class (which yields both the
/// exit code via `exit_code()` and the hint) — so the raw stderr/exit are not
/// re-exposed here.
pub struct K8sExecOut {
    pub stdout: String,
    /// `Some` when the exec failed — the classified failure, so the
    /// caller can branch (e.g. `no_shell_in_container` → exit 16).
    pub failure: Option<KubectlFailure>,
}

/// The `kubectl … ` prefix STRING for a revert `command_pair` payload —
/// includes `--context`, `--kubeconfig` (expanded), and an explicit, resolved
/// `-n <ns>` (from [`effective_namespace`]), so the payload is fully
/// self-contained and reverts in the SAME namespace the mutation targeted
/// (omitting `--kubeconfig` broke reverts on a non-default kubeconfig; pinning
/// the resolved namespace ensures a revert never lands in a different namespace
/// than the apply). Values are
/// config-controlled identifiers/paths.
pub fn revert_kubectl_prefix_in(
    cfg: &crate::config::namespace::NamespaceConfig,
    ns: &str,
) -> String {
    let mut s = String::from("kubectl");
    if let Some(ctx) = cfg.context.as_deref() {
        s.push_str(&format!(" --context {ctx}"));
    }
    if let Some(kc) = cfg.kubeconfig.as_deref() {
        s.push_str(&format!(" --kubeconfig {}", expand_tilde_kc(kc)));
    }
    s.push_str(&format!(" -n {ns}"));
    s
}

/// A context-pinned `kubectl` base command (`--context` always explicit,
/// never the ambient current-context) with `--kubeconfig` + `-n` from config.
/// The caller appends the subcommand (`get` / `describe` / `top` / `events`).
/// Reused by the read verbs (`why`, `describe`, `events`, `top`, and more). When
/// `k8s_namespace` is unset the `-n` flag is omitted and kubectl uses the
/// context's default namespace — fine for reads; write verbs instead resolve
/// and pin the namespace explicitly (see [`kubectl_base_in`]).
pub fn kubectl_base(cfg: &crate::config::namespace::NamespaceConfig) -> Command {
    let ns = cfg
        .k8s_namespace
        .as_deref()
        .filter(|n| !n.trim().is_empty());
    kubectl_base_impl(cfg, ns)
}

/// Like [`kubectl_base`] but pins an explicit, already-resolved `-n <ns>` (from
/// [`effective_namespace`]) regardless of `cfg.k8s_namespace`. The k8s **write**
/// verbs use this so the mutating command targets exactly the namespace named
/// in the dry-run preview, the confirm prompt, and the `AuditEntry` —
/// never an implicit context-default the audit can't see.
pub fn kubectl_base_in(cfg: &crate::config::namespace::NamespaceConfig, ns: &str) -> Command {
    kubectl_base_impl(cfg, Some(ns))
}

fn kubectl_base_impl(cfg: &crate::config::namespace::NamespaceConfig, ns: Option<&str>) -> Command {
    let mut c = Command::new("kubectl");
    if let Some(ctx) = cfg.context.as_deref() {
        c.args(["--context", ctx]);
    }
    if let Some(kc) = cfg.kubeconfig.as_deref() {
        c.args(["--kubeconfig", &expand_tilde_kc(kc)]);
    }
    if let Some(n) = ns {
        c.args(["-n", n]);
    }
    c
}

/// Chained failure hint for a **Deployment-scoped** write verb (scale /
/// rollout / restart). On `NotFound` it names the deploy-only conservative
/// write set + the kubectl escape hatch, so a StatefulSet/DaemonSet of the
/// same name does not fail as an opaque `not_found` (the refuse-with-idiom
/// applied to a wrong-kind target). Every other class defers to the
/// standard [`KubectlFailure::hint`]. `delete`/`exec` target pods, not
/// Deployments, so they keep the plain hint.
pub fn deploy_write_hint(f: KubectlFailure, workload: &str, ns: &str, context: &str) -> String {
    match f {
        KubectlFailure::NotFound => format!(
            "no Deployment '{workload}' in namespace '{ns}' on context '{context}'. \
             inspect's k8s write verbs (scale / restart / rollout) target Deployments — \
             the v0.1.4 conservative write set. If '{workload}' is a StatefulSet or \
             DaemonSet, act on it with kubectl directly, e.g. \
             `kubectl -n {ns} --context {context} scale statefulset/{workload} --replicas N`."
        ),
        _ => f.hint(""),
    }
}

/// Resolve the k8s namespace a verb will ACTUALLY act in, for an honest
/// echo / audit / confirm and for explicit `-n` pinning. Config
/// `k8s_namespace` wins; otherwise the default namespace the pinned kubeconfig
/// **context** sets (read locally + offline via `kubectl config view --minify`
/// — no API round-trip, so it works in dry-run and against an unreachable
/// cluster); otherwise the literal `"default"` (kubectl's own final fallback).
///
/// The write verbs previously echoed/audited
/// `unwrap_or("default")`, so a context whose default namespace was e.g.
/// `production` ran there while the confirm prompt + `AuditEntry` said
/// `default` — a false forensic record on a destructive op. Resolving + pinning
/// the real value also extends the anti-footgun from context to namespace:
/// the mutation never rides an implicit context-default. `config view` is
/// read-only and does not touch `current-context`.
pub fn effective_namespace(cfg: &crate::config::namespace::NamespaceConfig) -> String {
    if let Some(n) = cfg.k8s_namespace.as_deref() {
        if !n.trim().is_empty() {
            return n.to_string();
        }
    }
    let out = kubectl_base_impl(cfg, None)
        .args([
            "config",
            "view",
            "--minify",
            "-o",
            "jsonpath={.contexts[0].context.namespace}",
        ])
        .output();
    if let Ok(out) = out {
        if out.status.success() {
            let ns = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !ns.is_empty() {
                return ns;
            }
        }
    }
    "default".to_string()
}

/// Build the context-pinned `kubectl exec <pod> [-c <c>] --` prefix (the
/// context is always explicit, never the ambient current-context). The caller
/// appends the in-pod argv. No `/bin/sh` wrapper — the command runs directly,
/// avoiding the k9s Alpine-no-bash trap.
pub fn exec_base(
    cfg: &crate::config::namespace::NamespaceConfig,
    pod: &str,
    container: Option<&str>,
) -> Command {
    // Reuse `kubectl_base` for the context/kubeconfig/-n prefix so the
    // blank-`k8s_namespace` filter and the context pinning apply
    // uniformly — then append the exec framing.
    let mut c = kubectl_base(cfg);
    c.arg("exec").arg(pod);
    if let Some(cn) = container {
        c.args(["-c", cn]);
    }
    c.arg("--");
    c
}

/// Run `argv` inside a pod and return a classified result. Used by
/// `cat`/`ls`/`grep`/`run` for their in-pod read commands.
pub fn exec_in_pod(
    cfg: &crate::config::namespace::NamespaceConfig,
    pod: &str,
    container: Option<&str>,
    argv: &[&str],
) -> std::io::Result<K8sExecOut> {
    let mut c = exec_base(cfg, pod, container);
    c.args(argv);
    let out = c.output()?;
    let failure = if out.status.success() {
        None
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Some(classify_kubectl_failure(
            &stderr,
            out.status.code().unwrap_or(1),
        ))
    };
    Ok(K8sExecOut {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        failure,
    })
}

fn expand_tilde_kc(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = crate::paths::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// Classify a kubectl failure from its stderr + exit code into a stable
/// [`KubectlFailure`]. Ordering matters: the most specific markers are tested
/// before the generic ones so, e.g., a Forbidden is never swallowed by a
/// broad "unable to connect" fallback.
pub fn classify_kubectl_failure(stderr: &str, _exit_code: i32) -> KubectlFailure {
    let s = stderr.to_ascii_lowercase();
    // RBAC — "(Forbidden)" / "is forbidden" / "forbidden:".
    if s.contains("(forbidden)") || s.contains("is forbidden") || s.contains("forbidden:") {
        return KubectlFailure::RbacForbidden;
    }
    // Distroless / no-shell exec — the OCI exec error shapes. Before NotFound
    // because its "no such file or directory" contains no "not found", but its
    // exec markers must win over a generic fallback.
    if (s.contains("exec") || s.contains("oci runtime"))
        && (s.contains("no such file or directory")
            || s.contains("executable file not found")
            || s.contains("/bin/sh")
            || s.contains("/bin/bash"))
    {
        return KubectlFailure::NoShellInContainer;
    }
    // Metrics-server absent.
    if s.contains("metrics api not available")
        || s.contains("metrics.k8s.io")
        || (s.contains("metrics") && s.contains("not available"))
    {
        return KubectlFailure::MetricsUnavailable;
    }
    // Auth-expired at the API (distinct from RBAC-forbidden above).
    if s.contains("unauthorized")
        || (s.contains("certificate") && (s.contains("expired") || s.contains("invalid")))
        || s.contains("token is expired")
        || s.contains("must be logged in")
    {
        return KubectlFailure::TransportAuthExpired;
    }
    // API server unreachable — connection + bad-context shapes. MUST be tested
    // BEFORE NotFound: kubectl's bad-context error is "context was not found",
    // whose "not found" would otherwise be misread as an object NotFound.
    if s.contains("unable to connect to the server")
        || s.contains("the connection to the server")   // "... was refused ..."
        || s.contains("was refused")
        || s.contains("connection refused")
        || s.contains("no route to host")
        || s.contains("i/o timeout")
        || s.contains("context was not found")
        || (s.contains("context") && s.contains("does not exist")) // real kubectl bad-context
        || s.contains("error in configuration")
        || s.contains("dial tcp")
    {
        return KubectlFailure::TransportUnreachable;
    }
    // Object NotFound — the server-side "(NotFound)" shape. Guarded so a stray
    // "not found" in some other message does not misclassify (config/context
    // "not found" is already handled above as unreachable).
    if s.contains("(notfound)") || (s.contains("not found") && s.contains("from server")) {
        return KubectlFailure::NotFound;
    }
    KubectlFailure::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    // Acceptance — REAL kubectl stderr harvested from the live clusters
    // (maker/hub) on 2026-07-04, so the classifier is validated against
    // genuine output, not invented strings (no-synthetic-verification rule):
    //   notfound   : kubectl get pod does-not-exist -n inspect-livetest
    //   forbidden  : kubectl get secrets -n kube-system \
    //                  --as=system:serviceaccount:default:default
    //   unreachable: kubectl --context no-such-ctx get pods
    // `effective_namespace` prefers an explicit config `k8s_namespace`
    // and returns it WITHOUT shelling out (the early return before the
    // `kubectl config view` fallback). This is the deterministic branch — the
    // context-default query branch needs a live kubeconfig and is covered by
    // the k8s write acceptance tests. Pins that the write verbs' echo/audit
    // resolve the real namespace rather than a fabricated "default".
    #[test]
    fn h3_effective_namespace_prefers_explicit_config() {
        let cfg = crate::config::namespace::NamespaceConfig {
            context: Some("z2-maker".into()),
            k8s_namespace: Some("production".into()),
            ..Default::default()
        };
        assert_eq!(effective_namespace(&cfg), "production");
    }

    // A blank `k8s_namespace` is treated as unset (falls through to the
    // context-default resolver), never echoed as the literal empty string.
    #[test]
    fn h3_effective_namespace_treats_blank_config_as_unset() {
        let cfg = crate::config::namespace::NamespaceConfig {
            context: Some("z2-maker".into()),
            k8s_namespace: Some("   ".into()),
            ..Default::default()
        };
        // With no reachable kubeconfig context in the unit-test env, the
        // resolver's final fallback is kubectl's own default — never "   ".
        let ns = effective_namespace(&cfg);
        assert!(!ns.trim().is_empty());
        assert_ne!(ns, "   ");
    }

    // A Deployment-scoped write verb turns a wrong-kind `not_found` into a
    // chained hint that names the deploy-only conservative write set + the
    // kubectl escape hatch, instead of an opaque "does not exist".
    #[test]
    fn r6_deploy_write_hint_notfound_names_scope_and_escape_hatch() {
        let h = deploy_write_hint(KubectlFailure::NotFound, "my-sts", "prod", "z2-maker");
        assert!(h.contains("Deployment"), "names the deploy scope: {h}");
        assert!(
            h.contains("StatefulSet") && h.contains("DaemonSet"),
            "names other kinds: {h}"
        );
        assert!(
            h.contains("kubectl -n prod"),
            "gives the kubectl escape hatch: {h}"
        );
        assert!(h.contains("my-sts"), "names the workload: {h}");
    }

    // Non-NotFound classes defer to the standard hint (no deploy-scope noise).
    #[test]
    fn r6_deploy_write_hint_other_class_uses_standard_hint() {
        let h = deploy_write_hint(KubectlFailure::RbacForbidden, "web", "prod", "z2-maker");
        assert!(h.contains("RBAC"), "RBAC class keeps its own hint: {h}");
        assert!(!h.contains("conservative write set"));
    }

    #[test]
    fn k4_classify_forbidden_to_rbac_with_cani_hint() {
        let real = "Error from server (Forbidden): secrets is forbidden: User \
            \"system:serviceaccount:default:default\" cannot list resource \
            \"secrets\" in API group \"\" in the namespace \"kube-system\"";
        let c = classify_kubectl_failure(real, 1);
        assert_eq!(c, KubectlFailure::RbacForbidden);
        assert_eq!(c.failure_class(), "rbac_forbidden");
        assert!(c.hint("list secrets -n kube-system").contains("auth can-i"));
    }

    #[test]
    fn wa4_exit_code_bands() {
        // Transport band (parallel to the SSH 12-14 band).
        assert_eq!(KubectlFailure::TransportUnreachable.exit_code(), 13);
        assert_eq!(KubectlFailure::TransportAuthExpired.exit_code(), 14);
        assert_eq!(KubectlFailure::RbacForbidden.exit_code(), 14);
        // Operational-degradation band (15-16, JP-assigned distinct codes).
        assert_eq!(KubectlFailure::MetricsUnavailable.exit_code(), 15);
        assert_eq!(KubectlFailure::NoShellInContainer.exit_code(), 16);
        // No-match / unknown.
        assert_eq!(KubectlFailure::NotFound.exit_code(), 1);
        assert_eq!(KubectlFailure::Unknown.exit_code(), 1);
    }

    #[test]
    fn k4_classify_notfound() {
        let real = "Error from server (NotFound): pods \"does-not-exist\" not found";
        let c = classify_kubectl_failure(real, 1);
        assert_eq!(c, KubectlFailure::NotFound);
        assert_eq!(c.failure_class(), "not_found");
    }

    #[test]
    fn k4_classify_no_shell_in_container() {
        let real = "OCI runtime exec failed: exec failed: unable to start container \
            process: exec: \"/bin/sh\": stat /bin/sh: no such file or directory";
        let c = classify_kubectl_failure(real, 126);
        assert_eq!(c, KubectlFailure::NoShellInContainer);
        assert_eq!(c.failure_class(), "no_shell_in_container");
        assert!(c.hint("").contains("debug"));
    }

    #[test]
    fn k4_classify_metrics_unavailable() {
        let real = "error: Metrics API not available";
        let c = classify_kubectl_failure(real, 1);
        assert_eq!(c, KubectlFailure::MetricsUnavailable);
        assert_eq!(c.failure_class(), "metrics_unavailable");
    }

    #[test]
    fn k4_classify_transport_unreachable() {
        // REAL string from `kubectl --context no-such-ctx version` against the
        // live maker kubeconfig (2026-07-04) — the live test caught that the
        // message is `context "X" does not exist`, not the config-error shape
        // originally assumed.
        let real = "error: context \"no-such-ctx\" does not exist";
        let c = classify_kubectl_failure(real, 1);
        assert_eq!(c, KubectlFailure::TransportUnreachable);
        assert_eq!(c.failure_class(), "transport_unreachable");
        // The older config-error shape also classifies (kept for coverage).
        let cfg_err = "Error in configuration: context was not found for \
            specified context: no-such-ctx";
        assert_eq!(
            classify_kubectl_failure(cfg_err, 1),
            KubectlFailure::TransportUnreachable
        );
    }

    #[test]
    fn k4_classify_transport_unreachable_connection_refused() {
        let real = "The connection to the server 127.0.0.1:1 was refused - \
            did you specify the right host or port?";
        // "connection refused" lower-cases into the unreachable marker set.
        let c = classify_kubectl_failure(real, 1);
        assert_eq!(c, KubectlFailure::TransportUnreachable);
    }

    #[test]
    fn k4_unknown_stderr_falls_back_safely() {
        // An unrecognized error must NOT be misclassified into a wrong branch.
        let c = classify_kubectl_failure("some brand new kubectl error nobody has seen", 1);
        assert_eq!(c, KubectlFailure::Unknown);
        assert_eq!(c.failure_class(), "unknown");
    }

    // Acceptance (unit-level). The crate is bin-only (no `[lib]`),
    // so the `tests/phase_k_v014.rs` integration file is black-box and
    // cannot import these internal APIs — the pure version-floor and
    // parse logic is exercised here (same precedent as the in-module
    // tests in `src/exec/runtime.rs`). The presence/absent/docker-unaffected
    // acceptance is black-box in `tests/phase_k_v014.rs`.

    #[test]
    fn k3_kubectl_version_floor_enforced() {
        // At/above the floor → no warning.
        assert!(version_meets_floor((1, 19)));
        assert!(version_meets_floor((1, 30)));
        assert!(version_meets_floor((2, 0)));
        assert!(floor_warning(Some((1, 30))).is_none());
        // Below the floor → a warning naming both the detected version
        // and the floor (CI-gate-quality: what + the target).
        assert!(!version_meets_floor((1, 11)));
        let w = floor_warning(Some((1, 11))).expect("below-floor must warn");
        assert!(
            w.contains("1.11") && w.contains("1.19"),
            "warning must name detected + floor versions, got: {w}"
        );
        // Unknown version → no warning (never assert a floor we can't read).
        assert!(floor_warning(None).is_none());
    }

    #[test]
    fn k3_parse_git_version_from_kubectl_json() {
        let json = r#"{"clientVersion":{"major":"1","minor":"29","gitVersion":"v1.29.3","gitCommit":"abc"}}"#;
        assert_eq!(parse_git_version(json).as_deref(), Some("v1.29.3"));
        // Absent field → None (present-but-unparseable is a valid state).
        assert_eq!(parse_git_version("{}"), None);
        assert_eq!(parse_git_version("not json at all"), None);
    }

    #[test]
    fn k3_parse_semver_tolerates_vendor_suffixes() {
        assert_eq!(parse_semver("v1.29.3"), Some((1, 29)));
        assert_eq!(parse_semver("1.27.9-eks-1234"), Some((1, 27)));
        assert_eq!(parse_semver("v1.30+"), Some((1, 30)));
        assert_eq!(parse_semver("garbage"), None);
    }

    #[test]
    fn k3_not_found_message_answers_four_questions() {
        let m = not_found_message("maker", "/usr/bin:/bin");
        for needle in ["what:", "where:", "why:", "fix:"] {
            assert!(
                m.contains(needle),
                "four-question message missing {needle:?}: {m}"
            );
        }
        assert!(m.contains("kubectl not found"));
        assert!(
            m.contains("/usr/bin:/bin"),
            "where: must echo the searched PATH"
        );
        assert!(m.contains("install kubectl"), "fix: must be actionable");
        // Empty PATH renders a legible placeholder, not a blank.
        assert!(not_found_message("ns", "").contains("(empty PATH)"));
    }
}
