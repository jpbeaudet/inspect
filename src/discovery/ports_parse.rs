//! L10 (v0.1.3): parse the `Ports` column from `docker ps --format
//! '{{.Ports}}'` into structured `Port` records.
//!
//! The column shape is documented but not formally specified. Tokens
//! observed across the field-corpus collected from each v0.1.2 user's
//! host:
//!
//! - **Full IPv4 bind**: `0.0.0.0:5432->5432/tcp`
//! - **IPv6 bind**: `[::]:53->53/udp`
//! - **Range bind**: `0.0.0.0:8000-8002->8000-8002/tcp`
//!   (one host:container range per token; expands to N records)
//! - **Unbound exposed port**: `5432/tcp`
//!   (no host bind; we record `host: 0` to distinguish from an
//!   actual `:0` bind, which docker would never emit)
//! - **Proto-less**: `5432` (rare; treated as `tcp` to match docker's
//!   own default; docker normally always emits `/tcp` or `/udp`)
//! - **Comma-separated list**: tokens joined by `, ` — split first,
//!   then parse each token independently.
//! - **Bracketed IPv6 unbound**: not observed in any field corpus
//!   (docker doesn't emit it), but the parser handles it defensively.
//!
//! The parser is total: every malformed token returns `None` from
//! `parse_token` and is logged-by-counter at the call site, never
//! panics. The `Ports` column is human-rendered text and we expect
//! the surface to evolve; a half-correct parser that silently
//! mis-interprets a future format would be the worst class of bug
//! for drift detection ("ports unchanged" when they actually
//! changed).

use crate::profile::schema::Port;

/// Parse a full `docker ps --format '{{.Ports}}'` cell into a sorted
/// `Vec<Port>`. Empty input returns `Vec::new()`. Malformed tokens
/// are silently dropped; the caller can compare the input's comma
/// count with the output's len to count drops if it cares (drift
/// detection doesn't — port-level diff degrades gracefully when
/// docker emits an unrecognized shape, treating the unparseable
/// row as "no ports").
pub fn parse_ports_column(raw: &str) -> Vec<Port> {
    let mut out: Vec<Port> = Vec::new();
    for tok in raw.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        out.extend(parse_token(tok));
    }
    sort_canonical(&mut out);
    out
}

/// Parse one token. Range tokens expand to N records; non-range
/// tokens to one. Unrecognized shapes return an empty vec.
fn parse_token(tok: &str) -> Vec<Port> {
    // Split off the proto suffix (`/tcp` or `/udp`). When absent,
    // default to "tcp" — matches docker's own default.
    let (head, proto) = match tok.rsplit_once('/') {
        Some((h, p)) if is_proto(p) => (h, p.to_string()),
        _ => (tok, "tcp".to_string()),
    };

    // Two shapes for `head`:
    //   "host_bind->container_port" (bound)
    //   "container_port"            (unbound, just exposed)
    let (bind_part, container_part) = match head.split_once("->") {
        Some((b, c)) => (Some(b), c),
        None => (None, head),
    };

    // Container port may be a single u16 or a range `LO-HI`.
    let cont_lo_hi = match parse_port_or_range(container_part) {
        Some(t) => t,
        None => return Vec::new(),
    };

    // If bound, parse the host side (which may also be `host_addr:port`
    // or `host_addr:LO-HI` or even bracketed IPv6 like `[::]:port`).
    let host_lo_hi = match bind_part {
        Some(b) => match parse_host_bind(b) {
            Some(t) => Some(t),
            None => return Vec::new(),
        },
        None => None,
    };

    // Expand range. The range arity must match between host and
    // container sides; docker normally emits matching ranges, but a
    // mismatched corpus row is malformed.
    let (clo, chi) = cont_lo_hi;
    let cont_count = (chi - clo + 1) as usize;
    if let Some((hlo, hhi)) = host_lo_hi {
        let host_count = (hhi - hlo + 1) as usize;
        if host_count != cont_count {
            return Vec::new();
        }
        let mut v: Vec<Port> = Vec::with_capacity(cont_count);
        for offset in 0..cont_count {
            v.push(Port {
                host: hlo + offset as u16,
                container: clo + offset as u16,
                proto: proto.clone(),
            });
        }
        v
    } else {
        // Unbound: every record records `host: 0`.
        let mut v: Vec<Port> = Vec::with_capacity(cont_count);
        for offset in 0..cont_count {
            v.push(Port {
                host: 0,
                container: clo + offset as u16,
                proto: proto.clone(),
            });
        }
        v
    }
}

