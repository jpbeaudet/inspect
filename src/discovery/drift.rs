//! Async drift detection.
//!
//! Per the bible (§5.1), every command may trigger a non-blocking drift check
//! against the cached profile. We run a *cheap* fingerprint probe (just
//! `docker ps` IDs + image tags + names) on a background thread; if it
//! diverges from the cached fingerprint, we write a drift marker. Subsequent
//! commands can surface the marker as a `NEXT:` hint.
//!
//! v0.1.2 (B4): in addition to the SHA-256 fingerprints, we now emit a
//! structured `DriftDiff` (containers added / removed / image-changed). The
//! fingerprints are still the source of truth for "drifted vs fresh"; the
//! diff is a human-readable explanation of *what* changed.
//!
//! L10 (v0.1.3): port-level entries. `DriftDiff` gains a `port_changes:
//! Vec<PortChange>` field with four kinds — `Added`, `Removed`, `Bind`
//! (same container_port + proto, different host), `Proto` (same
//! container_port + host, different proto). The cheap probe now also
//! captures `{{.Ports}}` per container; the parser in
//! `discovery::ports_parse` handles every shape collected from the field
//! corpus (IPv4 + IPv6 binds, ranges, unbound exposed ports,
//! comma-separated lists). UDP port changes between snapshots flow
//! through the same parser (L9 made `proto: "udp"` first-class on the
//! cached side; the parser already understood `/udp` tokens).

use std::time::Duration;

use crate::discovery::ports_parse::parse_ports_column;
use crate::profile::cache::{clear_drift_marker, load_profile, write_drift_marker};
use crate::profile::schema::{Port, Profile};
use crate::ssh::{run_remote, RunOpts, SshTarget};

/// One container row, normalized so cached and live data are
/// apples-to-apples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftRow {
    pub id: String,
    pub name: String,
    pub image: String,
    /// L10 (v0.1.3): structured port set for the container, parsed
    /// from `docker ps`'s `Ports` column on the live side and
    /// projected from `Service.ports` on the cached side. Sorted
    /// canonically by `(container, proto, host)` so the diff layer
    /// can compare by index without reorder.
    pub ports: Vec<Port>,
}

impl DriftRow {
    fn fingerprint_line(&self) -> String {
        // Stable, tab-separated. Order: id\tname\timage\tports. Names
        // matter for the human diff but the id is the primary key for
        // sameness — if ids match but image (or ports) changed,
        // that's a "changed" entry, not add+remove. L10 (v0.1.3):
        // port set folded into the fingerprint so a port-only change
        // (e.g. 5432:5432 -> 5433:5432) flips the fingerprint and
        // surfaces a drift signal — pre-L10 it was silent.
        let mut s = format!("{}\t{}\t{}", self.id, self.name, self.image);
        for p in &self.ports {
            s.push('\t');
            s.push_str(&format!("{}:{}/{}", p.host, p.container, p.proto));
        }
        s
    }
}

/// One container whose image changed in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftContainerChange {
    pub name: String,
    pub from_image: String,
    pub to_image: String,
}

/// L10 (v0.1.3): the kind of port-level change between two snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortChangeKind {
    /// Port present in live, absent in cached.
    Added,
    /// Port present in cached, absent in live.
    Removed,
    /// Same `(container_port, proto)` in both, but `host` differs.
    /// Most common when an operator moves a service to a different
    /// host port to dodge a collision (`5432:5432` → `5433:5432`).
    Bind,
    /// Same `(host, container_port)` in both, but `proto` differs.
    /// E.g. a DNS service flipped from TCP to UDP listener.
    Proto,
}

impl PortChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PortChangeKind::Added => "added",
            PortChangeKind::Removed => "removed",
            PortChangeKind::Bind => "bind",
            PortChangeKind::Proto => "proto",
        }
    }
}

/// L10 (v0.1.3): one port-level change attributed to a container.
/// `before` / `after` carry the structured payloads — `Added` has
/// `before: None`, `Removed` has `after: None`, both `Bind` and
/// `Proto` have both populated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortChange {
    pub container: String,
    pub kind: PortChangeKind,
    pub before: Option<Port>,
    pub after: Option<Port>,
}

/// Structured human-readable diff between cached and live container sets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriftDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<DriftContainerChange>,
    /// L10 (v0.1.3): per-port differences within containers that
    /// exist in both snapshots. Containers that are entirely added
    /// or removed surface in `added` / `removed` and do NOT also
    /// fan their per-port deltas into `port_changes` (that would be
    /// double-counting; the container-level entry implies its
    /// ports moved with it).
    pub port_changes: Vec<PortChange>,
}

