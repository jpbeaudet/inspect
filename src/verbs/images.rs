//! `inspect images <sel>` — list docker images.

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::plan;
use crate::verbs::output::{Envelope, Renderer};

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
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
