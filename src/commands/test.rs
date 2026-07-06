//! `inspect test <ns>` — validate a namespace's configuration.
//!
//! Phase 0 scope:
//!
//! 1. Resolve the namespace (env ∪ file).
//! 2. Validate required fields and conflict rules.
//! 3. Check that `key_path`, if set, points to an existing readable file with
//!    safe permissions (0600 or 0400) on unix.
//! 4. Verify TCP reachability of `host:port` (default 22) with a short timeout.
//!
//! Real SSH authentication is the job of `inspect connect`. This command
//! intentionally does not attempt cryptographic auth so it remains side-effect
//! free and runnable from CI without secrets.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use crate::cli::TestArgs;
use crate::config::namespace::validate_namespace_name;
use crate::config::resolver;
use crate::error::ExitKind;

const TCP_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_PORT: u16 = 22;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl CheckStatus {
    fn label(self) -> &'static str {
        match self {
            CheckStatus::Pass => "pass",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
            CheckStatus::Skip => "skip",
        }
    }
}

struct Check {
    name: &'static str,
    status: CheckStatus,
    detail: String,
}

pub fn run(args: TestArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;
    let r = resolver::resolve(&args.namespace)?;
    let cfg = &r.config;

    // A k8s namespace is reached via kubectl locally, not SSH.
    // Its `test` runs k8s-appropriate checks (config, kubectl backend, API
    // reachability) and skips the SSH key/tcp checks that are nonsensical for
    // a kubeconfig target (running them would report bogus "no key_path" /
    // "no host" failures — itself a mindtrap). API failures are classified
    // through the taxonomy so an agent gets a stable `failure_class` + a
    // chained hint rather than raw kubectl prose.
    if cfg.runtime_kind() == crate::exec::runtime::RuntimeKind::K8s {
        let checks = k8s_checks(cfg, &r.name);
        let overall = overall_status(&checks);
        if args.format.is_json() {
            emit_json(&r.name, &checks, overall);
        } else {
            emit_text_k8s(&r.name, &checks, overall);
        }
        return Ok(match overall {
            CheckStatus::Pass | CheckStatus::Warn => ExitKind::Success,
            _ => ExitKind::Error,
        });
    }

    let mut checks: Vec<Check> = Vec::new();

    // 1. Required fields
    match cfg.validate(&r.name) {
        Ok(()) => checks.push(Check {
            name: "config",
            status: CheckStatus::Pass,
            detail: "required fields present".into(),
        }),
        Err(e) => checks.push(Check {
            name: "config",
            status: CheckStatus::Fail,
            detail: e.to_string(),
        }),
    }

    // 2. Key file exists and has safe permissions
    if let Some(path) = cfg.key_path.as_deref() {
        let expanded = expand_tilde(path);
        let p = Path::new(&expanded);
        if !p.exists() {
            checks.push(Check {
                name: "key_file",
                status: CheckStatus::Fail,
                detail: format!("key_path does not exist: {expanded}"),
            });
        } else {
            let detail = key_file_detail(p);
            checks.push(detail);
        }
    } else if cfg.key_inline.is_some() {
        checks.push(Check {
            name: "key_file",
            status: CheckStatus::Pass,
            detail: "inline key (env-only) configured".into(),
        });
    } else {
        checks.push(Check {
            name: "key_file",
            status: CheckStatus::Fail,
            detail: "no key_path or key_inline configured".into(),
        });
    }

    // 3. TCP reachability
    let host = cfg.host.as_deref().unwrap_or("");
    let port = cfg.port.unwrap_or(DEFAULT_PORT);
    if host.is_empty() {
        checks.push(Check {
            name: "tcp",
            status: CheckStatus::Skip,
            detail: "no host configured".into(),
        });
    } else {
        checks.push(probe_tcp(host, port));
    }

    let overall = overall_status(&checks);

    if args.format.is_json() {
        emit_json(&r.name, &checks, overall);
    } else {
        emit_text(&r.name, host, port, &checks, overall);
    }

    Ok(match overall {
        CheckStatus::Pass | CheckStatus::Warn => ExitKind::Success,
        _ => ExitKind::Error,
    })
}

