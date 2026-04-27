//! Preflight / postflight check engine for `inspect bundle` (B9).
//!
//! Five first-class check kinds + an `exec:` escape hatch. Each kind
//! compiles to a single remote command so the engine doesn't need to
//! reach into other verbs' guts:
//!
//! | Check              | Probe issued on the target                                   |
//! |--------------------|--------------------------------------------------------------|
//! | `disk_free`        | `df -P <path>` — parses the 4th column (Available bytes)     |
//! | `docker_running`   | `docker inspect -f '{{.State.Running}}' <name>...`           |
//! | `services_healthy` | `docker inspect -f '{{.State.Health.Status}}' <name>...`     |
//! | `http_ok`          | `curl -fsS <url>`                                            |
//! | `sql_returns`      | `docker exec <ctr> psql -tAc <sql>`                          |
//! | `exec`             | run the operator's command verbatim                          |
//!
//! `services_healthy` retries until `timeout:` elapses (default 0s =
//! single probe); the others are one-shot. All checks honor the
//! step-level `target:` override, falling back to the bundle's `host:`.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

use crate::ssh::exec::RunOpts;
use crate::ssh::options::SshTarget;
use crate::verbs::duration::parse_duration;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::{resolve_target, RemoteRunner};

use super::schema::Check;

/// Outcome of running a single check. `Ok(true)` = pass; `Ok(false)` =
/// fail (with `detail` describing why); `Err(_)` = the probe itself
/// could not run.
pub struct CheckResult {
    pub label: String,
    pub passed: bool,
    pub detail: String,
}

/// Resolve the SSH target for a check, preferring the check's own
/// `target:` override over the bundle default.
fn target_for(
    check_target: Option<&str>,
    default_host: Option<&str>,
) -> Result<(String, SshTarget)> {
    let ns = check_target
        .or(default_host)
        .ok_or_else(|| anyhow!("check has no target and bundle has no `host:` default"))?;
    let (_resolved, target) = resolve_target(ns).context("resolving check target")?;
    Ok((ns.to_string(), target))
}

/// Per-check timeout. Checks should be cheap — a stuck `df` or curl
/// shouldn't keep a bundle wedged. 30s is generous.
const CHECK_TIMEOUT_SECS: u64 = 30;

pub fn run_check(
    runner: &dyn RemoteRunner,
    default_host: Option<&str>,
    check: &Check,
) -> Result<CheckResult> {
    match check {
        Check::DiskFree {
            path,
            min_gb,
            target,
        } => disk_free(runner, default_host, target.as_deref(), path, *min_gb),
        Check::DockerRunning { services, target } => {
            docker_running(runner, default_host, target.as_deref(), services)
        }
        Check::ServicesHealthy {
            services,
            target,
            timeout,
        } => services_healthy(
            runner,
            default_host,
            target.as_deref(),
            services,
            timeout.as_deref(),
        ),
        Check::HttpOk { url, target } => http_ok(runner, default_host, target.as_deref(), url),
        Check::SqlReturns {
            container,
            sql,
            psql_opts,
            target,
        } => sql_returns(
            runner,
            default_host,
            target.as_deref(),
            container,
            sql,
            psql_opts.as_deref(),
        ),
        Check::Exec { exec, target } => exec_check(runner, default_host, target.as_deref(), exec),
    }
}

// -----------------------------------------------------------------------------
// disk_free
// -----------------------------------------------------------------------------

fn disk_free(
    runner: &dyn RemoteRunner,
    default_host: Option<&str>,
    check_target: Option<&str>,
    path: &str,
    min_gb: u64,
) -> Result<CheckResult> {
    let (ns, target) = target_for(check_target, default_host)?;
    // -P: POSIX output (single line per filesystem, KB blocks). The
    // 4th column is "Available". `tail -n +2` strips the header.
    let cmd = format!("df -P {} | tail -n +2", shquote(path));
    let out = runner.run(
        &ns,
        &target,
        &cmd,
        RunOpts::with_timeout(CHECK_TIMEOUT_SECS),
    )?;
    if out.exit_code != 0 {
        return Ok(CheckResult {
            label: format!("disk_free({path} ≥ {min_gb}GB)"),
            passed: false,
            detail: format!("df failed: {}", out.stderr.trim()),
        });
    }
    let avail_kb = out
        .stdout
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(3))
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| anyhow!("df output unparseable: {}", out.stdout.trim()))?;
    let avail_gb = avail_kb / (1024 * 1024);
    Ok(CheckResult {
        label: format!("disk_free({path} ≥ {min_gb}GB)"),
        passed: avail_gb >= min_gb,
        detail: format!("available={avail_gb}GB"),
    })
}

// -----------------------------------------------------------------------------
// docker_running
// -----------------------------------------------------------------------------

