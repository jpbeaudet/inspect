//! `inspect profile <ns>` — render the cached profile.

use crate::cli::ProfileArgs;
use crate::commands::list::json_string;
use crate::config::namespace::validate_namespace_name;
use crate::config::resolver;
use crate::error::ExitKind;
use crate::profile::cache::{load_profile, profile_path, read_drift_marker};

pub fn run(args: ProfileArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;
    // Resolve to fail fast on unknown namespaces with a friendly error.
    let _ = resolver::resolve(&args.namespace)?;

    let profile = match load_profile(&args.namespace)? {
        Some(p) => p,
        None => {
            if args.format.is_json() {
                println!(
                    "{{\"schema_version\":1,\"namespace\":{ns},\"status\":\"missing\"}}",
                    ns = json_string(&args.namespace),
                );
            } else {
                println!("SUMMARY: no cached profile for '{}'", args.namespace);
                println!("NEXT:    inspect setup {}", args.namespace);
            }
            return Ok(ExitKind::Error);
        }
    };

    let drift = read_drift_marker(&args.namespace);
    if args.format.is_json() {
        // Stream the full YAML re-encoded as JSON for parity with other
        // `--json` outputs.
        let value: serde_json::Value = serde_json::to_value(&profile)?;
        let envelope = serde_json::json!({
            "schema_version": 1,
            "namespace": profile.namespace,
            "status": "ok",
            "drift": drift.is_some(),
            "profile": value,
        });
        println!("{envelope}");
    } else {
        let yaml = serde_yaml::to_string(&profile)?;
        println!(
            "SUMMARY: profile for '{}' ({} services, {} volumes, {} networks, {} images)",
            profile.namespace,
            profile.services.len(),
            profile.volumes.len(),
            profile.networks.len(),
            profile.images.len()
        );
        println!("DATA:    {}", profile_path(&args.namespace).display());
        if let Some(body) = drift {
            println!("DRIFT:");
            for line in body.lines() {
                println!("  {line}");
            }
        }
        println!("---");
        print!("{yaml}");
        if !yaml.ends_with('\n') {
            println!();
        }
        println!(
            "NEXT:    inspect setup {} --check-drift    inspect setup {} --force",
            profile.namespace, profile.namespace
        );
    }
    Ok(ExitKind::Success)
}
