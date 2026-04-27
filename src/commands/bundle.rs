//! `inspect bundle` command handler (B9, v0.1.2).
//!
//! Thin shell over [`crate::bundle`]: load YAML, dispatch to plan or
//! apply.

use anyhow::{Context, Result};

use crate::bundle::{self, ApplyOpts, Bundle};
use crate::cli::{BundleApplyArgs, BundleArgs, BundleMode, BundlePlanArgs};
use crate::error::ExitKind;

pub fn run(args: BundleArgs) -> Result<ExitKind> {
    match args.mode {
        BundleMode::Plan(a) => plan(a),
        BundleMode::Apply(a) => apply(a),
    }
}

fn plan(args: BundlePlanArgs) -> Result<ExitKind> {
    let bundle = load(&args.file)?;
    bundle::plan(&bundle)
}

fn apply(args: BundleApplyArgs) -> Result<ExitKind> {
    let bundle = load(&args.file)?;
    // Reason is propagated via the bundle's own `reason:` field if the
    // YAML didn't already set one. The CLI flag is the override.
    let mut bundle = bundle;
    if let Some(r) = args.reason {
        let validated = crate::safety::validate_reason(Some(r.as_str()))?;
        bundle.reason = validated;
    }
    bundle::apply(
        &bundle,
        ApplyOpts {
            apply: args.apply,
            no_prompt: args.no_prompt,
        },
    )
}

fn load(path: &std::path::Path) -> Result<Bundle> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading bundle file `{}`", path.display()))?;
    let bundle: Bundle = serde_yaml::from_str(&raw)
        .with_context(|| format!("parsing bundle file `{}`", path.display()))?;
    Ok(bundle)
}
