//! `inspect watch <target> --until-<kind>` (B10, v0.1.2).
//!
//! Block until a predicate over a single target becomes true, exit 124
//! on timeout (matching `timeout(1)`'s convention), exit 0 on match.
//! One watch invocation = one audit entry with `verb=watch`.
//!
//! Four predicate kinds, all mutually exclusive (enforced via clap
//! groups in [`crate::cli::WatchArgs`]):
//!
//! * `--until-cmd <CMD>` — run CMD on the target every interval; apply
//!   one of `--equals/--matches/--gt/--lt/--changes/--stable-for`. With
//!   no comparator, exit code 0 is the match condition.
//! * `--until-log <PATTERN>` — poll `docker logs --since <watch-start>`
//!   on the target each interval; literal substring by default,
//!   extended-regex with `--regex`.
//! * `--until-sql <SQL>` — `docker exec <ctr> psql -tAc <SQL>` on the
//!   target container; truthy iff trimmed stdout is one of t/true/1/yes
//!   (case-insensitive). Use `--psql-opts` to pass `-U/-d/...`.
//! * `--until-http <URL>` — `curl -sS -w ...` on the target; without
//!   `--match` matches any 2xx/3xx; with `--match <EXPR>` evaluates a
//!   tiny DSL `<lhs> <op> <rhs>` where lhs ∈ {body, status, $.foo.bar},
//!   op ∈ {==, !=, <, >, contains}.
//!
//! Stacked predicates are out of scope for v0.1.2 — chain watches with
//! `&&` in the shell.

use std::io::IsTerminal;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use chrono::Utc;

use crate::cli::WatchArgs;
use crate::error::ExitKind;
use crate::safety::{AuditEntry, AuditStore};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan, Step};
use crate::verbs::duration::parse_duration;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

/// Default polling interval (`--interval`) when the operator doesn't
/// override it. Two seconds matches `kubectl rollout status`'s cadence.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(2);

/// Default total timeout (`--timeout`). Ten minutes is enough for
/// rolling restarts and DB warm-up, short enough that a stuck watch
/// fails the surrounding script before a human notices.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// Per-poll command timeout. Each individual probe must finish in
/// time for the next interval; we cap it so a hung remote doesn't
/// stretch the whole watch beyond `--timeout`.
const POLL_CMD_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone)]
enum Predicate {
    Cmd(String),
    Log(String),
    Sql(String),
    Http(String),
}

impl Predicate {
    fn label(&self) -> String {
        match self {
            Predicate::Cmd(c) => format!("cmd `{c}`"),
            Predicate::Log(p) => format!("log `{p}`"),
            Predicate::Sql(q) => format!("sql `{q}`"),
            Predicate::Http(u) => format!("http `{u}`"),
        }
    }
}

/// Result of a single poll.
enum Probe {
    /// Predicate satisfied; carry the value that triggered the match
    /// (printed to stdout; recorded in the audit entry summary).
    Match(String),
    /// Predicate not yet satisfied; carry the most recent observation
    /// for status display.
    NoMatch(String),
}

