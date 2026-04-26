//! Phase 10 — correlation rules.
//!
//! A correlation rule observes a command's structured data and produces
//! zero-or-more [`NextStep`] suggestions. Rules are intentionally
//! **cheap** — they look at already-collected data, not new probes.
//! The bible (§11) is explicit: "if it can't be computed cheaply, it's
//! omitted."
//!
//! The registry exposes a small surface so individual commands can
//! pick the rules that apply to their data shape:
//!
//!   * [`status_rules`] — operates on per-service status rows.
//!   * [`why_rules`]    — operates on the dependency walk + root cause.
//!   * [`search_rules`] — operates on the post-pipeline record stream.
//!   * [`drift_rules`]  — appended whenever the meta carries a non-null
//!     `drift_warning` field.
//!
//! Confidence is encoded by ordering: rules append in priority order
//! and callers may cap the final list at 3 (bible §11).

use crate::verbs::output::NextStep;

/// Maximum number of correlation suggestions to surface per command.
/// Bible §11 caps the `NEXT` block at 3 follow-ups.
pub const MAX_NEXT: usize = 3;

/// Lightweight structured view of a per-service status row used by
/// `status_rules` and shared with `why_rules` for cascade detection.
#[derive(Debug, Clone)]
pub struct StatusRow {
    pub server: String,
    pub service: String,
    pub status: String,
}

/// Suggestions for `inspect status` based on observed unhealthy
/// services. Rules: a single down service implies a `why` walk and a
/// dry-run `restart`; multiple down services in one namespace implies
/// a connectivity check first.
pub fn status_rules(rows: &[StatusRow]) -> Vec<NextStep> {
    let mut out = Vec::new();
    let bad: Vec<&StatusRow> = rows
        .iter()
        .filter(|r| matches!(r.status.as_str(), "down" | "unhealthy"))
        .collect();
    if bad.is_empty() {
        return out;
    }

    // Cluster by namespace to detect cascades.
    let mut per_ns: std::collections::BTreeMap<&str, Vec<&StatusRow>> =
        std::collections::BTreeMap::new();
    for r in &bad {
        per_ns.entry(&r.server).or_default().push(r);
    }
    for (ns, group) in &per_ns {
        if group.len() >= 2 {
            out.push(NextStep::new(
                format!("inspect connectivity {ns}"),
                format!("multiple unhealthy services in {ns}; check edges"),
            ));
        } else if let Some(r) = group.first() {
            out.push(NextStep::new(
                format!("inspect why {}/{}", r.server, r.service),
                format!("diagnose the {} service", r.service),
            ));
            out.push(NextStep::new(
                format!("inspect restart {}/{}", r.server, r.service),
                "dry-run; add --apply to execute".to_string(),
            ));
        }
    }
    out.truncate(MAX_NEXT);
    out
}

/// Suggestions for `inspect why`. If a root-cause was identified,
/// recommend logs against it and a dry-run restart. Otherwise hint
/// at a `health` probe.
pub fn why_rules(server: &str, root_cause: Option<&str>) -> Vec<NextStep> {
    let mut out = Vec::new();
    if let Some(rc) = root_cause {
        out.push(NextStep::new(
            format!("inspect logs {server}/{rc} --since 5m"),
            format!("inspect recent activity on root-cause service {rc}"),
        ));
        out.push(NextStep::new(
            format!("inspect restart {server}/{rc}"),
            "dry-run; add --apply if recovery is the goal".to_string(),
        ));
    } else {
        out.push(NextStep::new(
            format!("inspect health {server}"),
            "no failing dependency found; probe live health".to_string(),
        ));
    }
    out.truncate(MAX_NEXT);
    out
}

