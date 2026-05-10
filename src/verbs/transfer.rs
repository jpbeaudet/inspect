//! — `inspect put` / `inspect get` / `inspect cp` file
//! transfer over the persistent ControlPath master.
//!
//! Replaces the v0.1.2 base64-in-argv `cp` implementation. The 4 MiB
//! cap is gone: uploads stream the local file body via the runner's
//! stdin channel into a remote `cat > /tmp && mv /tmp /path`
//! atomic-write helper; downloads pull via `base64 <path>` for
//! binary safety. Both directions ride the same multiplexed SSH
//! master used by every other namespace verb, so they inherit
//! revert capture, env overlay, stale-session auto-reauth,
//! and the standard audit trail.
//!
//! Container vs host filesystem is decided by selector form: a
//! selector that names a service (`<ns>/<svc>:/path`) dispatches the
//! atomic-write helper inside `docker exec -i <ctr> sh -c ...`; the
//! host form (`<ns>/_:/path` or the shorthand `<ns>:/path`)
//! runs the helper directly.
//!
//! `inspect cp` is now a thin bidirectional dispatcher: it inspects
//! arg shape and routes to [`run_put`] (push) or [`run_get`] (pull)
//! depending on which side carries the selector. The verb is
//! preserved as a convenience for operators with `scp`/`rsync`
//! muscle memory; the canonical names are `put` and `get`.

use std::io::Write;
use std::time::Instant;

use anyhow::Result;
use base64::Engine as _;

use crate::cli::{CpArgs, GetArgs, PutArgs};
use crate::error::ExitKind;
use crate::safety::diff::{diff_summary, unified_diff};
use crate::safety::gate::ConfirmResult;
use crate::safety::snapshot::sha256_hex;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate, SnapshotStore};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan, Step};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

/// Field pitfall §1.4: a single bulk transfer through the multiplexed
/// SSH master temporarily monopolises the channel. Above this size we
/// emit a one-line stderr warning so operators know to expect brief
/// starvation of concurrent verbs against the same host. The cap is
/// advisory only (no hard refusal); operator can silence with
/// `INSPECT_CP_WARN_BYTES=0`.
const DEFAULT_LARGE_FILE_WARN_BYTES: usize = 1024 * 1024;

fn large_file_warn_threshold() -> usize {
    if let Ok(s) = std::env::var("INSPECT_CP_WARN_BYTES") {
        if let Ok(n) = s.parse::<usize>() {
            return n;
        }
    }
    DEFAULT_LARGE_FILE_WARN_BYTES
}

// ---------------------------------------------------------------------------
// Public verb entrypoints
// ---------------------------------------------------------------------------

/// `inspect put <local> <remote>` — upload a local file to a remote
/// path via the persistent SSH master.
pub fn run_put(args: PutArgs) -> Result<ExitKind> {
    push(PushArgs {
        local: args.local,
        remote: args.remote,
        apply: args.apply,
        diff: args.diff,
        yes: args.yes,
        yes_all: args.yes_all,
        reason: args.reason,
        revert_preview: args.revert_preview,
        mode: args.mode,
        owner: args.owner,
        mkdir_p: args.mkdir_p,
    })
}

/// `inspect get <remote> <local>` — download a remote file to a local
/// path via the persistent SSH master.
pub fn run_get(args: GetArgs) -> Result<ExitKind> {
    pull(args.remote, args.local)
}

/// `inspect cp <source> <dest>` — bidirectional convenience that
/// dispatches to [`run_put`] (push) or [`run_get`] (pull) based on
/// which arg carries the selector. The selector-bearing arg is the
/// remote endpoint.
pub fn run_cp(args: CpArgs) -> Result<ExitKind> {
    let src_remote = looks_remote(&args.source);
    let dst_remote = looks_remote(&args.dest);
    match (src_remote, dst_remote) {
        (false, true) => push(PushArgs {
            local: args.source,
            remote: args.dest,
            apply: args.apply,
            diff: args.diff,
            yes: args.yes,
            yes_all: args.yes_all,
            reason: args.reason,
            revert_preview: args.revert_preview,
            mode: args.mode,
            owner: args.owner,
            mkdir_p: args.mkdir_p,
        }),
        (true, false) => pull(args.source, args.dest),
        (false, false) => {
            crate::error::emit("cp needs at least one remote endpoint (selector with `:path`)");
            Ok(ExitKind::Error)
        }
        (true, true) => {
            crate::error::emit("cp does not support remote→remote transfers");
            Ok(ExitKind::Error)
        }
    }
}