pub fn run(args: WatchArgs) -> Result<ExitKind> {
    // Predicate selection (clap groups guarantee ≤ 1, but be defensive
    // because `inspect help` regression-tests parse args without the
    // full clap validation cycle in some paths).
    let pred = match (
        args.until_cmd.as_deref(),
        args.until_log.as_deref(),
        args.until_sql.as_deref(),
        args.until_http.as_deref(),
    ) {
        (Some(c), None, None, None) => Predicate::Cmd(c.to_string()),
        (None, Some(l), None, None) => Predicate::Log(l.to_string()),
        (None, None, Some(q), None) => Predicate::Sql(q.to_string()),
        (None, None, None, Some(u)) => Predicate::Http(u.to_string()),
        _ => {
            crate::error::emit(
                "watch requires exactly one of --until-cmd / --until-log / --until-sql / --until-http",
            );
            return Ok(ExitKind::Error);
        }
    };

    // Cmd-only comparators. Validated up-front so the operator gets a
    // precise error before any remote dial.
    let cmp = CmdCmp::from_args(&args)?;

    // Loop knobs.
    let interval = match args.interval.as_deref() {
        Some(s) => parse_duration(s).map_err(|e| anyhow!("--interval {e}"))?,
        None => DEFAULT_INTERVAL,
    };
    let total_timeout = match args.timeout.as_deref() {
        Some(s) => parse_duration(s).map_err(|e| anyhow!("--timeout {e}"))?,
        None => DEFAULT_TIMEOUT,
    };
    let stable_for = match args.stable_for.as_deref() {
        Some(s) => Some(parse_duration(s).map_err(|e| anyhow!("--stable-for {e}"))?),
        None => None,
    };

    // Compile --matches / --regex up-front so a bad regex fails fast
    // instead of after the first poll.
    let cmd_re = match args.matches.as_deref() {
        Some(p) => Some(regex::Regex::new(p).map_err(|e| anyhow!("--matches {e}"))?),
        None => None,
    };
    let log_re = if args.regex {
        if let Some(p) = args.until_log.as_deref() {
            Some(regex::Regex::new(p).map_err(|e| anyhow!("--until-log {e}"))?)
        } else {
            None
        }
    } else {
        None
    };

    let reason = crate::safety::validate_reason(args.reason.as_deref())?;
    if let Some(r) = &reason {
        crate::tee_eprintln!("# reason: {r}");
    }

    // Resolve selector → exactly one target. Watch is single-target by
    // design; multi-target fan-out belongs in `inspect bundle` (B9).
    let (runner, nses, targets) = plan(&args.selector).map_err(|e| anyhow!("{e}"))?;
    let steps: Vec<_> = iter_steps(&nses, &targets).collect();
    if steps.is_empty() {
        crate::error::emit(format!("'{}' matched no targets", args.selector));
        return Ok(ExitKind::Error);
    }
    if steps.len() > 1 {
        crate::error::emit(format!(
            "watch requires a single target ('{}' matched {}). Refine the selector or chain watches with `&&`.",
            args.selector,
            steps.len()
        ));
        return Ok(ExitKind::Error);
    }
    let step = &steps[0];
    let label = format!(
        "{}{}",
        step.ns.namespace,
        step.service().map(|x| format!("/{x}")).unwrap_or_default()
    );
    let pred_label = pred.label();
    let tty = std::io::stderr().is_terminal();

    // Anchor docker-logs --since to *before* the watch started, in
    // RFC3339, so the first poll already sees lines emitted between
    // the operator pressing Enter and the first SSH round-trip.
    let log_since = Utc::now().to_rfc3339();

    let started = Instant::now();
    let deadline = if total_timeout.is_zero() {
        None
    } else {
        Some(started + total_timeout)
    };

    // State carried across polls (used by --changes / --stable-for).
    let mut prev_value: Option<String> = None;
    let mut stable_anchor: Option<(Instant, String)> = None;
    let mut poll_n: u64 = 0;
    let mut last_seen: Option<String> = None;

    let outcome = loop {
        if crate::exec::cancel::is_cancelled() {
            break Outcome::Cancelled;
        }
        if let Some(d) = deadline {
            if Instant::now() >= d {
                break Outcome::Timeout;
            }
        }

        poll_n += 1;
        emit_status(
            &label,
            &pred_label,
            poll_n,
            started.elapsed(),
            tty,
            args.verbose,
        );

        let probe = match &pred {
            Predicate::Cmd(c) => probe_cmd(
                &*runner,
                step,
                c,
                &cmp,
                cmd_re.as_ref(),
                &mut prev_value,
                &mut stable_anchor,
                stable_for,
            ),
            Predicate::Log(pat) => probe_log(&*runner, step, pat, log_re.as_ref(), &log_since),
            Predicate::Sql(q) => probe_sql(&*runner, step, q, args.psql_opts.as_deref()),
            Predicate::Http(u) => {
                probe_http(&*runner, step, u, args.r#match.as_deref(), args.insecure)
            }
        };

        match probe {
            Ok(Probe::Match(value)) => {
                break Outcome::Match {
                    value,
                    polls: poll_n,
                };
            }
            Ok(Probe::NoMatch(seen)) => {
                last_seen = Some(seen);
            }
            Err(e) => break Outcome::Error(e.to_string()),
        }

        // Sleep until the next poll, but never past the deadline (so
        // we don't oversleep the timeout by up to one interval).
        let next = Instant::now() + interval;
        let wake = match deadline {
            Some(d) => next.min(d),
            None => next,
        };
        let now = Instant::now();
        if wake > now {
            std::thread::sleep(wake - now);
        }
    };

    // Clear the live status line before printing the final result so
    // captured-by-pipe stderr doesn't get a half-overwritten line.
    if tty && !args.verbose {
        eprint!("\r\x1b[K");
    }

    let dur_ms = started.elapsed().as_millis() as u64;
    let exit_audit: i32 = match &outcome {
        Outcome::Match { .. } => 0,
        Outcome::Timeout => 124,
        Outcome::Cancelled => 130,
        Outcome::Error(_) => 2,
    };

    // Best-effort audit append. Audit failures must not mask the watch
    // outcome (operator scripts read the exit code, not the audit log).
    if let Ok(store) = AuditStore::open() {
        let mut e = AuditEntry::new("watch", &label);
        e.args = match &outcome {
            Outcome::Match { value, polls } => {
                format!(
                    "{pred_label} [matched poll={polls} value={}]",
                    trimmed_for_audit(value)
                )
            }
            Outcome::Timeout => format!(
                "{pred_label} [timeout last={}]",
                last_seen
                    .as_deref()
                    .map(trimmed_for_audit)
                    .unwrap_or_default()
            ),
            Outcome::Cancelled => format!("{pred_label} [cancelled]"),
            Outcome::Error(msg) => format!("{pred_label} [error: {}]", trimmed_for_audit(msg)),
        };
        e.exit = exit_audit;
        e.duration_ms = dur_ms;
        e.reason = reason;
        let _ = store.append(&e);
    }

    match outcome {
        Outcome::Match { value, polls } => {
            // Emit the matched value to stdout so shell pipelines can
            // consume it (e.g. `version=$(inspect watch ...)`).
            crate::tee_println!("{}", value.trim_end_matches('\n'));
            crate::tee_eprintln!(
                "[inspect] watch matched on {label}: {pred_label} (poll {polls}, {dur_ms}ms)"
            );
            Ok(ExitKind::Success)
        }
        Outcome::Timeout => {
            crate::tee_eprintln!(
                "[inspect] watch timed out on {label} after {dur_ms}ms: {pred_label}{}",
                last_seen
                    .as_deref()
                    .map(|v| format!(" [last={}]", trimmed_for_audit(v)))
                    .unwrap_or_default()
            );
            Ok(ExitKind::Inner(124))
        }
        Outcome::Cancelled => {
            crate::tee_eprintln!("[inspect] watch cancelled on {label}: {pred_label}");
            Ok(ExitKind::Error)
        }
        Outcome::Error(msg) => {
            crate::error::emit(format!("watch failed on {label}: {msg}"));
            Ok(ExitKind::Error)
        }
    }
}

enum Outcome {
    Match { value: String, polls: u64 },
    Timeout,
    Cancelled,
    Error(String),
}

fn emit_status(
    label: &str,
    pred_label: &str,
    poll_n: u64,
    elapsed: Duration,
    tty: bool,
    verbose: bool,
) {
    let secs = elapsed.as_secs();
    if tty && !verbose {
        // In-place rewrite: \r + clear-to-EOL + new content. No newline
        // so the next poll overwrites cleanly.
        eprint!(
            "\r\x1b[K[inspect] watching {label}: {pred_label} (poll {poll_n}, {secs}s elapsed)"
        );
        let _ = std::io::Write::flush(&mut std::io::stderr());
    } else {
        crate::tee_eprintln!(
            "[inspect] watching {label}: {pred_label} (poll {poll_n}, {secs}s elapsed)"
        );
    }
}

/// Trim and collapse a value to a single short line for audit/UX use.
fn trimmed_for_audit(s: &str) -> String {
    let t = s.trim();
    let mut out = String::with_capacity(t.len().min(120));
    for ch in t.chars().take(120) {
        if ch == '\n' || ch == '\r' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    if t.chars().count() > 120 {
        out.push('…');
    }
    out
}

// ---------------------------------------------------------------------
// --until-cmd
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
enum CmdCmp {
    /// No comparator — match on exit code 0.
    ExitZero,
    Equals(String),
    Matches, // regex compiled separately
    Gt(f64),
    Lt(f64),
    Changes,
    StableFor, // duration carried separately
}

impl CmdCmp {
    fn from_args(args: &WatchArgs) -> Result<Self> {
        // Count the comparator flags that are set so we can reject
        // contradictory combinations early.
        let mut count = 0;
        if args.equals.is_some() {
            count += 1;
        }
        if args.matches.is_some() {
            count += 1;
        }
        if args.gt.is_some() {
            count += 1;
        }
        if args.lt.is_some() {
            count += 1;
        }
        if args.changes {
            count += 1;
        }
        if args.stable_for.is_some() {
            count += 1;
        }
        if count > 1 {
            return Err(anyhow!(
                "watch: only one of --equals/--matches/--gt/--lt/--changes/--stable-for may be set"
            ));
        }
        if let Some(v) = args.equals.clone() {
            return Ok(CmdCmp::Equals(v));
        }
        if args.matches.is_some() {
            return Ok(CmdCmp::Matches);
        }
        if let Some(n) = args.gt {
            return Ok(CmdCmp::Gt(n));
        }
        if let Some(n) = args.lt {
            return Ok(CmdCmp::Lt(n));
        }
        if args.changes {
            return Ok(CmdCmp::Changes);
        }
        if args.stable_for.is_some() {
            return Ok(CmdCmp::StableFor);
        }
        Ok(CmdCmp::ExitZero)
    }
}

#[allow(clippy::too_many_arguments)]
fn probe_cmd(
    runner: &dyn RemoteRunner,
    step: &Step<'_>,
    user_cmd: &str,
    cmp: &CmdCmp,
    cmd_re: Option<&regex::Regex>,
    prev_value: &mut Option<String>,
    stable_anchor: &mut Option<(Instant, String)>,
    stable_for: Option<Duration>,
) -> Result<Probe> {
    let cmd = wrap_in_docker_exec(step, user_cmd);
    let opts = RunOpts::with_timeout(POLL_CMD_TIMEOUT_SECS);
    let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, opts)?;
    let stdout_trim = out.stdout.trim().to_string();
    let exit = out.exit_code;

    let matched = match cmp {
        CmdCmp::ExitZero => exit == 0,
        CmdCmp::Equals(v) => stdout_trim == *v,
        CmdCmp::Matches => cmd_re.map(|r| r.is_match(&out.stdout)).unwrap_or(false),
        CmdCmp::Gt(n) => stdout_trim.parse::<f64>().map(|v| v > *n).unwrap_or(false),
        CmdCmp::Lt(n) => stdout_trim.parse::<f64>().map(|v| v < *n).unwrap_or(false),
        CmdCmp::Changes => match prev_value {
            Some(prev) if *prev != stdout_trim => true,
            _ => false, // first poll, or unchanged — never matches
        },
        CmdCmp::StableFor => {
            let now = Instant::now();
            let dur = stable_for.unwrap_or(Duration::from_secs(0));
            match stable_anchor {
                Some((anchor, val)) if *val == stdout_trim => now.duration_since(*anchor) >= dur,
                _ => {
                    *stable_anchor = Some((now, stdout_trim.clone()));
                    false
                }
            }
        }
    };
    *prev_value = Some(stdout_trim.clone());

    if matched {
        Ok(Probe::Match(stdout_trim))
    } else {
        Ok(Probe::NoMatch(stdout_trim))
    }
}

/// Wrap a user command in `docker exec <ctr> sh -c '<cmd>'` when the
/// step points at a container. Mirrors `inspect run` so cmd polling
/// behaves identically to a `run`-built equivalent.
fn wrap_in_docker_exec(step: &Step<'_>, user_cmd: &str) -> String {
    match step.container() {
        Some(container) => format!(
            "docker exec {} sh -c {}",
            shquote(container),
            shquote(user_cmd)
        ),
        None => user_cmd.to_string(),
    }
}

// ---------------------------------------------------------------------
// --until-log
// ---------------------------------------------------------------------

fn probe_log(
    runner: &dyn RemoteRunner,
    step: &Step<'_>,
    pattern: &str,
    log_re: Option<&regex::Regex>,
    since_rfc3339: &str,
) -> Result<Probe> {
    let container = step.container().ok_or_else(|| {
        anyhow!("--until-log requires a container target (e.g. arte/api), got host-level selector")
    })?;
    // Always merge stderr into stdout — most images log to stderr, and
    // we want both streams in the predicate match window.
    let cmd = format!(
        "docker logs --since {} {} 2>&1",
        shquote(since_rfc3339),
        shquote(container)
    );
    let opts = RunOpts::with_timeout(POLL_CMD_TIMEOUT_SECS);
    let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, opts)?;

    let hit_line = if let Some(re) = log_re {
        out.stdout.lines().rev().find(|l| re.is_match(l))
    } else {
        out.stdout.lines().rev().find(|l| l.contains(pattern))
    };

    if let Some(line) = hit_line {
        Ok(Probe::Match(line.to_string()))
    } else {
        // Last log line, if any, gives operator a sense of progress.
        let tail = out
            .stdout
            .lines()
            .next_back()
            .unwrap_or("(no logs yet)")
            .to_string();
        Ok(Probe::NoMatch(tail))
    }
}