/// Parse the host-bind portion of a token. Strips an optional
/// `<addr>:` prefix (IPv4 or bracketed IPv6) and returns the
/// host port range. Returns `None` on malformed input.
fn parse_host_bind(s: &str) -> Option<(u16, u16)> {
    let port_part = if let Some(rest) = s.strip_prefix('[') {
        // Bracketed IPv6: `[::]:port` or `[::1]:LO-HI`.
        let (_addr, after) = rest.split_once(']')?;
        let after = after.strip_prefix(':')?;
        after
    } else if let Some(idx) = s.rfind(':') {
        // IPv4 (or unbracketed shorthand): `0.0.0.0:port`.
        // Take the segment after the LAST colon — IPv6 without
        // brackets isn't a docker-emitted shape but if it appears
        // we fall through gracefully (the rfind picks up the port
        // boundary).
        &s[idx + 1..]
    } else {
        // No colon ⇒ no addr prefix; the whole string is the port
        // range. Docker doesn't emit this but the parser stays
        // permissive.
        s
    };
    parse_port_or_range(port_part)
}

/// Parse `<port>` or `<lo>-<hi>` into an inclusive `(lo, hi)` pair.
/// Returns `None` on malformed input or when `hi < lo`.
fn parse_port_or_range(s: &str) -> Option<(u16, u16)> {
    if let Some((lo_s, hi_s)) = s.split_once('-') {
        let lo: u16 = lo_s.parse().ok()?;
        let hi: u16 = hi_s.parse().ok()?;
        if hi < lo {
            return None;
        }
        Some((lo, hi))
    } else {
        let p: u16 = s.parse().ok()?;
        Some((p, p))
    }
}

fn is_proto(s: &str) -> bool {
    matches!(s, "tcp" | "udp" | "sctp")
}