/// Return `true` if `s` looks like a `<selector>:<path>` form (a colon
/// followed by an absolute path). Used by [`run_cp`] to disambiguate
/// push vs pull at argv level.
fn looks_remote(s: &str) -> bool {
    if let Some((before, after)) = s.split_once(':') {
        !before.is_empty() && after.starts_with('/')
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Push (upload) flow
// ---------------------------------------------------------------------------

struct PushArgs {
    local: String,
    remote: String,
    apply: bool,
    diff: bool,
    yes: bool,
    yes_all: bool,
    reason: Option<String>,
    revert_preview: bool,
    mode: Option<String>,
    owner: Option<String>,
    mkdir_p: bool,
}

fn push(args: PushArgs) -> Result<ExitKind> {
    // 1. Read the local file. There is no fixed size cap (the v0.1.2
    // 4 MiB limit was an artifact of base64-in-argv); we stream
    // through stdin so the only practical limits are the operator's
    // patience and the remote disk.
    let local_path = args.local.clone();
    let body = std::fs::read(&local_path)
        .map_err(|e| anyhow::anyhow!("reading local source '{local_path}': {e}"))?;

    let warn_bytes = large_file_warn_threshold();
    if warn_bytes > 0 && body.len() >= warn_bytes {
        crate::tee_eprintln!(
            "inspect: warning: pushing {} bytes through the multiplexed SSH channel \
             will briefly starve concurrent verbs against the same host. \
             (silence with INSPECT_CP_WARN_BYTES=0)",
            body.len()
        );
    }

    // 2. Resolve the selector + path.
    let (runner, nses, targets) = plan(&args.remote)?;
    let mut planned = Vec::new();
    for s in iter_steps(&nses, &targets) {
        let Some(p) = s.path.clone() else {
            crate::error::emit("put requires a `:path` on the remote selector");
            return Ok(ExitKind::Error);
        };
        planned.push((s, p));
    }
    if planned.is_empty() {
        crate::error::emit(format!("'{}' matched no targets", args.remote));
        return Ok(ExitKind::Error);
    }

    // 3. Dry-run path: pull existing remote content (best-effort) and
    // render a per-target diff against the local body if the operator
    // asked for one or just print the SUMMARY otherwise.
    let new_text = String::from_utf8_lossy(&body).to_string();

    if !args.apply {
        let mut r = Renderer::new();
        let mut diffs: Vec<(String, String, String)> = Vec::new();
        for (s, path) in &planned {
            let label = label_for(s, path);
            let old = read_remote(&*runner, s, path).unwrap_or_default();
            diffs.push((label, old, new_text.clone()));
        }
        let summary_diffs: Vec<(String, String)> = diffs
            .iter()
            .map(|(_, o, n)| (o.clone(), n.clone()))
            .collect();
        r.summary(format!(
            "DRY RUN. Would push {} → {} target(s) [{}]",
            local_path,
            planned.len(),
            diff_summary(&summary_diffs),
        ));
        if args.diff {
            for (lbl, old, new) in &diffs {
                let block = unified_diff(old, new, lbl, &format!("{lbl} (proposed)"));
                if !block.is_empty() {
                    r.data_line(block);
                }
            }
        } else {
            for (s, p) in &planned {
                r.data_line(label_for(s, p));
            }
        }
        if args.revert_preview {
            // Show the captured inverse before applying.
            for (s, path) in &planned {
                let label = label_for(s, path);
                let exists = read_remote(&*runner, s, path).is_some();
                let line = if exists {
                    format!("REVERT: state_snapshot of {label}")
                } else {
                    format!("REVERT: rm {label} (created by put)")
                };
                r.data_line(line);
            }
        }
        r.next("Re-run with --apply to execute");
        r.next("Use --diff for a per-target preview");
        r.print();
        return Ok(ExitKind::Success);
    }

    // 4. Apply path: confirmation gate, then per-target streaming
    // upload + audit + revert capture.
    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if let ConfirmResult::Aborted(why) =
        gate.confirm(Confirm::LargeFanout, planned.len(), "Continue?")
    {
        crate::tee_eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }

    let snaps = SnapshotStore::open()?;
    let store = AuditStore::open()?;
    let new_hash = sha256_hex(&body);
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();

    for (s, path) in &planned {
        let label = label_for(s, path);

        // 4a. Snapshot prior remote content (if any) so revert can
        // restore byte-for-byte. A failed read is treated as "file
        // does not exist" — revert becomes a delete.
        let prev_text = read_remote(&*runner, s, path);
        let prev_hash = match &prev_text {
            Some(t) if !t.is_empty() => Some(snaps.put(t.as_bytes())?),
            _ => None,
        };

        // 4b. Build the atomic-write shell pipeline. Reads stdin into
        // a `.tmp` sibling, mirrors mode/ownership of the prior file
        // (best-effort), applies any --mode / --owner overrides, then
        // mv's into place.
        let tmp = format!("{path}.inspect.{}.tmp", &new_hash[..8]);
        let inner = build_stream_atomic_script(
            &tmp,
            path,
            args.mkdir_p,
            args.mode.as_deref(),
            args.owner.as_deref(),
        );
        let cmd = match s.container() {
            Some(container) => format!(
                "docker exec -i {} sh -c {}",
                shquote(container),
                shquote(&inner)
            ),
            None => format!("sh -c {}", shquote(&inner)),
        };

        // 4c. Dispatch with stdin = local body. The runner's stdin
        // path streams bytes off-thread so the local file body never
        // hits the command argv.
        let started = Instant::now();
        let out = runner.run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(120).with_stdin(body.clone()),
        )?;
        let dur = started.elapsed().as_millis() as u64;

        // 4d. Audit entry. fields (transfer_*) record the
        // direction + sha + bytes; fields (revert) record the
        // inverse to restore on `inspect revert <id>`.
        let mut entry = AuditEntry::new("put", &label);
        entry.args = local_path.clone();
        entry.previous_hash = prev_hash.clone().map(|h| format!("sha256:{h}"));
        entry.new_hash = Some(format!("sha256:{new_hash}"));
        entry.snapshot = prev_hash
            .clone()
            .map(|h| snaps.path_for(&h).display().to_string());
        entry.diff_summary =
            diff_summary(&[(prev_text.clone().unwrap_or_default(), new_text.clone())]);
        entry.exit = out.exit_code;
        entry.duration_ms = dur;
        entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        entry.transfer_direction = Some("up".into());
        entry.transfer_local = Some(local_path.clone());
        entry.transfer_remote = Some(path.clone());
        entry.transfer_bytes = Some(body.len() as u64);
        entry.transfer_sha256 = Some(format!("sha256:{new_hash}"));
        entry.revert = Some(match prev_hash.as_ref() {
            Some(h) => Revert::state_snapshot(
                format!("sha256:{h}"),
                format!("restore {label} from snapshot sha256:{}", &h[..12]),
            ),
            None => Revert::command_pair(
                // payload = literal command the runner dispatches;
                // preview = human-readable. v0.1.3 smoke caught
                // these reversed: revert of a put-create dispatched
                // the prose `inspect put created …` as a remote
                // shell command and failed with `command not
                // found`. Capture-site authoritative: payload is
                // always the runnable form.
                build_remote_rm(s, path),
                format!("rm {label} (created by put)"),
            ),
        });
        entry.applied = Some(out.ok());
        store.append(&entry)?;

        if out.ok() {
            ok += 1;
            renderer.data_line(format!(
                "{label}: pushed {} bytes (audit {})",
                body.len(),
                entry.id
            ));
        } else {
            bad += 1;
            renderer.data_line(format!(
                "{label}: FAILED (exit {}): {}",
                out.exit_code,
                out.stderr.trim()
            ));
        }
    }

    renderer
        .summary(format!("put: {ok} ok, {bad} failed"))
        .next("inspect audit ls")
        .next("inspect revert <audit-id> to undo");
    renderer.print();
    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}

// ---------------------------------------------------------------------------
// Pull (download) flow
// ---------------------------------------------------------------------------

fn pull(remote_sel: String, local: String) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&remote_sel)?;
    let steps: Vec<_> = iter_steps(&nses, &targets).collect();
    if steps.is_empty() {
        crate::error::emit(format!("'{remote_sel}' matched no targets"));
        return Ok(ExitKind::Error);
    }
    if steps.len() > 1 {
        crate::tee_eprintln!(
            "error: get requires exactly one source target; got {}",
            steps.len()
        );
        return Ok(ExitKind::Error);
    }
    let s = &steps[0];
    let Some(path) = s.path.clone() else {
        crate::error::emit("get requires a `:path` on the remote selector");
        return Ok(ExitKind::Error);
    };

    // base64-encode on the remote so binary content survives the
    // SSH stdout pipe (which goes through the runner's lossy-UTF8
    // string decode). Decoded locally back to bytes.
    let inner = format!("base64 -- {}", shquote(&path));
    let cmd = match s.container() {
        Some(container) => format!(
            "docker exec {} sh -c {}",
            shquote(container),
            shquote(&inner)
        ),
        None => inner,
    };
    let started = Instant::now();
    let out = runner.run(
        &s.ns.namespace,
        &s.ns.target,
        &cmd,
        RunOpts::with_timeout(120),
    )?;
    let dur = started.elapsed().as_millis() as u64;

    if !out.ok() {
        crate::tee_eprintln!(
            "error: pulling '{}' failed (exit {}): {}",
            path,
            out.exit_code,
            out.stderr.trim()
        );
        return Ok(ExitKind::Error);
    }
    let cleaned: String = out.stdout.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| anyhow::anyhow!("decoding remote payload: {e}"))?;
    let sha = sha256_hex(&bytes);

    if local == "-" {
        std::io::stdout().write_all(&bytes)?;
    } else {
        std::fs::write(&local, &bytes)?;
    }

    // Get is read-only on the remote, so the     // contract surfaces it as `revert.kind = unsupported` (the
    // operator can revert by deleting the local file but the audit
    // schema does not capture local-side state). We still record
    // bytes + sha256 so a later `put` of the same content is
    // verifiable byte-for-byte.
    let label = label_for(s, &path);
    let mut entry = AuditEntry::new("get", &label);
    entry.args = local.clone();
    entry.exit = out.exit_code;
    entry.duration_ms = dur;
    entry.transfer_direction = Some("down".into());
    entry.transfer_local = Some(local.clone());
    entry.transfer_remote = Some(path.clone());
    entry.transfer_bytes = Some(bytes.len() as u64);
    entry.transfer_sha256 = Some(format!("sha256:{sha}"));
    entry.revert = Some(Revert::unsupported(format!(
        "get is read-only on the remote; delete the local file '{local}' to undo"
    )));
    entry.applied = Some(true);
    AuditStore::open()?.append(&entry)?;

    let mut r = Renderer::new();
    r.summary(format!(
        "pulled {} bytes → {local} (audit {})",
        bytes.len(),
        entry.id
    ));
    r.next("inspect put <local> <sel>:<path>  to push back");
    r.print();
    Ok(ExitKind::Success)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn label_for(s: &Step<'_>, path: &str) -> String {
    format!(
        "{}{}:{path}",
        s.ns.namespace,
        s.service().map(|x| format!("/{x}")).unwrap_or_default()
    )
}