// ---------------------------------------------------------------------
// --until-sql
// ---------------------------------------------------------------------

fn probe_sql(
    runner: &dyn RemoteRunner,
    step: &Step<'_>,
    sql: &str,
    psql_opts: Option<&str>,
) -> Result<Probe> {
    let container = step.container().ok_or_else(|| {
        anyhow!("--until-sql requires a container target (e.g. arte/db), got host-level selector")
    })?;
    // psql -tAc: tuples-only + unaligned + run single command. The
    // operator's SQL must yield a single scalar; we just trim and
    // compare against the truthy set.
    let opts_str = psql_opts.unwrap_or("");
    let psql_cmd = if opts_str.is_empty() {
        format!("psql -tAc {}", shquote(sql))
    } else {
        format!("psql {} -tAc {}", opts_str, shquote(sql))
    };
    let cmd = format!(
        "docker exec {} sh -c {}",
        shquote(container),
        shquote(&psql_cmd)
    );
    let opts = RunOpts::with_timeout(POLL_CMD_TIMEOUT_SECS);
    let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, opts)?;
    let trimmed = out.stdout.trim().to_string();
    let truthy = matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "t" | "true" | "1" | "yes" | "y"
    );
    if truthy && out.exit_code == 0 {
        Ok(Probe::Match(trimmed))
    } else {
        Ok(Probe::NoMatch(if trimmed.is_empty() {
            out.stderr.trim().to_string()
        } else {
            trimmed
        }))
    }
}