/// Suggestions for `inspect search` based on the record stream. We use
/// a small subset of cheap signals that fit bible §11's "if it can't
/// be computed cheaply, it's omitted" rule:
///
///   * Many records concentrated on one service → suggest a `logs`
///     command on that service (deeper context).
///   * A high error count → suggest a `why` walk.
///
/// `services` is the list of `service` labels seen on records;
/// duplicates are expected.
pub fn search_rules(server_hint: Option<&str>, services: &[String]) -> Vec<NextStep> {
    let mut out = Vec::new();
    if services.is_empty() {
        return out;
    }
    // Find the dominant service if it accounts for >=60% of records.
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for s in services {
        *counts.entry(s.as_str()).or_insert(0) += 1;
    }
    let total = services.len();
    if let Some((svc, n)) = counts.iter().max_by_key(|(_, n)| **n) {
        if *n * 10 >= total * 6 && !svc.is_empty() {
            // dominant service
            let server = server_hint.unwrap_or("<server>");
            out.push(NextStep::new(
                format!("inspect logs {server}/{svc} --since 15m"),
                format!("most matches concentrated on {svc} ({n}/{total})"),
            ));
            out.push(NextStep::new(
                format!("inspect why {server}/{svc}"),
                "dependency walk for the dominant service".to_string(),
            ));
        }
    }
    out.truncate(MAX_NEXT);
    out
}

/// Append a "profile drift detected" suggestion when the namespace
/// profile is stale. Caller passes the namespace name.
#[cfg(test)]
pub fn drift_rules(namespace: &str) -> Vec<NextStep> {
    vec![NextStep::new(
        format!("inspect setup {namespace} --force"),
        "profile drift detected; re-run discovery".to_string(),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_rules_empty_when_all_healthy() {
        let rows = vec![StatusRow {
            server: "arte".into(),
            service: "pulse".into(),
            status: "ok".into(),
        }];
        assert!(status_rules(&rows).is_empty());
    }

    #[test]
    fn status_rules_single_down_suggests_why_and_restart() {
        let rows = vec![StatusRow {
            server: "arte".into(),
            service: "pulse".into(),
            status: "down".into(),
        }];
        let n = status_rules(&rows);
        assert_eq!(n.len(), 2);
        assert!(n[0].cmd.starts_with("inspect why "));
        assert!(n[1].cmd.starts_with("inspect restart "));
    }

    #[test]
    fn status_rules_multiple_unhealthy_suggests_connectivity() {
        let rows = vec![
            StatusRow {
                server: "arte".into(),
                service: "pulse".into(),
                status: "down".into(),
            },
            StatusRow {
                server: "arte".into(),
                service: "atlas".into(),
                status: "unhealthy".into(),
            },
        ];
        let n = status_rules(&rows);
        assert_eq!(n.len(), 1);
        assert!(n[0].cmd.contains("connectivity"));
    }

    #[test]
    fn why_rules_with_root_cause_suggests_logs_and_restart() {
        let n = why_rules("arte", Some("postgres"));
        assert_eq!(n.len(), 2);
        assert!(n[0].cmd.contains("logs arte/postgres"));
        assert!(n[1].cmd.contains("restart arte/postgres"));
    }

    #[test]
    fn why_rules_without_root_cause_suggests_health() {
        let n = why_rules("arte", None);
        assert_eq!(n.len(), 1);
        assert!(n[0].cmd.contains("health"));
    }

    #[test]
    fn search_rules_dominant_service_promoted() {
        let svcs: Vec<String> = vec!["pulse"; 7]
            .into_iter()
            .chain(vec!["atlas"; 3])
            .map(String::from)
            .collect();
        let n = search_rules(Some("arte"), &svcs);
        assert_eq!(n.len(), 2);
        assert!(n[0].cmd.contains("logs arte/pulse"));
        assert!(n[1].cmd.contains("why arte/pulse"));
    }

    #[test]
    fn search_rules_balanced_no_promotion() {
        let svcs: Vec<String> = vec!["pulse", "atlas", "atlas", "pulse"]
            .into_iter()
            .map(String::from)
            .collect();
        // 50/50: no dominant service.
        assert!(search_rules(Some("arte"), &svcs).is_empty());
    }

    #[test]
    fn drift_rules_emits_setup_force() {
        let n = drift_rules("arte");
        assert_eq!(n.len(), 1);
        assert!(n[0].cmd.contains("inspect setup arte --force"));
    }
}
