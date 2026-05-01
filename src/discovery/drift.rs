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
//! diff is a human-readable explanation of *what* changed. Port-level diffs
//! are intentionally deferred — `docker ps`'s `Ports` column needs careful
//! parsing and the value/risk ratio of a half-correct parser is poor.

use std::time::Duration;

use crate::profile::cache::{clear_drift_marker, load_profile, write_drift_marker};
use crate::profile::schema::Profile;
use crate::ssh::{run_remote, RunOpts, SshTarget};

/// One container row, normalized so cached and live data are
/// apples-to-apples.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DriftRow {
    pub id: String,
    pub name: String,
    pub image: String,
}

impl DriftRow {
    fn fingerprint_line(&self) -> String {
        // Stable, tab-separated. Order: id\tname\timage. Names matter for
        // the human diff but the id is the primary key for sameness — if
        // ids match but image changed, that's a "changed" entry, not
        // add+remove.
        format!("{}\t{}\t{}", self.id, self.name, self.image)
    }
}

/// One container whose image changed in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftContainerChange {
    pub name: String,
    pub from_image: String,
    pub to_image: String,
}

/// Structured human-readable diff between cached and live container sets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriftDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<DriftContainerChange>,
}

impl DriftDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
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

/// Cheap probe of the live host. Container ids + names + images. Sorted by
/// container id for stable hashing.
fn cheap_rows(namespace: &str, target: &SshTarget) -> anyhow::Result<Vec<DriftRow>> {
    // Docker's Go template needs the literal `\t` to make tabs; the shell
    // single-quote here passes the backslash-t through to docker untouched.
    let cmd = "docker ps --format '{{.ID}}\\t{{.Names}}\\t{{.Image}}' 2>/dev/null";
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
            let mut it = l.splitn(3, '\t');
            let id = it.next()?.trim().to_string();
            let name = it.next()?.trim().to_string();
            let image = it.next()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            Some(DriftRow { id, name, image })
        })
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

/// Project the cached profile down to the same shape as [`cheap_rows`].
fn baseline_rows(p: &Profile) -> Vec<DriftRow> {
    let mut rows: Vec<DriftRow> = p
        .services
        .iter()
        .filter(|s| matches!(s.kind, crate::profile::schema::ServiceKind::Container))
        .map(|s| DriftRow {
            id: s.container_id.clone().unwrap_or_default(),
            name: s.name.clone(),
            image: s.image.clone().unwrap_or_default(),
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
    diff
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
    let mut lines = Vec::with_capacity(3);
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
    lines.join("\n")
}

/// Render a [`DriftDiff`] as a JSON object. Fields are stable for v0.1.2.
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
    format!(
        "{{\"added\":[{a}],\"removed\":[{r}],\"changed\":[{c}]}}",
        a = added.join(","),
        r = removed.join(","),
        c = changed.join(","),
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
        };
        let s = format_diff_json(&diff);
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        assert_eq!(v["added"][0], "alpha");
        assert_eq!(v["changed"][0]["name"], "z");
        assert_eq!(v["changed"][0]["from"], "a");
        assert_eq!(v["changed"][0]["to"], "b");
    }
}
