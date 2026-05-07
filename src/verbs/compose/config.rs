//! F6 (v0.1.3): `inspect compose config <ns>/<project>` — effective
//! merged compose config (redacted).
//!
//! Wraps `cd <wd> && docker compose -p <p> config` over the
//! persistent ssh socket. Output streams through the L7 redaction
//! pipeline (PEM / header / URL / env maskers) so secret-shaped
//! values in `environment:` blocks and URL auth portions are
//! masked unless `--show-secrets` is passed.

use anyhow::Result;
use serde_json::json;

use crate::cli::ComposeConfigArgs;
use crate::error::ExitKind;
use crate::redact::OutputRedactor;
use crate::ssh::exec::RunOpts;
use crate::verbs::output::OutputDoc;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::{current_runner, resolve_target};

use super::resolve::{project_in_profile, Parsed};

pub fn run(args: ComposeConfigArgs) -> Result<ExitKind> {
    let fmt = args.format.resolve()?;
    let parsed = match Parsed::parse(&args.selector) {
        Ok(p) => p,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::Error);
        }
    };
    let project_name = match parsed.project.as_deref() {
        Some(p) => p,
        None => {
            crate::error::emit(format!(
                "selector '{}' is missing the project portion — \
                 expected '<ns>/<project>'",
                args.selector
            ));
            return Ok(ExitKind::Error);
        }
    };
    let (_profile, project) = match project_in_profile(&parsed.namespace, project_name) {
        Ok(t) => t,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::NoMatches);
        }
    };

    let runner = current_runner();
    let (_resolved, target) = resolve_target(&parsed.namespace)?;
    let cmd = format!(
        "cd {wd} && docker compose -p {p} config 2>&1",
        wd = shquote(&project.working_dir),
        p = shquote(&project.name),
    );
    let out = runner.run(&parsed.namespace, &target, &cmd, RunOpts::with_timeout(30))?;
    if !out.ok() {
        crate::error::emit(format!(
            "docker compose config exited {} on {}/{}: {}",
            out.exit_code,
            parsed.namespace,
            project.name,
            out.stdout
                .trim()
                .lines()
                .take(2)
                .collect::<Vec<_>>()
                .join(" / ")
        ));
        return Ok(ExitKind::Error);
    }

    // Redact line-by-line. The output is YAML, which is line-
    // oriented enough that the existing maskers (which are
    // line-scoped) work without a parser. PEM blocks (rare in
    // compose config but possible in a literal `environment:`
    // value) collapse to a single marker.
    let redactor = OutputRedactor::new(args.show_secrets, false);
    let mut data_lines: Vec<String> = Vec::new();
    for line in out.stdout.lines() {
        match redactor.mask_line(line) {
            Some(masked) => data_lines.push(masked.into_owned()),
            None => continue, // suppressed (inside PEM block)
        }
    }

    let summary = format!(
        "compose config for {ns}/{p} ({n} line(s)){redacted}",
        ns = parsed.namespace,
        p = project.name,
        n = data_lines.len(),
        redacted = if redactor.was_active() && !args.show_secrets {
            " — secrets masked"
        } else {
            ""
        },
    );

    let doc = OutputDoc::new(
        summary,
        json!({
            "namespace": parsed.namespace,
            "project": project.name,
            "working_dir": project.working_dir,
            "compose_file": project.compose_file,
            // The full body is preserved in the JSON envelope so
            // agent consumers don't have to re-parse the human-mode
            // `DATA:` block to get the YAML back.
            "config": data_lines.join("\n"),
            "secrets_masked": redactor.was_active() && !args.show_secrets,
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_quiet(args.format.quiet);

    crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())
}