fn docker_running(
    runner: &dyn RemoteRunner,
    default_host: Option<&str>,
    check_target: Option<&str>,
    services: &[String],
) -> Result<CheckResult> {
    let (ns, target) = target_for(check_target, default_host)?;
    if services.is_empty() {
        return Ok(CheckResult {
            label: "docker_running()".into(),
            passed: true,
            detail: "no services".into(),
        });
    }
    let names: Vec<String> = services.iter().map(|s| shquote(s)).collect();
    // One docker call, all containers. Each line is `name|true|false`.
    let cmd = format!(
        "docker inspect -f '{{{{.Name}}}}|{{{{.State.Running}}}}' {} 2>&1",
        names.join(" ")
    );
    let out = runner.run(
        &ns,
        &target,
        &cmd,
        RunOpts::with_timeout(CHECK_TIMEOUT_SECS),
    )?;
    let mut failed: Vec<String> = Vec::new();
    for line in out.stdout.lines() {
        let mut parts = line.splitn(2, '|');
        let name = parts.next().unwrap_or("").trim_start_matches('/');
        let running = parts.next().unwrap_or("").trim();
        if running != "true" {
            failed.push(format!("{name}={running}"));
        }
    }
    // If docker inspect itself errored (e.g. unknown container), the
    // exit code is non-zero and stdout may be partial. Surface that.
    if out.exit_code != 0 && failed.is_empty() {
        failed.push(out.stderr.trim().to_string());
    }
    Ok(CheckResult {
        label: format!("docker_running({})", services.join(",")),
        passed: failed.is_empty(),
        detail: if failed.is_empty() {
            format!("{} container(s) running", services.len())
        } else {
            format!("not running: {}", failed.join(", "))
        },
    })
}

// -----------------------------------------------------------------------------
// services_healthy
// -----------------------------------------------------------------------------

fn services_healthy(
    runner: &dyn RemoteRunner,
    default_host: Option<&str>,
    check_target: Option<&str>,
    services: &[String],
    timeout_str: Option<&str>,
) -> Result<CheckResult> {
    let (ns, target) = target_for(check_target, default_host)?;
    let timeout = match timeout_str {
        Some(s) => parse_duration(s).map_err(|e| anyhow!("services_healthy timeout: {e}"))?,
        None => Duration::ZERO,
    };
    let label = format!("services_healthy({})", services.join(","));
    if services.is_empty() {
        return Ok(CheckResult {
            label,
            passed: true,
            detail: "no services".into(),
        });
    }
    let names: Vec<String> = services.iter().map(|s| shquote(s)).collect();
    // Health.Status is "healthy" / "unhealthy" / "starting" / "" if
    // the image has no HEALTHCHECK. Treat empty as "running fine"
    // because most images don't define one — we already validated
    // running-ness via docker_running.
    let cmd = format!(
        "docker inspect -f '{{{{.Name}}}}|{{{{.State.Status}}}}|{{{{.State.Health.Status}}}}' {} 2>&1",
        names.join(" ")
    );
    let started = Instant::now();
    loop {
        let out = runner.run(
            &ns,
            &target,
            &cmd,
            RunOpts::with_timeout(CHECK_TIMEOUT_SECS),
        )?;
        let mut bad: Vec<String> = Vec::new();
        for line in out.stdout.lines() {
            let mut parts = line.splitn(3, '|');
            let name = parts.next().unwrap_or("").trim_start_matches('/');
            let state = parts.next().unwrap_or("").trim();
            let health = parts.next().unwrap_or("").trim();
            // Pass when state=running AND (no health check OR healthy).
            let ok = state == "running" && (health.is_empty() || health == "healthy");
            if !ok {
                bad.push(format!("{name}={state}/{health}"));
            }
        }
        if bad.is_empty() && out.exit_code == 0 {
            return Ok(CheckResult {
                label,
                passed: true,
                detail: format!("{} healthy", services.len()),
            });
        }
        let last_detail = if bad.is_empty() {
            out.stderr.trim().to_string()
        } else {
            format!("not healthy: {}", bad.join(", "))
        };
        if started.elapsed() >= timeout {
            return Ok(CheckResult {
                label,
                passed: false,
                detail: last_detail,
            });
        }
        // Re-poll every 2s while waiting.
        let next = started + timeout;
        let wake = (Instant::now() + Duration::from_secs(2)).min(next);
        let now = Instant::now();
        if wake > now {
            std::thread::sleep(wake - now);
        }
    }
}

// -----------------------------------------------------------------------------
// http_ok
// -----------------------------------------------------------------------------

fn http_ok(
    runner: &dyn RemoteRunner,
    default_host: Option<&str>,
    check_target: Option<&str>,
    url: &str,
) -> Result<CheckResult> {
    let (ns, target) = target_for(check_target, default_host)?;
    // -f: fail on 4xx/5xx so curl's exit code carries the verdict.
    // -sS: silent but show errors; -o /dev/null: drop body.
    let cmd = format!(
        "curl -fsS -o /dev/null -w '%{{http_code}}' {}",
        shquote(url)
    );
    let out = runner.run(
        &ns,
        &target,
        &cmd,
        RunOpts::with_timeout(CHECK_TIMEOUT_SECS),
    )?;
    let status = out.stdout.trim();
    Ok(CheckResult {
        label: format!("http_ok({url})"),
        passed: out.exit_code == 0,
        detail: if out.exit_code == 0 {
            format!("HTTP {status}")
        } else {
            format!(
                "curl exit {} ({})",
                out.exit_code,
                out.stderr.trim().lines().next().unwrap_or("")
            )
        },
    })
}

