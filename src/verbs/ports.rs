//! `inspect ports <sel>` — list listening ports.
//!
//! Strategy: prefer `ss -tlnp`, fall back to `netstat -tlnp` if the host
//! profile says `ss` isn't available. For container selectors, `docker
//! port <name>` is more honest (returns the published mapping).

use anyhow::{anyhow, Result};

use crate::cli::PortsArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, Renderer};
use crate::verbs::quote::shquote;

/// F7.3 (v0.1.3): inclusive port filter.
#[derive(Debug, Clone, Copy)]
enum PortFilter {
    None,
    Single(u16),
    Range(u16, u16),
}

impl PortFilter {
    fn from_args(args: &PortsArgs) -> Result<Self> {
        if let Some(p) = args.port {
            return Ok(PortFilter::Single(p));
        }
        if let Some(r) = &args.port_range {
            let (lo_s, hi_s) = r
                .split_once('-')
                .ok_or_else(|| anyhow!("--port-range must be 'LO-HI', got '{r}'"))?;
            let lo: u16 = lo_s
                .trim()
                .parse()
                .map_err(|_| anyhow!("--port-range LO must be a u16, got '{lo_s}'"))?;
            let hi: u16 = hi_s
                .trim()
                .parse()
                .map_err(|_| anyhow!("--port-range HI must be a u16, got '{hi_s}'"))?;
            if hi < lo {
                return Err(anyhow!("--port-range LO ({lo}) must be <= HI ({hi})"));
            }
            return Ok(PortFilter::Range(lo, hi));
        }
        Ok(PortFilter::None)
    }

    fn matches_line(&self, line: &str) -> bool {
        match self {
            PortFilter::None => true,
            PortFilter::Single(p) => line_has_port(line, |q| q == *p),
            PortFilter::Range(lo, hi) => line_has_port(line, |q| q >= *lo && q <= *hi),
        }
    }
}

/// Scan whitespace tokens in `line` for any `:N` or `N/proto` shape
/// where `N` is a u16, then test the predicate. Picks up both the
/// `ss -tlnp` form (`0.0.0.0:8200`) and the `docker port` form
/// (`8200/tcp -> 0.0.0.0:8200`).
fn line_has_port(line: &str, mut pred: impl FnMut(u16) -> bool) -> bool {
    for tok in line.split(|c: char| c.is_whitespace() || c == ',') {
        // colon-suffix: 0.0.0.0:8200, [::]:8200
        if let Some(idx) = tok.rfind(':') {
            let tail = &tok[idx + 1..];
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(p) = digits.parse::<u16>() {
                if pred(p) {
                    return true;
                }
            }
        }
        // slash-suffix: 8200/tcp
        if let Some(idx) = tok.find('/') {
            let head = &tok[..idx];
            if let Ok(p) = head.parse::<u16>() {
                if pred(p) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn run(args: PortsArgs) -> Result<ExitKind> {
    let filter = PortFilter::from_args(&args)?;
    let (runner, nses, targets) = plan(&args.selector)?;
    let mut renderer = Renderer::new();
    let mut count = 0usize;

    for step in iter_steps(&nses, &targets) {
        let cmd = match step.service() {
            Some(svc) => format!("docker port {} 2>/dev/null || true", shquote(svc)),
            None => {
                let prefer_ss = step
                    .ns
                    .profile
                    .as_ref()
                    .map(|p| p.remote_tooling.ss)
                    .unwrap_or(true);
                if prefer_ss {
                    "ss -tlnp 2>/dev/null || netstat -tlnp 2>/dev/null".to_string()
                } else {
                    "netstat -tlnp 2>/dev/null".to_string()
                }
            }
        };
        let out = runner.run(
            &step.ns.namespace,
            &step.ns.target,
            &cmd,
            RunOpts::with_timeout(15),
        )?;
        if !out.ok() {
            if !args.format.is_json() {
                eprintln!(
                    "{}: ports failed (exit {}): {}",
                    step.ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        for line in out.stdout.lines() {
            // F7.3: server-side port filter. Skip rows that do not
            // match before they reach DATA / JSON output.
            if !filter.matches_line(line) {
                continue;
            }
            count += 1;
            renderer.data_line(format!(
                "{ns}{svc} | {line}",
                ns = step.ns.namespace,
                svc = step.service().map(|s| format!("/{s}")).unwrap_or_default()
            ));
            renderer.push_row(
                &Envelope::new(&step.ns.namespace, "network", "ports")
                    .with_service(step.service().unwrap_or("_"))
                    .put("line", line),
            );
        }
    }
    renderer.summary(format!("{count} port-line(s)"));
    renderer.quiet(args.format.quiet);
    let __fmt = args.format.resolve()?;
    renderer.dispatch(&__fmt)?;
    Ok(ExitKind::Success)
}