/// The k8s check set — config, kubectl backend presence, and a
/// context-pinned API-reachability probe. Any kubectl failure is routed
/// through the classifier so the reported detail is
/// `[<failure_class>] <hint>` rather than raw kubectl stderr.
fn k8s_checks(cfg: &crate::config::namespace::NamespaceConfig, name: &str) -> Vec<Check> {
    use crate::exec::kubectl;
    let mut checks: Vec<Check> = Vec::new();

    // 1. Config validity (type-conditional; k8s needs no host/user).
    match cfg.validate(name) {
        Ok(()) => checks.push(Check {
            name: "config",
            status: CheckStatus::Pass,
            detail: "k8s addressing present".into(),
        }),
        Err(e) => checks.push(Check {
            name: "config",
            status: CheckStatus::Fail,
            detail: e.to_string(),
        }),
    }

    // 2. kubectl backend presence. Without it, no API probe is possible.
    let probe = kubectl::probe_kubectl();
    if probe.available {
        let mut detail = probe.version.clone().unwrap_or_else(|| "present".into());
        if let Some(w) = probe.floor_warning() {
            detail = format!("{detail} — {w}");
        }
        checks.push(Check {
            name: "kubectl",
            status: if probe.floor_warning().is_some() {
                CheckStatus::Warn
            } else {
                CheckStatus::Pass
            },
            detail,
        });
    } else {
        checks.push(Check {
            name: "kubectl",
            status: CheckStatus::Fail,
            detail: "kubectl not found on PATH (k8s backend) — install kubectl \
                     (https://kubernetes.io/docs/tasks/tools/)"
                .into(),
        });
        return checks; // no point probing the API without kubectl
    }

    // 3. API reachability — context-pinned per the anti-footgun invariant
    //    (never the ambient current-context). Failures are classified.
    let mut cmd = k8s_kubectl_base(cfg);
    cmd.args(["version", "--output=json", "--request-timeout=5s"]);
    let api_ok = match cmd.output() {
        Ok(out) if out.status.success() => {
            checks.push(Check {
                name: "api",
                status: CheckStatus::Pass,
                detail: "API server reachable".into(),
            });
            true
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let fc = kubectl::classify_kubectl_failure(&stderr, out.status.code().unwrap_or(1));
            checks.push(Check {
                name: "api",
                status: CheckStatus::Fail,
                detail: format!("[{}] {}", fc.failure_class(), fc.hint("")),
            });
            false
        }
        Err(e) => {
            checks.push(Check {
                name: "api",
                status: CheckStatus::Fail,
                detail: format!("could not spawn kubectl: {e}"),
            });
            false
        }
    };

    // 4-5. RBAC self-test + metrics-server probe — only meaningful once
    //      the API is reachable. `auth can-i` pre-empts a mid-task Forbidden
    //      (research w3-P2); the metrics probe pre-answers `top` (w3-P6).
    if api_ok {
        checks.push(k8s_rbac_check(cfg));
        checks.push(k8s_metrics_check(cfg));
    }
    checks
}

/// A context-pinned kubectl base command — `--context` + `--kubeconfig`
/// from config, never the ambient current-context. The k8s namespace `-n`
/// scope is added per-call where relevant (not on cluster-scoped calls).
fn k8s_kubectl_base(cfg: &crate::config::namespace::NamespaceConfig) -> std::process::Command {
    let mut cmd = std::process::Command::new("kubectl");
    if let Some(ctx) = cfg.context.as_deref() {
        cmd.args(["--context", ctx]);
    }
    if let Some(kc) = cfg.kubeconfig.as_deref() {
        cmd.args(["--kubeconfig", &expand_tilde(kc)]);
    }
    cmd
}

/// RBAC self-test: run `kubectl auth can-i` for the EXACT verbs inspect
/// uses (not a superset — bible security-scope-narrower). Reads are essential
/// (deny → fail); the writes are optional capabilities (deny → warn, you can
/// still diagnose). Pre-empts the mid-task Forbidden that is the #1 RBAC pain.
fn k8s_rbac_check(cfg: &crate::config::namespace::NamespaceConfig) -> Check {
    // (verb, resource, is_read)
    let probes: [(&str, &str, bool); 5] = [
        ("get", "pods", true),
        ("get", "pods/log", true),
        ("create", "pods/exec", false),
        ("patch", "deployments", false),
        ("delete", "pods", false),
    ];
    let ns = cfg.k8s_namespace.as_deref();
    let mut denied_reads = Vec::new();
    let mut denied_writes = Vec::new();
    for (verb, res, is_read) in probes {
        let mut cmd = k8s_kubectl_base(cfg);
        cmd.args(["auth", "can-i", verb, res, "--request-timeout=5s"]);
        if let Some(n) = ns {
            cmd.args(["-n", n]);
        }
        // can-i exits 0 = allowed, non-zero = denied; stdout is yes/no.
        let allowed = cmd.output().map(|o| o.status.success()).unwrap_or(false);
        if !allowed {
            let label = format!("{verb} {res}");
            if is_read {
                denied_reads.push(label);
            } else {
                denied_writes.push(label);
            }
        }
    }
    rbac_verdict(&denied_reads, &denied_writes)
}

