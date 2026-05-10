//! `inspect ports <sel>` — list listening ports.
//!
//! Strategy: for a host selector (no service portion), probe both
//! TCP and UDP via `ss -[tu]lnp` (preferred) with `netstat -[tu]lnp`
//! fallback; for a container selector, `docker port <name>` is
//! more honest (returns the published mapping with proto already
//! tagged). adds the UDP probe + a `--proto tcp|udp|all`
//! filter so DNS forwarders / mDNS responders / syslog receivers /
//! IPSec daemons / WireGuard endpoints — invisible earlier — surface
//! through the same verb operators already use for TCP.

use anyhow::{anyhow, Result};

use crate::cli::PortsArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, Renderer};
use crate::verbs::quote::shquote;

/// Inclusive port filter.
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

/// Which proto axis (or axes) the verb scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtoAxis {
    Tcp,
    Udp,
    All,
}

impl ProtoAxis {
    fn parse(raw: &str) -> Self {
        // Clap value_parser already validated the input; treat
        // anything else as the safe default `all` so a future
        // typo here doesn't silently drop probes.
        match raw.to_ascii_lowercase().as_str() {
            "tcp" => ProtoAxis::Tcp,
            "udp" => ProtoAxis::Udp,
            _ => ProtoAxis::All,
        }
    }

    fn includes_tcp(self) -> bool {
        matches!(self, ProtoAxis::Tcp | ProtoAxis::All)
    }

    fn includes_udp(self) -> bool {
        matches!(self, ProtoAxis::Udp | ProtoAxis::All)
    }

    /// Build the per-host probe command. We run TCP and UDP probes
    /// in series in one ssh round-trip (separated by `&&` so a
    /// missing `ss` falls cleanly through to `netstat`); each
    /// probe's output is bracketed by a `--- <proto> ---` marker so
    /// the local parser can attribute each line to a proto.
    fn build_probe_cmd(self, prefer_ss: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.includes_tcp() {
            parts.push(probe_one(prefer_ss, "tcp", "t"));
        }
        if self.includes_udp() {
            parts.push(probe_one(prefer_ss, "udp", "u"));
        }
        parts.join("; ")
    }
}

fn probe_one(prefer_ss: bool, proto_label: &str, proto_letter: &str) -> String {
    let ss_cmd = format!("ss -{proto_letter}lnp 2>/dev/null");
    let netstat_cmd = format!("netstat -{proto_letter}lnp 2>/dev/null");
    let body = if prefer_ss {
        format!("{ss_cmd} || {netstat_cmd}")
    } else {
        netstat_cmd
    };
    format!("echo '--- {proto_label} ---'; {body} || true",)
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

/// Scan a `docker port <ctr>` line for its proto tag.
/// Output looks like `8200/tcp -> 0.0.0.0:8200` or `53/udp ->
/// 0.0.0.0:53`. Returns `"tcp"` when no `/proto` suffix is found
/// (some docker versions omit the proto for tcp).
fn line_proto_for_docker(line: &str) -> &'static str {
    if line.contains("/udp") {
        "udp"
    } else {
        "tcp"
    }
}

pub fn run(args: PortsArgs) -> Result<ExitKind> {
    let filter = PortFilter::from_args(&args)?;
    let axis = ProtoAxis::parse(&args.proto);
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
                axis.build_probe_cmd(prefer_ss)
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
                crate::tee_eprintln!(
                    "{}: ports failed (exit {}): {}",
                    step.ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }

        // Walk the output. For host probes we attribute each
        // line to the proto announced by the most recent
        // `--- tcp ---` / `--- udp ---` marker. For container
        // probes we read the proto out of the `<port>/<proto>`
        // shape. The same `--port` / `--port-range` filter applies
        // independently of proto.
        let mut current_proto: &str = "tcp";
        let is_host_probe = step.service().is_none();
        for line in out.stdout.lines() {
            if is_host_probe {
                if let Some(proto) = parse_marker(line) {
                    current_proto = proto;
                    continue;
                }
            }
            if !filter.matches_line(line) {
                continue;
            }
            let proto = if is_host_probe {
                current_proto
            } else {
                line_proto_for_docker(line)
            };
            // Client-side proto filter is needed for the
            // docker-port path (we can't push --proto down into
            // `docker port`); the host-probe path has already been
            // narrowed by the build_probe_cmd command set.
            if !is_host_probe && !proto_matches(axis, proto) {
                continue;
            }
            count += 1;
            renderer.data_line(format!(
                "{ns}{svc} | [{proto}] {line}",
                ns = step.ns.namespace,
                svc = step.service().map(|s| format!("/{s}")).unwrap_or_default(),
                proto = proto,
                line = line,
            ));
            renderer.push_row(
                &Envelope::new(&step.ns.namespace, "network", "ports")
                    .with_service(step.service().unwrap_or("_"))
                    .put("proto", proto)
                    .put("line", line),
            );
        }
    }
    renderer.summary(format!("{count} port-line(s)"));
    renderer.quiet(args.format.quiet);
    let fmt = args.format.resolve()?;
    let select = args.format.select_filter()?;
    renderer.dispatch(&fmt, select)
}