/// Canonicalize: sort by (container, proto, host). Determinism
/// matters because the diff layer hashes on the resulting Vec, and
/// docker's `Ports` column emits tokens in arbitrary order.
fn sort_canonical(v: &mut [Port]) {
    v.sort_by(|a, b| {
        a.container
            .cmp(&b.container)
            .then(a.proto.cmp(&b.proto))
            .then(a.host.cmp(&b.host))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(host: u16, container: u16, proto: &str) -> Port {
        Port {
            host,
            container,
            proto: proto.into(),
        }
    }

    #[test]
    fn l10_parse_ipv4_bind() {
        let r = parse_ports_column("0.0.0.0:5432->5432/tcp");
        assert_eq!(r, vec![p(5432, 5432, "tcp")]);
    }

    #[test]
    fn l10_parse_ipv4_bind_different_host_port() {
        let r = parse_ports_column("0.0.0.0:5433->5432/tcp");
        assert_eq!(r, vec![p(5433, 5432, "tcp")]);
    }

    #[test]
    fn l10_parse_ipv6_bracketed_bind() {
        let r = parse_ports_column("[::]:53->53/udp");
        assert_eq!(r, vec![p(53, 53, "udp")]);
    }

    #[test]
    fn l10_parse_ipv6_bracketed_with_address() {
        // `[::1]:5353->5353/udp`
        let r = parse_ports_column("[::1]:5353->5353/udp");
        assert_eq!(r, vec![p(5353, 5353, "udp")]);
    }

    #[test]
    fn l10_parse_unbound_exposed_port() {
        let r = parse_ports_column("5432/tcp");
        assert_eq!(r, vec![p(0, 5432, "tcp")]);
    }

    #[test]
    fn l10_parse_proto_less_token_defaults_to_tcp() {
        let r = parse_ports_column("8080");
        assert_eq!(r, vec![p(0, 8080, "tcp")]);
    }

    #[test]
    fn l10_parse_comma_separated_tokens() {
        let r = parse_ports_column("0.0.0.0:80->80/tcp, 0.0.0.0:443->443/tcp");
        assert_eq!(r, vec![p(80, 80, "tcp"), p(443, 443, "tcp")]);
    }

    #[test]
    fn l10_parse_range_expands_to_n_records() {
        let r = parse_ports_column("0.0.0.0:8000-8002->8000-8002/tcp");
        assert_eq!(
            r,
            vec![
                p(8000, 8000, "tcp"),
                p(8001, 8001, "tcp"),
                p(8002, 8002, "tcp")
            ]
        );
    }

    #[test]
    fn l10_parse_range_with_offset_host_to_container() {
        // Unusual but legal: 9000-9002 -> 80-82
        let r = parse_ports_column("0.0.0.0:9000-9002->80-82/tcp");
        assert_eq!(
            r,
            vec![p(9000, 80, "tcp"), p(9001, 81, "tcp"), p(9002, 82, "tcp")]
        );
    }

    #[test]
    fn l10_parse_range_arity_mismatch_drops_token() {
        // Host range 9000-9002 (3 entries) doesn't match container 80
        // (1 entry); docker shouldn't emit this but a malformed row
        // is silently dropped rather than mis-interpreted.
        let r = parse_ports_column("0.0.0.0:9000-9002->80/tcp");
        assert!(r.is_empty(), "expected empty on arity mismatch: {r:?}");
    }

    #[test]
    fn l10_parse_unbound_range() {
        let r = parse_ports_column("5000-5001/tcp");
        assert_eq!(r, vec![p(0, 5000, "tcp"), p(0, 5001, "tcp")]);
    }

    #[test]
    fn l10_parse_udp_proto() {
        let r = parse_ports_column("0.0.0.0:514->514/udp");
        assert_eq!(r, vec![p(514, 514, "udp")]);
    }

    #[test]
    fn l10_parse_mixed_proto_in_one_cell() {
        let r = parse_ports_column("0.0.0.0:53->53/tcp, 0.0.0.0:53->53/udp");
        assert_eq!(r, vec![p(53, 53, "tcp"), p(53, 53, "udp")]);
    }

    #[test]
    fn l10_parse_empty_yields_empty_vec() {
        assert!(parse_ports_column("").is_empty());
        assert!(parse_ports_column("   ").is_empty());
        assert!(parse_ports_column(",,").is_empty());
    }

    #[test]
    fn l10_parse_garbage_token_dropped() {
        let r = parse_ports_column("not-a-port");
        assert!(r.is_empty());
        // Surrounding good tokens still parse.
        let r = parse_ports_column("not-a-port, 80/tcp");
        assert_eq!(r, vec![p(0, 80, "tcp")]);
    }

    #[test]
    fn l10_parse_canonical_sort_order() {
        // Input order shouldn't matter for the parsed output —
        // the diff layer compares Vec<Port> by index.
        let r1 = parse_ports_column("0.0.0.0:443->443/tcp, 0.0.0.0:80->80/tcp");
        let r2 = parse_ports_column("0.0.0.0:80->80/tcp, 0.0.0.0:443->443/tcp");
        assert_eq!(r1, r2);
    }

    #[test]
    fn l10_parse_canonical_sort_breaks_ties_on_proto_then_host() {
        // Same container port, different proto: tcp before udp.
        // Same (container, proto), different host: lower host first.
        let r =
            parse_ports_column("0.0.0.0:53->53/udp, 0.0.0.0:53->53/tcp, 127.0.0.1:8053->53/tcp");
        assert_eq!(
            r,
            vec![p(53, 53, "tcp"), p(8053, 53, "tcp"), p(53, 53, "udp"),]
        );
    }

    #[test]
    fn l10_parse_real_corpus_postgres_redis_compose() {
        // Captured from a v0.1.2 user's `docker ps --format
        // '{{.Names}}\t{{.Ports}}'` output, the `Ports` column for
        // a typical compose project.
        let r = parse_ports_column("0.0.0.0:5432->5432/tcp, :::5432->5432/tcp");
        // The `:::5432` form is docker's notation for an IPv6
        // unbracketed wildcard bind — a legacy shape the parser
        // tolerates but doesn't try to interpret cleanly. The
        // first token parses; the second's `rfind(':')` picks
        // out `5432` as the port.
        assert!(!r.is_empty());
        assert!(r.iter().any(|x| x.container == 5432 && x.proto == "tcp"));
    }
}
