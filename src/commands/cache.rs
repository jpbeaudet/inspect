//! `inspect cache` — F8 (v0.1.3) runtime cache management.
//!
//! Provides two subcommands:
//!
//! - `inspect cache show` — list every cached namespace with the
//!   age of its runtime snapshot, the age of its inventory profile,
//!   the staleness verdict (vs `INSPECT_RUNTIME_TTL_SECS`), and the
//!   on-disk size of each tier.
//! - `inspect cache clear [<namespace> | --all]` — delete cached
//!   runtime snapshots. Inventory profiles are **never** touched
//!   here (use `inspect setup` to refresh those).
//!
//! Both subcommands respect `--json`. Output goes through
//! [`OutputDoc`] so the schema is the same as every other read verb.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::cli::{CacheArgs, CacheClearArgs, CacheCommand, CacheShowArgs};
use crate::error::ExitKind;
use crate::profile::runtime::{
    self, inventory_age, list_cached_namespaces, runtime_path, runtime_ttl,
};
use crate::verbs::output::OutputDoc;

pub fn run(args: CacheArgs) -> Result<ExitKind> {
    match args.command {
        CacheCommand::Show(a) => show(a),
        CacheCommand::Clear(a) => clear(a),
    }
}

fn show(args: CacheShowArgs) -> Result<ExitKind> {
    let nses = list_cached_namespaces();
    let ttl = runtime_ttl();

    let mut data_lines: Vec<String> = Vec::new();
    data_lines.push(format!(
        "{:<24} {:>12} {:>14} {:<8} {:>8} {:>10}",
        "NAMESPACE", "RUNTIME_AGE", "INVENTORY_AGE", "STALE?", "REFRESH", "SIZE"
    ));

    let mut rows: Vec<Value> = Vec::new();
    for ns in &nses {
        let snap = runtime::load_runtime(ns);
        let r_age = snap.as_ref().and_then(|s| s.age()).map(|d| d.as_secs());
        let i_age = inventory_age(ns).map(|d| d.as_secs());
        let stale = match snap.as_ref() {
            Some(s) => runtime::is_runtime_stale(s),
            None => true,
        };
        let refresh_count = snap.as_ref().map(|s| s.refresh_count).unwrap_or(0);
        let size = std::fs::metadata(runtime_path(ns)).map(|m| m.len()).ok();
        data_lines.push(format!(
            "{ns:<24} {ra:>12} {ia:>14} {st:<8} {rc:>8} {sz:>10}",
            ra = fmt_age_opt(r_age),
            ia = fmt_age_opt(i_age),
            st = if stale { "yes" } else { "no" },
            rc = refresh_count,
            sz = fmt_size(size),
        ));
        rows.push(json!({
            "namespace": ns,
            "runtime_age_s": r_age,
            "inventory_age_s": i_age,
            "stale": stale,
            "refresh_count": refresh_count,
            "runtime_bytes": size,
        }));
    }

    let ttl_label = match ttl {
        None => "disabled".to_string(),
        Some(d) if d == std::time::Duration::MAX => "infinite".to_string(),
        Some(d) => format!("{}s", d.as_secs()),
    };
    let summary = format!("{} cached namespace(s); ttl = {ttl_label}", nses.len());
    let doc = OutputDoc::new(summary, json!({ "namespaces": rows })).with_meta(
        "ttl_secs",
        match ttl {
            None => Value::Null,
            Some(d) if d == std::time::Duration::MAX => Value::String("infinite".into()),
            Some(d) => Value::from(d.as_secs()),
        },
    );
    let fmt = args.format.resolve()?;
    crate::format::render::render_doc(&doc, &fmt, &data_lines)?;
    Ok(ExitKind::Success)
}

fn clear(args: CacheClearArgs) -> Result<ExitKind> {
    if args.all && args.namespace.is_some() {
        return Err(anyhow!("--all and <namespace> are mutually exclusive"));
    }
    let cleared: Vec<String> = if args.all {
        let nses = list_cached_namespaces();
        runtime::clear_all()?;
        nses
    } else {
        let ns = args
            .namespace
            .ok_or_else(|| anyhow!("specify a namespace, or use --all"))?;
        // F18 (v0.1.3): cache clear is a namespace-scoped verb that
        // does not go through `resolve_target`. Stamp the transcript
        // context here so this invocation's output lands in the
        // right per-ns transcript file.
        crate::transcript::set_namespace(&ns);
        runtime::clear_runtime(&ns);
        vec![ns]
    };

    // F8: record an audit entry per cleared namespace. `cache clear`
    // is a deliberate operator action (unlike automatic invalidation
    // by lifecycle verbs, which is high-frequency and not auditable
    // by design), so it belongs in the same audit log as restarts.
    // Best-effort: a failure to open the audit store does not fail
    // the verb — the cache files are already gone.
    if let Ok(store) = crate::safety::AuditStore::open() {
        for ns in &cleared {
            let mut entry = crate::safety::AuditEntry::new("cache-clear", ns);
            entry.exit = 0;
            entry.args = if args.all {
                "--all".to_string()
            } else {
                String::new()
            };
            let _ = store.append(&entry);
        }
    }

    let mut data_lines: Vec<String> = Vec::new();
    for ns in &cleared {
        data_lines.push(format!("cleared {ns}"));
    }
    let summary = format!("cleared {} runtime cache entrie(s)", cleared.len());
    let doc = OutputDoc::new(summary, json!({ "cleared": cleared }));
    let fmt = args.format.resolve()?;
    crate::format::render::render_doc(&doc, &fmt, &data_lines)?;
    Ok(ExitKind::Success)
}

fn fmt_age_opt(age: Option<u64>) -> String {
    match age {
        Some(s) if s < 60 => format!("{s}s"),
        Some(s) if s < 3600 => format!("{}m", s / 60),
        Some(s) => format!("{}h", s / 3600),
        None => "-".to_string(),
    }
}

fn fmt_size(b: Option<u64>) -> String {
    match b {
        Some(n) if n < 1024 => format!("{n}B"),
        Some(n) if n < 1024 * 1024 => format!("{}K", n / 1024),
        Some(n) => format!("{}M", n / (1024 * 1024)),
        None => "-".to_string(),
    }
}