// ---------------------------------------------------------------------
// --until-http
// ---------------------------------------------------------------------

const HTTP_STATUS_MARKER: &str = "___INSPECT_HTTP_STATUS=";

fn probe_http(
    runner: &dyn RemoteRunner,
    step: &Step<'_>,
    url: &str,
    match_expr: Option<&str>,
    insecure: bool,
) -> Result<Probe> {
    // -sS: silent but show errors. We deliberately omit -f because we
    // want the body even on 4xx/5xx so the predicate DSL can inspect
    // it. The status code is appended in a marker line that we strip
    // back off before evaluating `body` / `$.json`.
    // --connect-timeout / --max-time: curl-level guards so a stuck
    // socket can't pin SSH for the full POLL_CMD_TIMEOUT_SECS budget.
    // --insecure is operator opt-in for self-signed staging only.
    let insecure_flag = if insecure { " --insecure" } else { "" };
    let curl_cmd = format!(
        "curl -sS --connect-timeout 5 --max-time 15{insecure_flag} -o - -w '\\n{marker}%{{http_code}}\\n' {url}",
        marker = HTTP_STATUS_MARKER,
        url = shquote(url)
    );
    let opts = RunOpts::with_timeout(POLL_CMD_TIMEOUT_SECS);
    let out = runner.run(&step.ns.namespace, &step.ns.target, &curl_cmd, opts)?;

    // Network failures (DNS, refused, ...) leave stdout empty and
    // surface a curl error message on stderr — surface that as the
    // "last seen" so operators see why polling isn't progressing.
    if out.stdout.is_empty() && out.exit_code != 0 {
        return Ok(Probe::NoMatch(format!(
            "curl exit {}: {}",
            out.exit_code,
            out.stderr.trim()
        )));
    }

    let (body, status) = split_http_response(&out.stdout);
    let value_summary = format!("status={status}");

    let matched = match match_expr {
        None => (200..400).contains(&status),
        Some(expr) => eval_http_match(expr, body, status)?,
    };

    if matched {
        Ok(Probe::Match(value_summary))
    } else {
        Ok(Probe::NoMatch(value_summary))
    }
}