impl DriftDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.port_changes.is_empty()
    }
}

/// Synchronous drift check. Used in tests and from `inspect setup --check-drift`.
pub fn run_drift_check(namespace: &str, target: &SshTarget) -> anyhow::Result<DriftStatus> {
    let cached = match load_profile(namespace)? {
        Some(p) => p,
        None => return Ok(DriftStatus::NoCache),
    };
    let live = match cheap_rows(namespace, target) {
        Ok(rows) => rows,
        Err(_) => return Ok(DriftStatus::ProbeFailed),
    };
    let baseline = baseline_rows(&cached);

    let cheap_fp = fingerprint(&live);
    let baseline_fp = fingerprint(&baseline);

    if cheap_fp == baseline_fp {
        clear_drift_marker(namespace);
        Ok(DriftStatus::Fresh)
    } else {
        write_drift_marker(namespace, &cheap_fp, &baseline_fp)?;
        let diff = compute_diff(&baseline, &live);
        Ok(DriftStatus::Drifted {
            current: cheap_fp,
            cached: baseline_fp,
            diff,
        })
    }
}

/// Drift outcome.
#[derive(Debug, Clone)]
pub enum DriftStatus {
    NoCache,
    ProbeFailed,
    Fresh,
    Drifted {
        current: String,
        cached: String,
        diff: DriftDiff,
    },
}

/// Cheap probe of the live host. Container ids + names + images +
/// L10 ports. Sorted by container id for stable hashing.
fn cheap_rows(namespace: &str, target: &SshTarget) -> anyhow::Result<Vec<DriftRow>> {
    // Docker's Go template needs the literal `\t` to make tabs; the shell
    // single-quote here passes the backslash-t through to docker untouched.
    // L10 (v0.1.3): the `{{.Ports}}` column is appended so the probe
    // captures port-level state in the same single ssh round-trip;
    // the column itself may be empty (containers without exposed
    // ports) and that case parses to `Vec::new()`.
    let cmd = "docker ps --format '{{.ID}}\\t{{.Names}}\\t{{.Image}}\\t{{.Ports}}' 2>/dev/null";
    let out = run_remote(
        namespace,
        target,
        cmd,
        RunOpts {
            timeout: Some(Duration::from_secs(8)),
            stdin: None,
            tty: false,
        },
    )?;
    if !out.ok() {
        anyhow::bail!("cheap fingerprint probe exited {}", out.exit_code);
    }
    Ok(parse_docker_ps(&out.stdout))
}

fn parse_docker_ps(s: &str) -> Vec<DriftRow> {
    let mut rows: Vec<DriftRow> = s
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let mut it = l.splitn(4, '\t');
            let id = it.next()?.trim().to_string();
            let name = it.next()?.trim().to_string();
            let image = it.next()?.trim().to_string();
            // L10: 4th column is `{{.Ports}}`. Pre-L10 cached
            // probe data without this column produces `it.next() ==
            // None`, which we handle by parsing an empty string
            // (yielding `ports: vec![]`) — the diff degrades to
            // pre-L10 behavior on legacy cache rows.
            let ports_raw = it.next().unwrap_or("").trim();
            if id.is_empty() {
                return None;
            }
            Some(DriftRow {
                id,
                name,
                image,
                ports: parse_ports_column(ports_raw),
            })
        })
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

