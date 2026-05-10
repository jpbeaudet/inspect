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