/// Best-effort fetch of remote file content via `cat`. Used for
/// dry-run diff rendering and for revert snapshot capture. Returns
/// `None` when the file does not exist or the read failed for any
/// reason — callers must treat missing as "no prior content".
fn read_remote(runner: &dyn RemoteRunner, s: &Step<'_>, path: &str) -> Option<String> {
    let inner = format!("cat -- {}", shquote(path));
    let cmd = match s.container() {
        Some(container) => format!(
            "docker exec {} sh -c {}",
            shquote(container),
            shquote(&inner)
        ),
        None => inner,
    };
    let out = runner
        .run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(20),
        )
        .ok()?;
    if out.ok() {
        Some(out.stdout)
    } else {
        None
    }
}

/// Build the `inspect rm` command-pair inverse for a put that
/// created a brand-new file. Mirrors the dispatch shape (host vs
/// container) so `inspect revert <id>` invokes the deletion on the
/// right side of the namespace boundary.
fn build_remote_rm(s: &Step<'_>, path: &str) -> String {
    let inner = format!("rm -f -- {}", shquote(path));
    match s.container() {
        Some(container) => format!(
            "docker exec {} sh -c {}",
            shquote(container),
            shquote(&inner)
        ),
        None => format!("sh -c {}", shquote(&inner)),
    }
}