/// Project the cached profile down to the same shape as [`cheap_rows`].
/// L10 (v0.1.3): the `Service.ports: Vec<Port>` field already carries
/// the structured port set the live probe surfaces — re-sort here so
/// the canonical (container, proto, host) order matches the parser's
/// output.
fn baseline_rows(p: &Profile) -> Vec<DriftRow> {
    let mut rows: Vec<DriftRow> = p
        .services
        .iter()
        .filter(|s| matches!(s.kind, crate::profile::schema::ServiceKind::Container))
        .map(|s| {
            let mut ports: Vec<Port> = s.ports.clone();
            // Match the canonical sort applied by the parser so the
            // diff layer compares Vec<Port> by index without reorder.
            ports.sort_by(|a, b| {
                a.container
                    .cmp(&b.container)
                    .then(a.proto.cmp(&b.proto))
                    .then(a.host.cmp(&b.host))
            });
            DriftRow {
                id: s.container_id.clone().unwrap_or_default(),
                name: s.name.clone(),
                image: s.image.clone().unwrap_or_default(),
                ports,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

/// SHA-256 over the canonical newline-joined row representation.
fn fingerprint(rows: &[DriftRow]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for r in rows {
        h.update(r.fingerprint_line().as_bytes());
        h.update(b"\n");
    }
    let bytes = h.finalize();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compute a human-readable diff between cached (baseline) and live
/// (current) container sets. Container id is the primary key:
///
/// * id present in both, image unchanged → no-op
/// * id present in both, image changed   → `changed`
/// * id only in live                     → `added`
/// * id only in cached                   → `removed`
///
/// Container ids that are empty (legacy cache rows that didn't capture
/// the id) are matched on name only — better than dropping them.
pub(crate) fn compute_diff(cached: &[DriftRow], live: &[DriftRow]) -> DriftDiff {
    use std::collections::{HashMap, HashSet};

    let mut diff = DriftDiff::default();

    let mut cached_by_id: HashMap<&str, &DriftRow> = HashMap::new();
    let mut cached_by_name: HashMap<&str, &DriftRow> = HashMap::new();
    for r in cached {
        if !r.id.is_empty() {
            cached_by_id.insert(r.id.as_str(), r);
        }
        if !r.name.is_empty() {
            cached_by_name.insert(r.name.as_str(), r);
        }
    }

    let mut consumed_ids: HashSet<&str> = HashSet::new();
    let mut consumed_names: HashSet<&str> = HashSet::new();

    for r in live {
        let prev = cached_by_id
            .get(r.id.as_str())
            .copied()
            .or_else(|| cached_by_name.get(r.name.as_str()).copied());
        match prev {
            Some(p) => {
                if !p.id.is_empty() {
                    consumed_ids.insert(p.id.as_str());
                }
                if !p.name.is_empty() {
                    consumed_names.insert(p.name.as_str());
                }
                if p.image != r.image {
                    diff.changed.push(DriftContainerChange {
                        name: pick_label(r),
                        from_image: p.image.clone(),
                        to_image: r.image.clone(),
                    });
                }
                // L10 (v0.1.3): per-port diff for containers present
                // in both snapshots. Containers that are entirely
                // added/removed surface in the container-level lists
                // and DO NOT also fan their per-port deltas into
                // `port_changes` — that would double-count the
                // operator's intent.
                let label = pick_label(r);
                let mut per_container = compute_port_diff(&label, &p.ports, &r.ports);
                diff.port_changes.append(&mut per_container);
            }
            None => diff.added.push(pick_label(r)),
        }
    }

    for r in cached {
        let id_taken = !r.id.is_empty() && consumed_ids.contains(r.id.as_str());
        let name_taken = !r.name.is_empty() && consumed_names.contains(r.name.as_str());
        if id_taken || name_taken {
            continue;
        }
        diff.removed.push(pick_label(r));
    }

    diff.added.sort();
    diff.removed.sort();
    diff.changed.sort_by(|a, b| a.name.cmp(&b.name));
    diff.port_changes.sort_by(|a, b| {
        a.container.cmp(&b.container).then_with(|| {
            let ka = port_change_key(a);
            let kb = port_change_key(b);
            ka.cmp(&kb)
        })
    });
    diff
}

/// L10 (v0.1.3): compute port-level changes for a single container
/// pair (cached vs live). The four kinds — Added / Removed / Bind /
/// Proto — are produced by:
///
/// 1. Walking ports keyed on `(container_port, proto)`. Same key in
///    both with same host ⇒ no change. Same key in both with
///    different host ⇒ `Bind`. Key only in cached ⇒ candidate
///    `Removed`. Key only in live ⇒ candidate `Added`.
/// 2. A coalescing pass: when a candidate `Removed` and a candidate
///    `Added` for the same container share the same `(container_port,
///    host)` tuple but differ only in proto, fold them into one
///    `Proto` entry (the operator's intent was "flip this port's
///    transport", not "remove one and add another").
fn compute_port_diff(container: &str, cached: &[Port], live: &[Port]) -> Vec<PortChange> {
    use std::collections::BTreeMap;

    // (container_port, proto) → host. Multiple bindings of the same
    // (cport, proto) on different hosts are unusual; we key the
    // primary diff on (cport, proto) and fall back to all-hosts when
    // there's a tie.
    let mut cmap: BTreeMap<(u16, String), u16> = BTreeMap::new();
    for p in cached {
        cmap.insert((p.container, p.proto.clone()), p.host);
    }
    let mut lmap: BTreeMap<(u16, String), u16> = BTreeMap::new();
    for p in live {
        lmap.insert((p.container, p.proto.clone()), p.host);
    }

    let mut adds: Vec<PortChange> = Vec::new();
    let mut removes: Vec<PortChange> = Vec::new();
    let mut binds: Vec<PortChange> = Vec::new();

    for (key, lhost) in &lmap {
        match cmap.get(key) {
            None => adds.push(PortChange {
                container: container.to_string(),
                kind: PortChangeKind::Added,
                before: None,
                after: Some(Port {
                    host: *lhost,
                    container: key.0,
                    proto: key.1.clone(),
                }),
            }),
            Some(chost) if chost != lhost => binds.push(PortChange {
                container: container.to_string(),
                kind: PortChangeKind::Bind,
                before: Some(Port {
                    host: *chost,
                    container: key.0,
                    proto: key.1.clone(),
                }),
                after: Some(Port {
                    host: *lhost,
                    container: key.0,
                    proto: key.1.clone(),
                }),
            }),
            _ => {}
        }
    }
    for (key, chost) in &cmap {
        if !lmap.contains_key(key) {
            removes.push(PortChange {
                container: container.to_string(),
                kind: PortChangeKind::Removed,
                before: Some(Port {
                    host: *chost,
                    container: key.0,
                    proto: key.1.clone(),
                }),
                after: None,
            });
        }
    }

    // Coalescing pass: a Removed (cport=X, proto=A, host=H) +
    // Added (cport=X, proto=B, host=H) with A != B and same host
    // is one `Proto` change, not two events.
    let mut protos: Vec<PortChange> = Vec::new();
    let mut consumed_remove: Vec<usize> = Vec::new();
    let mut consumed_add: Vec<usize> = Vec::new();
    for (ri, rem) in removes.iter().enumerate() {
        let rport = rem.before.as_ref().unwrap();
        for (ai, add) in adds.iter().enumerate() {
            if consumed_add.contains(&ai) {
                continue;
            }
            let aport = add.after.as_ref().unwrap();
            if rport.container == aport.container
                && rport.host == aport.host
                && rport.proto != aport.proto
            {
                protos.push(PortChange {
                    container: container.to_string(),
                    kind: PortChangeKind::Proto,
                    before: Some(rport.clone()),
                    after: Some(aport.clone()),
                });
                consumed_remove.push(ri);
                consumed_add.push(ai);
                break;
            }
        }
    }
    let removes: Vec<PortChange> = removes
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !consumed_remove.contains(i))
        .map(|(_, v)| v)
        .collect();
    let adds: Vec<PortChange> = adds
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !consumed_add.contains(i))
        .map(|(_, v)| v)
        .collect();

    let mut all: Vec<PortChange> = Vec::new();
    all.extend(adds);
    all.extend(removes);
    all.extend(binds);
    all.extend(protos);
    all
}

/// Stable secondary key for sorting `PortChange` within a container.
/// The diff is rendered in this order so two identical drift
/// snapshots produce byte-identical output.
fn port_change_key(c: &PortChange) -> (u8, u16, String, u16) {
    let kind_rank = match c.kind {
        PortChangeKind::Added => 0,
        PortChangeKind::Removed => 1,
        PortChangeKind::Bind => 2,
        PortChangeKind::Proto => 3,
    };
    let probe = c.after.as_ref().or(c.before.as_ref());
    let cport = probe.map(|p| p.container).unwrap_or(0);
    let proto = probe.map(|p| p.proto.clone()).unwrap_or_default();
    let host = probe.map(|p| p.host).unwrap_or(0);
    (kind_rank, cport, proto, host)
}

fn pick_label(r: &DriftRow) -> String {
    if !r.name.is_empty() {
        r.name.clone()
    } else if !r.id.is_empty() {
        r.id.chars().take(12).collect()
    } else {
        "<unknown>".to_string()
    }
}

/// Render a [`DriftDiff`] as a human-readable summary. Each line is
/// prefixed with two spaces so it slots cleanly under a `DATA:` block.
pub fn format_diff_human(diff: &DriftDiff) -> String {
    if diff.is_empty() {
        return "  (no container-level changes detected; fingerprint diverged for another reason)"
            .to_string();
    }
    let mut lines = Vec::with_capacity(4);
    if !diff.added.is_empty() {
        lines.push(format!(
            "  +{} container{} added: {}",
            diff.added.len(),
            if diff.added.len() == 1 { "" } else { "s" },
            diff.added.join(", ")
        ));
    }
    if !diff.removed.is_empty() {
        lines.push(format!(
            "  -{} container{} removed: {}",
            diff.removed.len(),
            if diff.removed.len() == 1 { "" } else { "s" },
            diff.removed.join(", ")
        ));
    }
    if !diff.changed.is_empty() {
        let mut s = format!(
            "  ~{} container{} changed:",
            diff.changed.len(),
            if diff.changed.len() == 1 { "" } else { "s" }
        );
        for c in &diff.changed {
            s.push_str(&format!(
                "\n    {} ({} → {})",
                c.name, c.from_image, c.to_image
            ));
        }
        lines.push(s);
    }
    // L10 (v0.1.3): port-level changes get their own block so the
    // operator's eye lands on them next to the container-level diff.
    // Each row's payload is shaped like the `inspect ports` output
    // (`<host>:<container>/<proto>`) so a quick `inspect ports
    // <ns>` confirms the new state without a mental translation.
    if !diff.port_changes.is_empty() {
        let mut s = format!(
            "  ⚓{} port-level change{}:",
            diff.port_changes.len(),
            if diff.port_changes.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        for c in &diff.port_changes {
            s.push_str(&format!(
                "\n    {} {} ({})",
                c.container,
                c.kind.as_str(),
                format_port_change_payload(c)
            ));
        }
        lines.push(s);
    }
    lines.join("\n")
}

fn format_port_payload(p: &Port) -> String {
    if p.host == 0 {
        format!("exposed {}/{}", p.container, p.proto)
    } else {
        format!("{}:{}/{}", p.host, p.container, p.proto)
    }
}

fn format_port_change_payload(c: &PortChange) -> String {
    match (c.before.as_ref(), c.after.as_ref()) {
        (None, Some(a)) => format_port_payload(a),
        (Some(b), None) => format_port_payload(b),
        (Some(b), Some(a)) => format!("{} → {}", format_port_payload(b), format_port_payload(a)),
        (None, None) => "<empty>".to_string(),
    }
}

/// Render a [`DriftDiff`] as a JSON object. Fields are stable for v0.1.2;
/// L10 (v0.1.3) adds the `port_changes` array.
pub fn format_diff_json(diff: &DriftDiff) -> String {
    use crate::commands::list::json_string;
    let added: Vec<String> = diff.added.iter().map(|s| json_string(s)).collect();
    let removed: Vec<String> = diff.removed.iter().map(|s| json_string(s)).collect();
    let changed: Vec<String> = diff
        .changed
        .iter()
        .map(|c| {
            format!(
                "{{\"name\":{n},\"from\":{f},\"to\":{t}}}",
                n = json_string(&c.name),
                f = json_string(&c.from_image),
                t = json_string(&c.to_image),
            )
        })
        .collect();
    let port_changes: Vec<String> = diff
        .port_changes
        .iter()
        .map(|c| {
            format!(
                "{{\"container\":{ctr},\"kind\":{k},\"before\":{b},\"after\":{a}}}",
                ctr = json_string(&c.container),
                k = json_string(c.kind.as_str()),
                b = match &c.before {
                    Some(p) => format_port_json(p),
                    None => "null".to_string(),
                },
                a = match &c.after {
                    Some(p) => format_port_json(p),
                    None => "null".to_string(),
                },
            )
        })
        .collect();
    format!(
        "{{\"added\":[{a}],\"removed\":[{r}],\"changed\":[{c}],\"port_changes\":[{pc}]}}",
        a = added.join(","),
        r = removed.join(","),
        c = changed.join(","),
        pc = port_changes.join(","),
    )
}

fn format_port_json(p: &Port) -> String {
    use crate::commands::list::json_string;
    format!(
        "{{\"host\":{h},\"container\":{c},\"proto\":{pr}}}",
        h = p.host,
        c = p.container,
        pr = json_string(&p.proto),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, name: &str, image: &str) -> DriftRow {
        DriftRow {
            id: id.into(),
            name: name.into(),
            image: image.into(),
            ports: Vec::new(),
        }
    }

    fn row_with_ports(id: &str, name: &str, image: &str, ports: Vec<Port>) -> DriftRow {
        DriftRow {
            id: id.into(),
            name: name.into(),
            image: image.into(),
            ports,
        }
    }

    fn p(host: u16, container: u16, proto: &str) -> Port {
        Port {
            host,
            container,
            proto: proto.into(),
        }
    }

    #[test]
    fn empty_inputs_yield_empty_diff() {
        let d = compute_diff(&[], &[]);
        assert!(d.is_empty());
    }

    #[test]
    fn added_container_appears_in_added() {
        let cached = vec![row("a", "alpha", "img:1")];
        let live = vec![row("a", "alpha", "img:1"), row("b", "beta", "img:1")];
        let d = compute_diff(&cached, &live);
        assert_eq!(d.added, vec!["beta"]);
        assert!(d.removed.is_empty());
        assert!(d.changed.is_empty());
    }

    #[test]
    fn removed_container_appears_in_removed() {
        let cached = vec![row("a", "alpha", "img:1"), row("b", "beta", "img:1")];
        let live = vec![row("a", "alpha", "img:1")];
        let d = compute_diff(&cached, &live);
        assert_eq!(d.removed, vec!["beta"]);
    }

    #[test]
    fn image_change_with_same_id_is_changed_not_replaced() {
        let cached = vec![row("a", "alpha", "img:1")];
        let live = vec![row("a", "alpha", "img:2")];
        let d = compute_diff(&cached, &live);
        assert!(d.added.is_empty());
        assert!(d.removed.is_empty());
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].name, "alpha");
        assert_eq!(d.changed[0].from_image, "img:1");
        assert_eq!(d.changed[0].to_image, "img:2");
    }

    #[test]
    fn legacy_cache_without_id_falls_back_to_name() {
        let cached = vec![row("", "alpha", "img:1")];
        let live = vec![row("xyz", "alpha", "img:1")];
        let d = compute_diff(&cached, &live);
        assert!(d.is_empty());
    }

    #[test]
    fn fingerprint_is_stable_across_input_order() {
        let mut a = vec![row("a", "x", "i"), row("b", "y", "j")];
        let mut b = vec![row("b", "y", "j"), row("a", "x", "i")];
        a.sort_by(|x, y| x.id.cmp(&y.id));
        b.sort_by(|x, y| x.id.cmp(&y.id));
        assert_eq!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn parse_docker_ps_handles_blank_lines_and_whitespace() {
        let raw = "abc\talpha\timg:1\n\ndef\tbeta\timg:2\n";
        let rows = parse_docker_ps(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "alpha");
        assert_eq!(rows[1].name, "beta");
    }

    #[test]
    fn human_format_handles_singular_and_plural() {
        let diff = DriftDiff {
            added: vec!["one".into()],
            removed: vec!["x".into(), "y".into()],
            changed: vec![DriftContainerChange {
                name: "z".into(),
                from_image: "a".into(),
                to_image: "b".into(),
            }],
            port_changes: vec![],
        };
        let s = format_diff_human(&diff);
        assert!(s.contains("+1 container added: one"));
        assert!(s.contains("-2 containers removed: x, y"));
        assert!(s.contains("~1 container changed:"));
        assert!(s.contains("z (a → b)"));
    }

    #[test]
    fn json_format_is_well_formed() {
        let diff = DriftDiff {
            added: vec!["alpha".into()],
            removed: vec![],
            changed: vec![DriftContainerChange {
                name: "z".into(),
                from_image: "a".into(),
                to_image: "b".into(),
            }],
            port_changes: vec![],
        };
        let s = format_diff_json(&diff);
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        assert_eq!(v["added"][0], "alpha");
        assert_eq!(v["changed"][0]["name"], "z");
        assert_eq!(v["changed"][0]["from"], "a");
        assert_eq!(v["changed"][0]["to"], "b");
        // L10: port_changes is always present (empty array when no
        // port-level changes), so agent consumers don't need to
        // handle the missing-field case.
        assert!(v["port_changes"].is_array());
        assert_eq!(v["port_changes"].as_array().unwrap().len(), 0);
    }

    // -------------------------------------------------------------------
    // L10 (v0.1.3) — port-level diff tests.
    // -------------------------------------------------------------------

    #[test]
    fn l10_port_added_when_live_has_extra() {
        // Container present in both; live exposes a new port.
        let cached = vec![row_with_ports("a", "api", "img:1", vec![p(80, 80, "tcp")])];
        let live = vec![row_with_ports(
            "a",
            "api",
            "img:1",
            vec![p(80, 80, "tcp"), p(443, 443, "tcp")],
        )];
        let d = compute_diff(&cached, &live);
        assert!(d.added.is_empty(), "container itself unchanged");
        assert_eq!(d.port_changes.len(), 1);
        assert_eq!(d.port_changes[0].container, "api");
        assert_eq!(d.port_changes[0].kind, PortChangeKind::Added);
        assert_eq!(d.port_changes[0].after, Some(p(443, 443, "tcp")));
        assert!(d.port_changes[0].before.is_none());
    }

    #[test]
    fn l10_port_removed_when_cached_has_extra() {
        let cached = vec![row_with_ports(
            "a",
            "api",
            "img:1",
            vec![p(80, 80, "tcp"), p(443, 443, "tcp")],
        )];
        let live = vec![row_with_ports("a", "api", "img:1", vec![p(80, 80, "tcp")])];
        let d = compute_diff(&cached, &live);
        assert_eq!(d.port_changes.len(), 1);
        assert_eq!(d.port_changes[0].kind, PortChangeKind::Removed);
        assert_eq!(d.port_changes[0].before, Some(p(443, 443, "tcp")));
        assert!(d.port_changes[0].after.is_none());
    }

    #[test]
    fn l10_port_bind_change_same_container_port_different_host() {
        // 5432:5432 → 5433:5432 — operator dodging a collision.
        let cached = vec![row_with_ports(
            "a",
            "db",
            "pg:14",
            vec![p(5432, 5432, "tcp")],
        )];
        let live = vec![row_with_ports(
            "a",
            "db",
            "pg:14",
            vec![p(5433, 5432, "tcp")],
        )];
        let d = compute_diff(&cached, &live);
        assert_eq!(d.port_changes.len(), 1);
        assert_eq!(d.port_changes[0].kind, PortChangeKind::Bind);
        assert_eq!(d.port_changes[0].before, Some(p(5432, 5432, "tcp")));
        assert_eq!(d.port_changes[0].after, Some(p(5433, 5432, "tcp")));
    }

    #[test]
    fn l10_port_proto_change_same_host_and_container_port() {
        // 53:53/tcp → 53:53/udp — DNS service flipped transport.
        // Naive diffing would see Removed(tcp) + Added(udp); the
        // coalescing pass folds them into one Proto entry.
        let cached = vec![row_with_ports("a", "dns", "img:1", vec![p(53, 53, "tcp")])];
        let live = vec![row_with_ports("a", "dns", "img:1", vec![p(53, 53, "udp")])];
        let d = compute_diff(&cached, &live);
        assert_eq!(d.port_changes.len(), 1);
        assert_eq!(d.port_changes[0].kind, PortChangeKind::Proto);
        assert_eq!(d.port_changes[0].before, Some(p(53, 53, "tcp")));
        assert_eq!(d.port_changes[0].after, Some(p(53, 53, "udp")));
    }

    #[test]
    fn l10_unchanged_ports_yield_empty_port_changes() {
        let cached = vec![row_with_ports(
            "a",
            "api",
            "img:1",
            vec![p(80, 80, "tcp"), p(443, 443, "tcp")],
        )];
        let live = vec![row_with_ports(
            "a",
            "api",
            "img:1",
            vec![p(443, 443, "tcp"), p(80, 80, "tcp")], // different input order
        )];
        let d = compute_diff(&cached, &live);
        assert!(
            d.port_changes.is_empty(),
            "unchanged port set must produce no entries (got: {:?})",
            d.port_changes
        );
    }

    #[test]
    fn l10_added_container_does_not_double_count_port_changes() {
        // A wholly new container shows up in `added`; we MUST NOT
        // also surface its ports as `port_changes` entries (that
        // would double-count the operator's intent).
        let cached: Vec<DriftRow> = vec![];
        let live = vec![row_with_ports(
            "a",
            "api",
            "img:1",
            vec![p(80, 80, "tcp"), p(443, 443, "tcp")],
        )];
        let d = compute_diff(&cached, &live);
        assert_eq!(d.added, vec!["api"]);
        assert!(
            d.port_changes.is_empty(),
            "ports of a new container belong to the container-level entry"
        );
    }

    #[test]
    fn l10_full_5_container_fixture_per_spec() {
        // Mirrors the L10 spec's acceptance test: A gains a port
        // (Added), B loses one (Removed), C bind moves (Bind), D
        // proto flips (Proto), E unchanged. Exactly 4 entries.
        let cached = vec![
            row_with_ports("a", "alpha", "img:1", vec![p(80, 80, "tcp")]),
            row_with_ports(
                "b",
                "bravo",
                "img:1",
                vec![p(80, 80, "tcp"), p(443, 443, "tcp")],
            ),
            row_with_ports("c", "charlie", "img:1", vec![p(5432, 5432, "tcp")]),
            row_with_ports("d", "delta", "img:1", vec![p(53, 53, "tcp")]),
            row_with_ports("e", "echo", "img:1", vec![p(8080, 8080, "tcp")]),
        ];
        let live = vec![
            row_with_ports(
                "a",
                "alpha",
                "img:1",
                vec![p(80, 80, "tcp"), p(443, 443, "tcp")],
            ),
            row_with_ports("b", "bravo", "img:1", vec![p(80, 80, "tcp")]),
            row_with_ports("c", "charlie", "img:1", vec![p(5433, 5432, "tcp")]),
            row_with_ports("d", "delta", "img:1", vec![p(53, 53, "udp")]),
            row_with_ports("e", "echo", "img:1", vec![p(8080, 8080, "tcp")]),
        ];
        let d = compute_diff(&cached, &live);
        assert!(d.added.is_empty());
        assert!(d.removed.is_empty());
        assert!(d.changed.is_empty());
        assert_eq!(
            d.port_changes.len(),
            4,
            "expected exactly 4 port changes: {:?}",
            d.port_changes
        );
        let by_kind: std::collections::BTreeMap<&str, usize> = d
            .port_changes
            .iter()
            .map(|c| (c.container.as_str(), 1))
            .collect();
        assert!(by_kind.contains_key("alpha"));
        assert!(by_kind.contains_key("bravo"));
        assert!(by_kind.contains_key("charlie"));
        assert!(by_kind.contains_key("delta"));
        assert!(!by_kind.contains_key("echo")); // unchanged
    }

    #[test]
    fn l10_human_format_renders_port_block() {
        let diff = DriftDiff {
            added: vec![],
            removed: vec![],
            changed: vec![],
            port_changes: vec![PortChange {
                container: "db".into(),
                kind: PortChangeKind::Bind,
                before: Some(p(5432, 5432, "tcp")),
                after: Some(p(5433, 5432, "tcp")),
            }],
        };
        let s = format_diff_human(&diff);
        assert!(s.contains("port-level change"), "missing block header: {s}");
        assert!(s.contains("db bind"), "missing per-row label: {s}");
        assert!(
            s.contains("5432:5432/tcp → 5433:5432/tcp"),
            "missing payload: {s}"
        );
    }

    #[test]
    fn l10_json_format_includes_port_changes_array() {
        let diff = DriftDiff {
            added: vec![],
            removed: vec![],
            changed: vec![],
            port_changes: vec![
                PortChange {
                    container: "api".into(),
                    kind: PortChangeKind::Added,
                    before: None,
                    after: Some(p(443, 443, "tcp")),
                },
                PortChange {
                    container: "dns".into(),
                    kind: PortChangeKind::Proto,
                    before: Some(p(53, 53, "tcp")),
                    after: Some(p(53, 53, "udp")),
                },
            ],
        };
        let s = format_diff_json(&diff);
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        let arr = v["port_changes"].as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["container"], "api");
        assert_eq!(arr[0]["kind"], "added");
        assert!(arr[0]["before"].is_null());
        assert_eq!(arr[0]["after"]["host"], 443);
        assert_eq!(arr[0]["after"]["proto"], "tcp");
        assert_eq!(arr[1]["kind"], "proto");
        assert_eq!(arr[1]["before"]["proto"], "tcp");
        assert_eq!(arr[1]["after"]["proto"], "udp");
    }

    #[test]
    fn l10_unbound_port_renders_as_exposed_in_human_form() {
        let diff = DriftDiff {
            added: vec![],
            removed: vec![],
            changed: vec![],
            port_changes: vec![PortChange {
                container: "api".into(),
                kind: PortChangeKind::Added,
                before: None,
                after: Some(p(0, 8080, "tcp")), // host=0 means unbound
            }],
        };
        let s = format_diff_human(&diff);
        assert!(
            s.contains("exposed 8080/tcp"),
            "unbound port must render with `exposed` prefix: {s}"
        );
    }

    #[test]
    fn l10_drift_row_fingerprint_includes_ports() {
        // Two rows that differ only in port set must produce
        // distinct fingerprints — a bind-only change has to flip
        // the cheap fingerprint or the diff layer never runs.
        let a = row_with_ports("x", "api", "img:1", vec![p(5432, 5432, "tcp")]);
        let b = row_with_ports("x", "api", "img:1", vec![p(5433, 5432, "tcp")]);
        assert_ne!(a.fingerprint_line(), b.fingerprint_line());
    }
}
