//! K3 (v0.1.4): local `kubectl` backend probe + preflight.
//!
//! The kubectl shell-out backend (Q2) is only as reliable as the
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
/// warning by [`floor_warning`]. (Warn-not-fail is the choice recorded in
/// the K3 spec: "pick warn unless the plan says fail, and document it.")
pub const KUBECTL_MIN_MAJOR: u32 = 1;
/// See [`KUBECTL_MIN_MAJOR`].
pub const KUBECTL_MIN_MINOR: u32 = 19;

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
    let minor_digits: String = minor_raw.chars().take_while(|c| c.is_ascii_digit()).collect();
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
/// kubernetes` pointer — that editorial topic lands in K24. The `fix:`
/// line carries the actionable install pointer directly until then.
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

#[cfg(test)]
mod tests {
    use super::*;

    // K3 acceptance (unit-level). The crate is bin-only (no `[lib]`),
    // so the `tests/phase_k_v014.rs` integration file is black-box and
    // cannot import these internal APIs — the pure version-floor and
    // parse logic is exercised here (same precedent as the in-module K1
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
            assert!(m.contains(needle), "four-question message missing {needle:?}: {m}");
        }
        assert!(m.contains("kubectl not found"));
        assert!(m.contains("/usr/bin:/bin"), "where: must echo the searched PATH");
        assert!(m.contains("install kubectl"), "fix: must be actionable");
        // Empty PATH renders a legible placeholder, not a blank.
        assert!(not_found_message("ns", "").contains("(empty PATH)"));
    }
}