/// Split the curl stdout into `(body, status_code)`. The marker is the
/// LAST occurrence of `___INSPECT_HTTP_STATUS=NNN` followed by EOL
/// (curl appends one trailing newline after the marker). On parse
/// failure status is 0 — callers treat that as never-matching.
fn split_http_response(s: &str) -> (&str, u16) {
    if let Some(idx) = s.rfind(HTTP_STATUS_MARKER) {
        let (before, after) = s.split_at(idx);
        // Strip the leading "\n" we wrote before the marker so the body
        // doesn't have a trailing empty line.
        let body = before.strip_suffix('\n').unwrap_or(before);
        let after = &after[HTTP_STATUS_MARKER.len()..];
        let code_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        let status = code_str.parse::<u16>().unwrap_or(0);
        (body, status)
    } else {
        (s, 0)
    }
}

/// Evaluate the tiny HTTP predicate DSL: `<lhs> <op> <rhs>`.
/// Supported lhs: `status`, `body`, `$.dot.path`. Supported ops:
/// `==`, `!=`, `<`, `>`, `contains`. `rhs` is the rest of the line
/// after the op, trimmed; surrounding double-quotes are stripped.
fn eval_http_match(expr: &str, body: &str, status: u16) -> Result<bool> {
    let expr = expr.trim();
    // Tokenize: lhs is everything up to the first op-token. We scan
    // for two-char ops first (==, !=) then single-char (<, >), then
    // the literal word `contains` surrounded by spaces.
    let (lhs, op, rhs) = split_match_expr(expr)
        .ok_or_else(|| anyhow!("--match: cannot parse `{expr}` (expected `<lhs> <op> <rhs>`)"))?;
    let rhs = strip_quotes(rhs.trim());

    let lhs_value: LhsValue = if lhs == "status" {
        LhsValue::Number(status as f64)
    } else if lhs == "body" {
        LhsValue::Text(body.to_string())
    } else if let Some(path) = lhs.strip_prefix("$.") {
        let json: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => return Ok(false), // body wasn't JSON → never matches
        };
        match json_lookup(&json, path) {
            Some(v) => LhsValue::from_json(&v),
            None => return Ok(false),
        }
    } else {
        return Err(anyhow!(
            "--match: unknown lhs `{lhs}` (use `status`, `body`, or `$.dot.path`)"
        ));
    };

    Ok(match op {
        Op::Eq => lhs_value.eq_str(rhs),
        Op::Ne => !lhs_value.eq_str(rhs),
        Op::Lt => lhs_value.cmp_num(rhs).map(|o| o < 0.0).unwrap_or(false),
        Op::Gt => lhs_value.cmp_num(rhs).map(|o| o > 0.0).unwrap_or(false),
        Op::Contains => lhs_value.as_text().contains(rhs),
    })
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Eq,
    Ne,
    Lt,
    Gt,
    Contains,
}