/// Pure verdict from the RBAC probe results (extracted so it is unit-testable
/// without a live cluster): denied reads → fail (can't diagnose); denied
/// writes only → warn (diagnostics still work); none → pass.
fn rbac_verdict(denied_reads: &[String], denied_writes: &[String]) -> Check {
    if !denied_reads.is_empty() {
        Check {
            name: "rbac",
            status: CheckStatus::Fail,
            detail: format!(
                "missing essential read permission(s): {} — inspect cannot \
                 diagnose this namespace. Grant a role covering `get pods` + \
                 `get pods/log`.",
                denied_reads.join(", ")
            ),
        }
    } else if !denied_writes.is_empty() {
        Check {
            name: "rbac",
            status: CheckStatus::Warn,
            detail: format!(
                "reads OK; write capability not granted: {} (diagnostics work; \
                 scale/restart/exec/delete will be Forbidden)",
                denied_writes.join(", ")
            ),
        }
    } else {
        Check {
            name: "rbac",
            status: CheckStatus::Pass,
            detail: "all inspect verbs permitted (reads + writes)".into(),
        }
    }
}

/// Metrics-server probe: pre-answers whether `top` will work, so an agent
/// isn't surprised by `metrics_unavailable` mid-task (research w3-P6). A
/// missing metrics-server is a cluster-component gap (Warn), not a failure.
fn k8s_metrics_check(cfg: &crate::config::namespace::NamespaceConfig) -> Check {
    let mut cmd = k8s_kubectl_base(cfg);
    cmd.args(["top", "pods", "--request-timeout=5s"]);
    if let Some(n) = cfg.k8s_namespace.as_deref() {
        cmd.args(["-n", n]);
    }
    match cmd.output() {
        Ok(out) if out.status.success() => Check {
            name: "metrics",
            status: CheckStatus::Pass,
            detail: "metrics-server available (`inspect top` will work)".into(),
        },
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let fc = crate::exec::kubectl::classify_kubectl_failure(
                &stderr,
                out.status.code().unwrap_or(1),
            );
            Check {
                name: "metrics",
                status: CheckStatus::Warn,
                detail: format!("[{}] {}", fc.failure_class(), fc.hint("")),
            }
        }
        Err(e) => Check {
            name: "metrics",
            status: CheckStatus::Warn,
            detail: format!("could not spawn kubectl: {e}"),
        },
    }
}

/// k8s-specific text output — no `host:port` line, and a sessionless NEXT
/// hint (k8s has no `connect` step — Q6/WA safety property).
fn emit_text_k8s(name: &str, checks: &[Check], overall: CheckStatus) {
    println!("SUMMARY: namespace '{name}' (k8s) -> {}", overall.label());
    println!("DATA:");
    for c in checks {
        println!("  [{:<4}] {:<10} {}", c.status.label(), c.name, c.detail);
    }
    match overall {
        CheckStatus::Pass | CheckStatus::Warn => {
            println!("NEXT:    inspect setup {name}   (k8s is sessionless — no connect step)");
        }
        _ => {
            println!("NEXT:    fix the failed checks above; rerun inspect test {name}");
        }
    }
}

fn key_file_detail(path: &Path) -> Check {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                return Check {
                    name: "key_file",
                    status: CheckStatus::Fail,
                    detail: format!("cannot stat {}: {e}", path.display()),
                };
            }
        };
        let mode = meta.permissions().mode() & 0o777;
        if matches!(mode, 0o600 | 0o400) {
            Check {
                name: "key_file",
                status: CheckStatus::Pass,
                detail: format!("{} mode {:o}", path.display(), mode),
            }
        } else {
            Check {
                name: "key_file",
                status: CheckStatus::Warn,
                detail: format!(
                    "{} mode {:o} (recommend 0600 or 0400)",
                    path.display(),
                    mode
                ),
            }
        }
    }
    #[cfg(not(unix))]
    {
        Check {
            name: "key_file",
            status: CheckStatus::Pass,
            detail: format!("{} present", path.display()),
        }
    }
}

