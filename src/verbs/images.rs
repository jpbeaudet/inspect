//! `inspect images <sel>` — list docker images.

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::plan;
use crate::verbs::output::{Envelope, Renderer};

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
    // k8s images come from the discovered profile's pod specs
    // (no docker daemon). Branch early.
    if let Some(ns_name) = args.selector.split('/').next() {
        if let Ok(resolved) = crate::config::resolver::resolve(ns_name) {
            if resolved.config.runtime_kind() == crate::exec::runtime::RuntimeKind::K8s {
                return images_k8s(&args, ns_name);
            }
        }
    }

    let (runner, nses, _) = plan(&args.selector)?;
    let mut renderer = Renderer::new();
    let mut count = 0usize;
    for ns in &nses {
        let out = runner.run(
            &ns.namespace,
            &ns.target,
            "docker images --format '{{json .}}'",
            RunOpts::with_timeout(20),
        )?;
        if !out.ok() {
            if !args.format.is_json() {
                crate::tee_eprintln!(
                    "{}: docker images failed (exit {}): {}",
                    ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        for line in out.stdout.lines() {
            count += 1;
            let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
            let repo = v.get("Repository").and_then(|x| x.as_str()).unwrap_or("");
            let tag = v.get("Tag").and_then(|x| x.as_str()).unwrap_or("");
            let size = v.get("Size").and_then(|x| x.as_str()).unwrap_or("");
            let repo_tag = format!("{repo}:{tag}");
            renderer.data_line(format!("{ns} | {repo_tag:<48} {size}", ns = ns.namespace));
            renderer.push_row(
                &Envelope::new(&ns.namespace, "image", "image")
                    .put("repo_tag", repo_tag.clone())
                    .put("size", size.to_string())
                    .put("raw", v),
            );
        }
    }
    renderer.summary(format!("{count} image(s)"));
    let fmt = args.format.resolve()?;
    let select = args.format.select_filter()?;
    renderer.dispatch(&fmt, select)
}

/// `inspect images <k8s-ns>` — unique container images across
/// the namespace's pods, read from the discovered profile (no daemon).
fn images_k8s(args: &SimpleSelectorArgs, ns: &str) -> Result<ExitKind> {
    let mut renderer = Renderer::new();
    let profile = match crate::profile::cache::load_profile(ns)? {
        Some(p) => p,
        None => {
            renderer
                .summary(format!(
                    "no cached profile for '{ns}' — run `inspect setup {ns}`"
                ))
                .next(format!("inspect setup {ns}"));
            let fmt = args.format.resolve()?;
            return renderer.dispatch(&fmt, args.format.select_filter()?);
        }
    };
    let mut seen = std::collections::BTreeSet::new();
    for s in &profile.services {
        if let Some(img) = s.image.as_deref() {
            if seen.insert(img.to_string()) {
                renderer.data_line(format!("{ns} | {img}"));
                renderer
                    .push_row(&Envelope::new(ns, "image", "image").put("image", img.to_string()));
            }
        }
    }
    renderer.summary(format!("{} unique image(s)", seen.len()));
    let fmt = args.format.resolve()?;
    renderer.dispatch(&fmt, args.format.select_filter()?)
}