fn split_match_expr(expr: &str) -> Option<(&str, Op, &str)> {
    // Two-char ops first so we don't match `<` inside `<=`-like
    // strings (we don't support <=, but defensively handle ordering).
    for (tok, op) in [("==", Op::Eq), ("!=", Op::Ne)] {
        if let Some(idx) = expr.find(tok) {
            let lhs = expr[..idx].trim();
            let rhs = &expr[idx + tok.len()..];
            return Some((lhs, op, rhs));
        }
    }
    if let Some(idx) = expr.find(" contains ") {
        return Some((
            expr[..idx].trim(),
            Op::Contains,
            &expr[idx + " contains ".len()..],
        ));
    }
    for (tok, op) in [("<", Op::Lt), (">", Op::Gt)] {
        if let Some(idx) = expr.find(tok) {
            let lhs = expr[..idx].trim();
            let rhs = &expr[idx + tok.len()..];
            return Some((lhs, op, rhs));
        }
    }
    None
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        if (bytes[0] == b'"' && bytes[s.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[s.len() - 1] == b'\'')
        {
            return &s[1..s.len() - 1];
        }
    }
    s
}

#[derive(Debug, Clone)]
enum LhsValue {
    Number(f64),
    Text(String),
}

impl LhsValue {
    fn from_json(v: &serde_json::Value) -> Self {
        match v {
            serde_json::Value::Number(n) => LhsValue::Number(n.as_f64().unwrap_or(f64::NAN)),
            serde_json::Value::Bool(b) => LhsValue::Text(b.to_string()),
            serde_json::Value::Null => LhsValue::Text(String::new()),
            serde_json::Value::String(s) => LhsValue::Text(s.clone()),
            // Arrays and objects fall back to JSON text for `contains`
            // semantics — `$.tags contains "ready"` on `["ready","up"]`
            // matches because the literal token appears in the text.
            other => LhsValue::Text(other.to_string()),
        }
    }