fn probe_tcp(host: &str, port: u16) -> Check {
    let target = format!("{host}:{port}");
    let addrs: Vec<SocketAddr> = match target.to_socket_addrs() {
        Ok(it) => it.collect(),
        Err(e) => {
            return Check {
                name: "tcp",
                status: CheckStatus::Fail,
                detail: format!("DNS resolution failed for {target}: {e}"),
            };
        }
    };
    if addrs.is_empty() {
        return Check {
            name: "tcp",
            status: CheckStatus::Fail,
            detail: format!("no addresses resolved for {target}"),
        };
    }
    for addr in &addrs {
        if TcpStream::connect_timeout(addr, TCP_TIMEOUT).is_ok() {
            return Check {
                name: "tcp",
                status: CheckStatus::Pass,
                detail: format!("connected to {addr} within {:?}", TCP_TIMEOUT),
            };
        }
    }
    Check {
        name: "tcp",
        status: CheckStatus::Fail,
        detail: format!(
            "could not connect to any of {} address(es) for {target} within {:?}",
            addrs.len(),
            TCP_TIMEOUT
        ),
    }
}

fn overall_status(checks: &[Check]) -> CheckStatus {
    let mut worst = CheckStatus::Pass;
    for c in checks {
        worst = match (worst, c.status) {
            (CheckStatus::Fail, _) | (_, CheckStatus::Fail) => CheckStatus::Fail,
            (CheckStatus::Warn, _) | (_, CheckStatus::Warn) => CheckStatus::Warn,
            (CheckStatus::Skip, x) | (x, CheckStatus::Skip) => x,
            (CheckStatus::Pass, CheckStatus::Pass) => CheckStatus::Pass,
        };
    }
    worst
}

fn emit_text(name: &str, host: &str, port: u16, checks: &[Check], overall: CheckStatus) {
    println!(
        "SUMMARY: namespace '{}' -> {} ({}:{})",
        name,
        overall.label(),
        if host.is_empty() { "<unset>" } else { host },
        port
    );
    println!("DATA:");
    for c in checks {
        println!("  [{:<4}] {:<10} {}", c.status.label(), c.name, c.detail);
    }
    match overall {
        CheckStatus::Pass | CheckStatus::Warn => {
            println!("NEXT:    inspect connect {name}");
        }
        CheckStatus::Fail => {
            println!("NEXT:    fix the failed checks above; rerun inspect test {name}");
        }
        CheckStatus::Skip => {
            println!("NEXT:    inspect show {name}");
        }
    }
}

fn emit_json(name: &str, checks: &[Check], overall: CheckStatus) {
    use crate::commands::list::json_string;
    let mut s = format!(
        "{{\"schema_version\":1,\"name\":{name},\"overall\":{overall},\"checks\":[",
        name = json_string(name),
        overall = json_string(overall.label())
    );
    for (i, c) in checks.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"name\":{},\"status\":{},\"detail\":{}}}",
            json_string(c.name),
            json_string(c.status.label()),
            json_string(&c.detail),
        ));
    }
    s.push_str("]}");
    println!("{s}");
}

fn expand_tilde(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = crate::paths::home_dir() {
            return home.join(stripped).display().to_string();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod k6_tests {
    use super::*;

    #[test]
    fn k6_rbac_verdict_denied_reads_fail() {
        let v = rbac_verdict(&["get pods".into()], &[]);
        assert_eq!(v.status, CheckStatus::Fail);
        assert!(v.detail.contains("get pods"));
    }

    #[test]
    fn k6_rbac_verdict_denied_writes_only_warn() {
        let v = rbac_verdict(&[], &["patch deployments".into(), "delete pods".into()]);
        assert_eq!(v.status, CheckStatus::Warn);
        assert!(v.detail.contains("patch deployments"));
    }

    #[test]
    fn k6_rbac_verdict_all_permitted_pass() {
        let v = rbac_verdict(&[], &[]);
        assert_eq!(v.status, CheckStatus::Pass);
    }
}