/// Recognize the `--- tcp ---` / `--- udp ---` markers
/// emitted by `build_probe_cmd`. Returns the proto string when the
/// line is a marker, `None` otherwise.
fn parse_marker(line: &str) -> Option<&'static str> {
    let l = line.trim();
    if l == "--- tcp ---" {
        Some("tcp")
    } else if l == "--- udp ---" {
        Some("udp")
    } else {
        None
    }
}

fn proto_matches(axis: ProtoAxis, proto: &str) -> bool {
    match axis {
        ProtoAxis::Tcp => proto == "tcp",
        ProtoAxis::Udp => proto == "udp",
        ProtoAxis::All => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l9_proto_axis_parse_defaults_to_all() {
        assert_eq!(ProtoAxis::parse("tcp"), ProtoAxis::Tcp);
        assert_eq!(ProtoAxis::parse("udp"), ProtoAxis::Udp);
        assert_eq!(ProtoAxis::parse("all"), ProtoAxis::All);
        // Anything else (clap should reject upstream, but be safe):
        assert_eq!(ProtoAxis::parse("garbage"), ProtoAxis::All);
        assert_eq!(ProtoAxis::parse("TCP"), ProtoAxis::Tcp);
    }

    #[test]
    fn l9_probe_cmd_includes_both_protos_for_all() {
        let cmd = ProtoAxis::All.build_probe_cmd(true);
        assert!(cmd.contains("--- tcp ---"));
        assert!(cmd.contains("--- udp ---"));
        assert!(cmd.contains("ss -tlnp"));
        assert!(cmd.contains("ss -ulnp"));
    }

    #[test]
    fn l9_probe_cmd_includes_only_tcp_when_narrowed() {
        let cmd = ProtoAxis::Tcp.build_probe_cmd(true);
        assert!(cmd.contains("--- tcp ---"));
        assert!(!cmd.contains("--- udp ---"));
        assert!(!cmd.contains("-ulnp"));
    }

    #[test]
    fn l9_probe_cmd_includes_only_udp_when_narrowed() {
        let cmd = ProtoAxis::Udp.build_probe_cmd(true);
        assert!(cmd.contains("--- udp ---"));
        assert!(!cmd.contains("--- tcp ---"));
        assert!(!cmd.contains("-tlnp"));
    }

    #[test]
    fn l9_probe_cmd_uses_netstat_only_when_ss_absent() {
        let cmd = ProtoAxis::All.build_probe_cmd(false);
        assert!(!cmd.contains("ss -"));
        assert!(cmd.contains("netstat -tlnp"));
        assert!(cmd.contains("netstat -ulnp"));
    }

    #[test]
    fn l9_parse_marker_recognizes_both_protos() {
        assert_eq!(parse_marker("--- tcp ---"), Some("tcp"));
        assert_eq!(parse_marker("--- udp ---"), Some("udp"));
        assert_eq!(parse_marker("LISTEN 0 4096 ..."), None);
        assert_eq!(parse_marker("--- foo ---"), None);
    }

    #[test]
    fn l9_line_proto_for_docker_handles_udp_suffix() {
        assert_eq!(line_proto_for_docker("53/udp -> 0.0.0.0:53"), "udp");
        assert_eq!(line_proto_for_docker("8200/tcp -> 0.0.0.0:8200"), "tcp");
        // No suffix → safe-default tcp (some docker versions omit it).
        assert_eq!(line_proto_for_docker("8200 -> 0.0.0.0:8200"), "tcp");
    }
}
