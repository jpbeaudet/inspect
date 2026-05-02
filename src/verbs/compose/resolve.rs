//! F6 (v0.1.3): selector + project resolution shared by every compose
//! sub-verb.
//!
//! The selector grammar for `inspect compose` is intentionally
//! narrower than the generic `<ns>/<service>` form used by `inspect
//! logs` / `inspect restart`:
//!
//! - `<ns>` — for `compose ls` (lists every project on the namespace).
//! - `<ns>/<project>` — for `compose ps`, `compose config`,
//!   aggregated `compose logs`, and `compose restart --all`.
//! - `<ns>/<project>/<service>` — for narrowed `compose logs` and
//!   for `compose restart` (the safe default).
//!
//! We don't route through `selector::resolve` because that resolver
//! treats `<ns>/<x>` as `<ns>/<service>`, which loses the project
//! context. Instead we parse here, then look up the project in the
//! namespace's cached profile to recover its `working_dir` (every
//! per-project verb `cd`s there before invoking `docker compose`).

use anyhow::{anyhow, bail, Result};

use crate::profile::cache::load_profile;
use crate::profile::schema::{ComposeProject, Profile};

/// A parsed compose selector. `service` is `None` for project-only
/// selectors; the verb decides whether that's an error
/// (`compose restart` without `--all`) or fine (`compose logs`,
/// `compose ps`).
#[derive(Debug, Clone)]
pub struct Parsed {
    pub namespace: String,
    pub project: Option<String>,
    pub service: Option<String>,
}

impl Parsed {
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            bail!("empty selector — pass `<ns>` (compose ls) or `<ns>/<project>[/<service>]`");
        }
        // Disallow `:` so a stray `arte:/path` (the F7-papercut #2
        // host-path shape) lands here with a clear message instead of
        // splitting weirdly.
        if raw.contains(':') {
            bail!(
                "selector '{raw}' contains ':' — compose verbs do not accept file paths; \
                 expected '<ns>[/<project>[/<service>]]'"
            );
        }
        let mut parts = raw.split('/').peekable();
        let namespace = parts.next().unwrap_or("").to_string();
        if namespace.is_empty() {
            bail!("selector '{raw}' has empty namespace portion");
        }
        let project = parts
            .next()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        let service = parts
            .next()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        if parts.next().is_some() {
            bail!(
                "selector '{raw}' has too many '/' segments — \
                 expected at most '<ns>/<project>/<service>'"
            );
        }
        if service.is_some() && project.is_none() {
            bail!("selector '{raw}' is missing the project portion");
        }
        Ok(Self {
            namespace,
            project,
            service,
        })
    }
}

/// Look up a single compose project on `namespace`'s cached profile.
/// Returns a chained-hint error when the namespace has no cached
/// profile or no project of that name.
pub fn project_in_profile(namespace: &str, project: &str) -> Result<(Profile, ComposeProject)> {
    let profile = load_profile(namespace)?.ok_or_else(|| {
        anyhow!(
            "no cached profile for namespace '{namespace}' \
                 (run `inspect setup {namespace}` first)"
        )
    })?;
    let cp = profile
        .compose_projects
        .iter()
        .find(|p| p.name == project)
        .cloned()
        .ok_or_else(|| {
            // F2 / F7-style chained hint: tell the operator both
            // *why* and *what to do next* without forcing them to
            // re-read the spec.
            let known: Vec<&str> = profile
                .compose_projects
                .iter()
                .map(|p| p.name.as_str())
                .collect();
            if known.is_empty() {
                anyhow!(
                    "namespace '{namespace}' has no compose projects in its cached profile. \
                     hint: run `inspect compose ls {namespace} --refresh` if you just deployed \
                     one, or `inspect setup {namespace}` to re-discover."
                )
            } else {
                anyhow!(
                    "compose project '{project}' not found on namespace '{namespace}' \
                     (known projects: {}). \
                     hint: `inspect compose ls {namespace}` lists every project.",
                    known.join(", ")
                )
            }
        })?;
    Ok((profile, cp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_namespace_only() {
        let p = Parsed::parse("arte").unwrap();
        assert_eq!(p.namespace, "arte");
        assert!(p.project.is_none());
        assert!(p.service.is_none());
    }

    #[test]
    fn parse_namespace_and_project() {
        let p = Parsed::parse("arte/luminary-onyx").unwrap();
        assert_eq!(p.namespace, "arte");
        assert_eq!(p.project.as_deref(), Some("luminary-onyx"));
        assert!(p.service.is_none());
    }

    #[test]
    fn parse_namespace_project_service() {
        let p = Parsed::parse("arte/luminary-onyx/onyx-vault").unwrap();
        assert_eq!(p.namespace, "arte");
        assert_eq!(p.project.as_deref(), Some("luminary-onyx"));
        assert_eq!(p.service.as_deref(), Some("onyx-vault"));
    }

    #[test]
    fn parse_rejects_too_many_segments() {
        let err = Parsed::parse("arte/p/s/extra").unwrap_err().to_string();
        assert!(err.contains("too many '/'"), "got: {err}");
    }

    #[test]
    fn parse_rejects_colon_in_selector() {
        let err = Parsed::parse("arte:/etc").unwrap_err().to_string();
        assert!(
            err.contains("compose verbs do not accept file paths"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_rejects_service_without_project() {
        // `arte//svc` parses ns="arte", project=None (empty
        // segment), service="svc" — and we error.
        let err = Parsed::parse("arte//svc").unwrap_err().to_string();
        assert!(err.contains("missing the project portion"), "got: {err}");
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(Parsed::parse("").is_err());
        assert!(Parsed::parse("   ").is_err());
    }
}
