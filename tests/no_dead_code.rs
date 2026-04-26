//! Contract test: lock down the dead-code cleanup
//! (archives/DEADCODE_CLEANUP_PLAN.md §8).
//!
//! Hard gates enforced here:
//!
//! * **H3** — every `#[allow(dead_code)]` carries an inline `// v2: <tag>`
//!   sentinel naming the catalog item that justifies it (bible §1, §27).
//!   Prevents the suppression from quietly drifting back to "I'll get to it".
//! * **H4** — zero `#![allow(dead_code)]` blanket attributes anywhere under
//!   `src/`. Module-wide suppressions hide cascade rot.
//! * **H5** — the total count of `#[allow(dead_code)]` sites is bounded by
//!   the v2 allow-list (today: exactly **one** — `AliasError::AliasChain`
//!   for V7 parameterized-aliases). Bumping this number requires updating
//!   the plan AND the bible v2 catalog.
//!
//! Companion guard: `Cargo.toml` carries `[lints.rust] dead_code = "deny"`,
//! so any new dead-code warning fails `cargo build`. This test catches the
//! `allow` escape hatch.

use std::fs;
use std::path::{Path, PathBuf};

/// Maximum allowed `#[allow(dead_code)]` sites under `src/`. See the v2
/// allow-list in `archives/DEADCODE_CLEANUP_PLAN.md` §3.
const MAX_DEAD_CODE_ALLOWS: usize = 1;

#[test]
fn h4_no_module_wide_dead_code_suppressions() {
    let mut offenders = Vec::new();
    for (path, body) in walk_src() {
        for (lineno, line) in body.lines().enumerate() {
            if line.contains("#![allow(dead_code)]") {
                offenders.push(format!("{}:{}", path.display(), lineno + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "module-wide `#![allow(dead_code)]` is forbidden (H4). Offenders:\n  {}",
        offenders.join("\n  "),
    );
}

#[test]
fn h3_every_dead_code_allow_carries_v2_sentinel() {
    let mut bare = Vec::new();
    for (path, body) in walk_src() {
        let lines: Vec<&str> = body.lines().collect();
        for (lineno, line) in lines.iter().enumerate() {
            if !line.contains("#[allow(dead_code)]") {
                continue;
            }
            // The `// v2: <tag>` sentinel may sit on the same line as the
            // attribute or on the next non-blank line — rustfmt is free to
            // split a trailing comment off. Either is fine; absence is not.
            let same = line.contains("// v2:");
            let next = lines
                .get(lineno + 1)
                .map(|l| l.trim_start().starts_with("// v2:"))
                .unwrap_or(false);
            if !(same || next) {
                bare.push(format!(
                    "{}:{}  {}",
                    path.display(),
                    lineno + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        bare.is_empty(),
        "every `#[allow(dead_code)]` must carry `// v2: <tag>` (H3). \
         Offenders:\n  {}",
        bare.join("\n  "),
    );
}

#[test]
fn h5_dead_code_allow_count_bounded() {
    let mut count = 0usize;
    let mut sites = Vec::new();
    for (path, body) in walk_src() {
        for (lineno, line) in body.lines().enumerate() {
            if line.contains("#[allow(dead_code)]") {
                count += 1;
                sites.push(format!(
                    "{}:{}  {}",
                    path.display(),
                    lineno + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        count <= MAX_DEAD_CODE_ALLOWS,
        "found {count} `#[allow(dead_code)]` sites (max {MAX_DEAD_CODE_ALLOWS}). \
         Bumping this requires updating archives/DEADCODE_CLEANUP_PLAN.md and the bible v2 \
         catalog. Current sites:\n  {}",
        sites.join("\n  "),
    );
}

// ---------------------------------------------------------------- helpers

fn src_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn walk_src() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    walk(&src_root(), &mut out);
    out
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().map(|e| e == "rs").unwrap_or(false) {
            if let Ok(body) = fs::read_to_string(&p) {
                out.push((p, body));
            }
        }
    }
}