    fn as_text(&self) -> String {
        match self {
            LhsValue::Number(n) => {
                // Avoid `200.0` vs `200` mismatches for integer-valued
                // numbers. JSON numbers without fractional part should
                // render the same as the decimal literal in `rhs`.
                if n.fract() == 0.0 && n.is_finite() {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            LhsValue::Text(s) => s.clone(),
        }
    }

    fn eq_str(&self, rhs: &str) -> bool {
        self.as_text() == rhs
    }

    fn cmp_num(&self, rhs: &str) -> Option<f64> {
        let r: f64 = rhs.parse().ok()?;
        let l: f64 = match self {
            LhsValue::Number(n) => *n,
            LhsValue::Text(s) => s.parse().ok()?,
        };
        Some(l - r)
    }
}

fn json_lookup(root: &serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let mut cur = root;
    for seg in path.split('.') {
        if seg.is_empty() {
            return None;
        }
        match cur {
            serde_json::Value::Object(map) => {
                cur = map.get(seg)?;
            }
            _ => return None,
        }
    }
    Some(cur.clone())
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_marker() {
        let s = "hello\nworld\n___INSPECT_HTTP_STATUS=200\n";
        let (body, code) = split_http_response(s);
        assert_eq!(body, "hello\nworld");
        assert_eq!(code, 200);
    }

    #[test]
    fn marker_missing_yields_zero_status() {
        let (body, code) = split_http_response("plain text");
        assert_eq!(body, "plain text");
        assert_eq!(code, 0);
    }

    #[test]
    fn http_match_status_eq() {
        assert!(eval_http_match("status == 200", "", 200).unwrap());
        assert!(!eval_http_match("status == 200", "", 500).unwrap());
        assert!(eval_http_match("status != 500", "", 200).unwrap());
    }

    #[test]
    fn http_match_status_lt_gt() {
        assert!(eval_http_match("status < 400", "", 200).unwrap());
        assert!(!eval_http_match("status < 400", "", 500).unwrap());
        assert!(eval_http_match("status > 199", "", 200).unwrap());
    }

    #[test]
    fn http_match_body_contains() {
        assert!(eval_http_match("body contains ready", "I am ready now", 200).unwrap());
        assert!(!eval_http_match("body contains nope", "I am ready now", 200).unwrap());
    }

    #[test]
    fn http_match_jsonpath() {
        let body = r#"{"status":"ok","count":42,"nested":{"flag":true}}"#;
        assert!(eval_http_match("$.status == ok", body, 200).unwrap());
        assert!(eval_http_match("$.status == \"ok\"", body, 200).unwrap());
        assert!(eval_http_match("$.count > 10", body, 200).unwrap());
        assert!(!eval_http_match("$.count < 10", body, 200).unwrap());
        assert!(eval_http_match("$.nested.flag == true", body, 200).unwrap());
        assert!(!eval_http_match("$.missing == x", body, 200).unwrap());
    }

    #[test]
    fn http_match_non_json_body_fails_jsonpath() {
        assert!(!eval_http_match("$.foo == bar", "not json", 200).unwrap());
    }

    #[test]
    fn http_match_unknown_lhs_errors() {
        assert!(eval_http_match("foo == bar", "", 200).is_err());
    }

    #[test]
    fn http_match_unparseable_errors() {
        assert!(eval_http_match("status", "", 200).is_err());
    }

    #[test]
    fn lhs_number_renders_integer_when_possible() {
        let v = LhsValue::Number(200.0);
        assert_eq!(v.as_text(), "200");
        let v2 = LhsValue::Number(2.5);
        assert_eq!(v2.as_text(), "2.5");
    }

    #[test]
    fn trimmed_for_audit_caps_long_lines() {
        let long = "a".repeat(200);
        let out = trimmed_for_audit(&long);
        assert!(out.chars().count() <= 121); // 120 + ellipsis
        assert!(out.ends_with('…'));
    }

    #[test]
    fn trimmed_for_audit_collapses_newlines() {
        let s = "  hello\nworld\r\n  ";
        assert_eq!(trimmed_for_audit(s), "hello world");
    }

    #[test]
    fn cmd_cmp_rejects_multiple_comparators() {
        let args = WatchArgs {
            selector: "n/s".into(),
            until_cmd: Some("true".into()),
            until_log: None,
            until_sql: None,
            until_http: None,
            equals: Some("a".into()),
            matches: None,
            gt: Some(1.0),
            lt: None,
            changes: false,
            stable_for: None,
            regex: false,
            psql_opts: None,
            r#match: None,
            insecure: false,
            interval: None,
            timeout: None,
            reason: None,
            verbose: false,
        };
        assert!(CmdCmp::from_args(&args).is_err());
    }

    #[test]
    fn cmd_cmp_default_is_exit_zero() {
        let args = WatchArgs {
            selector: "n/s".into(),
            until_cmd: Some("true".into()),
            until_log: None,
            until_sql: None,
            until_http: None,
            equals: None,
            matches: None,
            gt: None,
            lt: None,
            changes: false,
            stable_for: None,
            regex: false,
            psql_opts: None,
            r#match: None,
            insecure: false,
            interval: None,
            timeout: None,
            reason: None,
            verbose: false,
        };
        assert!(matches!(
            CmdCmp::from_args(&args).unwrap(),
            CmdCmp::ExitZero
        ));
    }
}