// -----------------------------------------------------------------------------
// sql_returns
// -----------------------------------------------------------------------------

fn sql_returns(
    runner: &dyn RemoteRunner,
    default_host: Option<&str>,
    check_target: Option<&str>,
    container: &str,
    sql: &str,
    psql_opts: Option<&str>,
) -> Result<CheckResult> {
    let (ns, target) = target_for(check_target, default_host)?;
    let psql = match psql_opts {
        Some(o) if !o.trim().is_empty() => format!("psql {} -tAc {}", o, shquote(sql)),
        _ => format!("psql -tAc {}", shquote(sql)),
    };
    let cmd = format!(
        "docker exec {} sh -c {}",
        shquote(container),
        shquote(&psql)
    );
    let out = runner.run(
        &ns,
        &target,
        &cmd,
        RunOpts::with_timeout(CHECK_TIMEOUT_SECS),
    )?;
    let trimmed = out.stdout.trim();
    let truthy = matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "t" | "true" | "1" | "yes" | "y"
    );
    Ok(CheckResult {
        label: format!("sql_returns({container}: {})", short(sql)),
        passed: truthy && out.exit_code == 0,
        detail: if trimmed.is_empty() {
            out.stderr.trim().to_string()
        } else {
            format!("returned `{trimmed}`")
        },
    })
}

// -----------------------------------------------------------------------------
// exec (escape hatch)
// -----------------------------------------------------------------------------

fn exec_check(
    runner: &dyn RemoteRunner,
    default_host: Option<&str>,
    check_target: Option<&str>,
    exec_cmd: &str,
) -> Result<CheckResult> {
    let (ns, target) = target_for(check_target, default_host)?;
    let out = runner.run(
        &ns,
        &target,
        exec_cmd,
        RunOpts::with_timeout(CHECK_TIMEOUT_SECS),
    )?;
    Ok(CheckResult {
        label: format!("exec(`{}`)", short(exec_cmd)),
        passed: out.exit_code == 0,
        detail: if out.exit_code == 0 {
            "exit 0".to_string()
        } else {
            format!(
                "exit {} ({})",
                out.exit_code,
                out.stderr.trim().lines().next().unwrap_or("")
            )
        },
    })
}

/// Truncate long command/SQL strings for log/audit display.
fn short(s: &str) -> String {
    let one = s.replace('\n', " ");
    if one.chars().count() > 60 {
        let mut out: String = one.chars().take(57).collect();
        out.push('…');
        out
    } else {
        one
    }
}

/// Render a check's source location for plan output. Used by
/// `inspect bundle plan` to show what would run without executing.
pub fn describe_check(check: &Check) -> String {
    match check {
        Check::DiskFree { path, min_gb, .. } => format!("disk_free path={path} min_gb={min_gb}"),
        Check::DockerRunning { services, .. } => {
            format!("docker_running services={}", services.join(","))
        }
        Check::ServicesHealthy {
            services, timeout, ..
        } => format!(
            "services_healthy services={} timeout={}",
            services.join(","),
            timeout.as_deref().unwrap_or("0s")
        ),
        Check::HttpOk { url, .. } => format!("http_ok url={url}"),
        Check::SqlReturns { container, sql, .. } => {
            format!("sql_returns container={container} sql={}", short(sql))
        }
        Check::Exec { exec, .. } => format!("exec `{}`", short(exec)),
    }
}

// -----------------------------------------------------------------------------
// Plan-side rendering
// -----------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::schema::Check;

    #[test]
    fn describe_disk_free() {
        let c = Check::DiskFree {
            path: "/srv".into(),
            min_gb: 50,
            target: None,
        };
        let s = describe_check(&c);
        assert!(s.contains("disk_free"));
        assert!(s.contains("/srv"));
        assert!(s.contains("50"));
    }

    #[test]
    fn describe_docker_running() {
        let c = Check::DockerRunning {
            services: vec!["a".into(), "b".into()],
            target: None,
        };
        let s = describe_check(&c);
        assert!(s.contains("a,b"));
    }

    #[test]
    fn describe_http_ok() {
        let c = Check::HttpOk {
            url: "http://x/y".into(),
            target: None,
        };
        let s = describe_check(&c);
        assert!(s.contains("http_ok"));
        assert!(s.contains("http://x/y"));
    }

    #[test]
    fn short_truncates_long_strings_with_ellipsis() {
        let long = "a".repeat(200);
        let s = short(&long);
        assert!(s.chars().count() <= 60);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn short_collapses_newlines() {
        assert_eq!(short("a\nb\nc"), "a b c");
    }
}