/// Build the streaming atomic-write shell pipeline.
///
/// The pipeline:
/// 1. Reads stdin into `<tmp>` (the local file body comes through
///    SSH stdin via [`RunOpts::with_stdin`]).
/// 2. Optionally mirrors the prior file's mode + owner onto the
///    `<tmp>` so atomic rename does not silently widen permissions.
/// 3. Optionally applies the operator's `--mode` / `--owner`
///    overrides (which take precedence over the inherited mirror).
/// 4. Optionally creates missing parent dirs (`--mkdir-p`).
/// 5. Atomically `mv`'s `<tmp>` over the final `<path>`.
///
/// Failure of any step (`set -e`) leaves the remote filesystem
/// either at the prior content or at the freshly-mv'd content; the
/// `<tmp>` does not survive.
pub(crate) fn build_stream_atomic_script(
    tmp: &str,
    path: &str,
    mkdir_p: bool,
    mode: Option<&str>,
    owner: Option<&str>,
) -> String {
    let tmp_q = shquote(tmp);
    let path_q = shquote(path);
    // G9 (v0.1.3): `set -C` enables the shell's `noclobber` flag so
    // the `cat > <tmp>` redirect uses `O_EXCL` semantics — it refuses
    // to follow a pre-existing symlink at the tmp path or to overwrite
    // a regular file. This closes a symlink-race window where an
    // attacker with write access to the tmp's parent directory could
    // pre-stage `<tmp>` as a symlink to a sensitive target before
    // inspect's redirect runs. A stale tmp from a crashed prior
    // upload is now a hard failure (operator must clean up manually);
    // that is the intended trade-off vs silently overwriting.
    let mut script = String::from("set -e; set -C; ");
    if mkdir_p {
        // Compute the parent dir on the remote and mkdir -p it.
        // dirname is POSIX; portable across coreutils + busybox.
        script.push_str(&format!("mkdir -p -- \"$(dirname -- {path_q})\"; "));
    }
    // 1. Stream stdin into the temp file.
    script.push_str(&format!("cat > {tmp_q}; "));
    // 2. Mirror prior mode/owner if path exists. POSIX-portable
    //    `stat -c` form (works on GNU coreutils and BusyBox / Alpine);
    //    pre-v0.1.3 used `chmod --reference` which is GNU-only and
    //    spewed BusyBox usage on every Alpine put/cp — release-smoke
    //    find on arte/inspect-smoke-* against `nginx:alpine`. Mode
    //    preservation is required (failure aborts via `set -e`);
    //    chown is best-effort (root-only) so we tolerate failure.
    script.push_str(&format!(
        "if [ -e {path_q} ]; then \
            chmod \"$(stat -c '%a' {path_q})\" {tmp_q}; \
            chown \"$(stat -c '%u:%g' {path_q})\" {tmp_q} 2>/dev/null || true; \
        fi; "
    ));
    // 3. Apply operator overrides (after mirror so they win).
    if let Some(m) = mode {
        // `--mode` accepts plain octal (e.g. `0644` or `644`); chmod
        // accepts both. shquote defends against shell metacharacters
        // even though the value is operator-supplied.
        let m_q = shquote(m);
        script.push_str(&format!("chmod {m_q} {tmp_q}; "));
    }
    if let Some(o) = owner {
        let o_q = shquote(o);
        script.push_str(&format!("chown {o_q} {tmp_q}; "));
    }
    // 4. Atomic rename.
    script.push_str(&format!("mv {tmp_q} {path_q}"));
    script
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_remote_recognizes_selector_with_path() {
        assert!(looks_remote("arte:/etc/foo"));
        assert!(looks_remote("arte/_:/etc/foo"));
        assert!(looks_remote("arte/atlas:/etc/foo"));
    }

    #[test]
    fn looks_remote_rejects_local_paths() {
        assert!(!looks_remote("./local"));
        assert!(!looks_remote("/etc/foo"));
        assert!(!looks_remote("local.txt"));
        // Bare selector with no `:path` — not a remote endpoint here.
        assert!(!looks_remote("arte"));
    }

    #[test]
    fn looks_remote_requires_absolute_path_after_colon() {
        // `arte:relative` is not a transfer endpoint; the `:` could
        // be a Windows drive letter or a typo. Conservative: only
        // accept absolute remote paths.
        assert!(!looks_remote("arte:relative"));
    }

    #[test]
    fn atomic_script_streams_stdin_into_tmp() {
        let s = build_stream_atomic_script("/etc/foo.tmp", "/etc/foo", false, None, None);
        assert!(s.contains("cat > '/etc/foo.tmp'"));
        assert!(s.contains("mv '/etc/foo.tmp' '/etc/foo'"));
    }

    #[test]
    fn atomic_script_mirrors_prior_mode_and_owner() {
        let s = build_stream_atomic_script("/p.tmp", "/p", false, None, None);
        assert!(s.contains("chmod \"$(stat -c '%a' '/p')\" '/p.tmp'"));
        assert!(s.contains("chown \"$(stat -c '%u:%g' '/p')\" '/p.tmp' 2>/dev/null || true"));
        // GNU-only `--reference` form must be gone.
        assert!(!s.contains("--reference="));
    }

    #[test]
    fn atomic_script_applies_mode_override_after_mirror() {
        let s = build_stream_atomic_script("/p.tmp", "/p", false, Some("0755"), None);
        // override appears AFTER the mirror block, before mv
        let mirror_idx = s.find("chmod \"$(stat").unwrap();
        let override_idx = s.find("chmod '0755'").unwrap();
        let mv_idx = s.find("mv ").unwrap();
        assert!(mirror_idx < override_idx);
        assert!(override_idx < mv_idx);
    }

    #[test]
    fn atomic_script_applies_owner_override() {
        let s = build_stream_atomic_script("/p.tmp", "/p", false, None, Some("root:adm"));
        assert!(s.contains("chown 'root:adm' '/p.tmp'"));
    }

    #[test]
    fn atomic_script_mkdir_p_targets_parent() {
        let s = build_stream_atomic_script("/a/b/c.tmp", "/a/b/c", true, None, None);
        assert!(s.contains("mkdir -p -- \"$(dirname -- '/a/b/c')\""));
        // mkdir runs BEFORE the cat (so the tmp's parent exists).
        let mkdir_idx = s.find("mkdir -p").unwrap();
        let cat_idx = s.find("cat >").unwrap();
        assert!(mkdir_idx < cat_idx);
    }

    #[test]
    fn atomic_script_no_mkdir_when_flag_off() {
        let s = build_stream_atomic_script("/a/b.tmp", "/a/b", false, None, None);
        assert!(!s.contains("mkdir -p"));
    }

    #[test]
    fn g9_atomic_script_uses_noclobber() {
        // G9 (v0.1.3): `set -C` must be enabled before the `cat >`
        // redirect so a pre-existing symlink at the tmp path causes
        // the redirect to fail rather than be silently followed.
        let s = build_stream_atomic_script("/etc/foo.tmp", "/etc/foo", false, None, None);
        let nocl = s.find("set -C").expect("set -C must be present");
        let cat = s.find("cat >").expect("cat redirect must be present");
        assert!(
            nocl < cat,
            "set -C must precede the cat redirect; script={s}"
        );
    }
}
