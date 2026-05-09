//! F18 (v0.1.3): per-namespace, per-day human-readable transcript of
//! every verb invocation and its output.
//!
//! The transcript is a **complement** to the structured audit log
//! (`~/.inspect/audit/`). Audit log answers "what verbs ran with
//! what arguments + what changed"; transcript answers "what did I
//! see on my terminal during the 4-hour migration?". One file per
//! namespace per UTC day under `~/.inspect/history/<ns>-<YYYY-MM-DD>.log`,
//! mode 0600.
//!
//! Each verb invocation produces one fenced block:
//!
//! ```text
//! ── 2026-04-28T14:32:11Z arte run #b8e3a1 ──────────────────────────
//! $ inspect run arte -- 'docker ps --format "{{.Names}}"'
//! arte | atlas-vault
//! arte | atlas-pg
//! arte | aware
//! ── exit=0 duration=423ms audit_id=01HXR9Q5YQK2 ──
//! ```
//!
//! The fence pattern (`── … ──` lines) is `awk '/^── /,/^── exit=/'`
//! friendly so block extraction stays trivial. The trailing
//! `audit_id=` field cross-links back to the structured audit entry
//! for forensic round-trip.
//!
//! ## Design
//!
//! Each `inspect <verb>` invocation is a separate process. The
//! transcript writer is therefore per-process, lives in a
//! `OnceLock<Mutex<Option<TranscriptContext>>>` global, is installed
//! at the top of `main.rs::main()`, and is finalized at process exit.
//! All user-visible `println!` / `eprintln!` calls inside the
//! rendering layer route through this module's `emit_stdout` /
//! `emit_stderr` helpers, which tee the line both to the real fd AND
//! to the in-memory transcript buffer.
//!
//! Subprocess output (e.g. `ssh ...`) flows through the streaming
//! line-emit code paths (F16) which themselves call `emit_stdout`,
//! so the transcript captures interleaved subprocess output the same
//! way the operator's terminal saw it.
//!
//! ## Performance
//!
//! Output is accumulated in memory during the verb and written in
//! one shot at finalize time. One `fdatasync(2)` per verb
//! invocation. A 10-minute streaming verb that produces 100 MB of
//! output produces exactly 1 fsync against the transcript file —
//! satisfies the F18 ≤ 70-fsyncs-per-10-min performance gate by
//! orders of magnitude.
//!
//! Buffer growth is capped at [`MAX_BUFFER_BYTES`] (16 MiB);
//! anything past that is replaced with a `[transcript truncated:
//! buffer cap reached]` marker so a runaway verb cannot OOM the
//! process. This is a soft cap chosen empirically: typical verb
//! output is < 100 KiB; the largest realistic streaming run a single
//! operator would do (`inspect logs --follow` for hours) tops out
//! around 5–10 MiB before the operator scrolls away.
//!
//! ## Redaction
//!
//! Every line tee'd to the transcript runs through the L7 four-masker
//! pipeline before being appended, using the per-namespace policy
//! resolved at `set_namespace` time. Per-namespace `redact = "off"`
//! disables redaction in the transcript only (not in stdout — stdout
//! redaction is governed by `--show-secrets`). This is a deliberate
//! split: an operator handling a forensic dump may want raw output
//! in the transcript file (file mode 0600 already restricts
//! exposure) while the L7 default still applies to everything else.
//!
//! ## Per-namespace disable
//!
//! Per-ns `[namespaces.<ns>.history].disabled = true` skips the
//! transcript write entirely. The audit log is still emitted — F18
//! and F11 are independent contracts.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Utc};

use crate::paths::{ensure_home, set_dir_mode_0700, set_file_mode_0600};

/// Soft cap on the per-invocation transcript buffer. See module docs.
pub const MAX_BUFFER_BYTES: usize = 16 * 1024 * 1024;

/// Per-namespace redaction mode resolved by [`set_namespace`].
/// Mirrors the spec's `[namespaces.<ns>.history].redact` value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RedactMode {
    /// L7 default: PEM / header / URL / env masking applied.
    #[default]
    Normal,
    /// Stricter: in addition to L7, mask any `<KEY>=<VALUE>` whose
    /// key looks secret-shaped, even if it doesn't match the
    /// per-masker patterns. Reserved for future tightening — for
    /// v0.1.3 this is identical to `Normal`.
    Strict,
    /// Disabled: write raw lines to the transcript without masking.
    /// Used by operators handling a forensic dump where the
    /// transcript file (mode 0600) is the trusted artifact.
    Off,
}

impl RedactMode {
    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "disabled" => Self::Off,
            "strict" => Self::Strict,
            _ => Self::Normal,
        }
    }
}

/// Per-process transcript state. Installed at `main()` entry and
/// finalized at process exit. Holds the buffered output, namespace
/// link, and audit-id link for the current invocation.
#[derive(Debug)]
pub struct TranscriptContext {
    /// Wall-clock start of the verb invocation. Used for the fence
    /// header timestamp and the duration calculation in the footer.
    pub started_at: DateTime<Utc>,
    /// `inspect <args>` line as the operator typed it (post-redaction).
    pub argv_line: String,
    /// Short identifier shown in the fence header (`#b8e3a1`-style).
    /// Locally unique per invocation — derived from the process pid +
    /// start-time millis to avoid collisions in tests that run many
    /// invocations in the same wall second.
    pub verb_token: String,
    /// Resolved namespace for this invocation. `None` until the verb
    /// resolves its target; `_global` if no namespace is meaningful
    /// (e.g. `inspect help`, `inspect audit ls`).
    pub namespace: Option<String>,
    /// Redaction mode for this verb's transcript writes. Resolved at
    /// `set_namespace` time from per-ns config; defaults to
    /// `RedactMode::Normal` until then.
    pub redact_mode: RedactMode,
    /// Per-namespace transcript-disabled flag. When true, finalize
    /// is a no-op (audit still writes; transcript stays silent).
    pub disabled: bool,
    /// First audit entry id appended during this invocation. For
    /// multi-audit verbs (e.g. F17 `--steps`) this is the parent
    /// `run.steps` id, not the per-step ids — the cross-link from
    /// transcript → audit is to the umbrella record.
    pub audit_id: Option<String>,
    /// Buffered output lines, ordered by emission. Each line is
    /// already L7-redacted (unless `redact_mode == Off`) and
    /// terminated by the writer with a `\n` at finalize time.
    pub buf: Vec<u8>,
    /// `true` once the buffer hit [`MAX_BUFFER_BYTES`] and was
    /// terminated with the truncation marker. Subsequent writes are
    /// no-ops.
    pub truncated: bool,
}

impl TranscriptContext {
    fn new(argv_line: String, started_at: DateTime<Utc>) -> Self {
        let token = format!(
            "#{:08x}",
            (started_at.timestamp_millis() as u64).wrapping_mul(2654435761)
                ^ std::process::id() as u64
        )
        .chars()
        .take(7)
        .collect();
        Self {
            started_at,
            argv_line,
            verb_token: token,
            namespace: None,
            redact_mode: RedactMode::Normal,
            disabled: false,
            audit_id: None,
            buf: Vec::with_capacity(4096),
            truncated: false,
        }
    }

    fn append(&mut self, line: &str) {
        if self.disabled || self.truncated {
            return;
        }
        let masked = match mask_for_transcript(line, self.redact_mode) {
            // None = the line was inside an active PEM block and the
            // L7 marker has already been emitted on the BEGIN line.
            // Skip emission to the transcript too — the marker is the
            // post-redaction record, the body bytes are not.
            None => return,
            Some(s) => s,
        };
        if self.buf.len() + masked.len() + 1 > MAX_BUFFER_BYTES {
            const MARKER: &str = "\n[transcript truncated: buffer cap reached]\n";
            self.buf.extend_from_slice(MARKER.as_bytes());
            self.truncated = true;
            return;
        }
        self.buf.extend_from_slice(masked.as_bytes());
        self.buf.push(b'\n');
    }
}

fn mask_for_transcript(line: &str, mode: RedactMode) -> Option<String> {
    if mode == RedactMode::Off {
        return Some(line.to_string());
    }
    // Reuse the L7 four-masker pipeline — same redaction semantics
    // as stdout. Each tee'd line gets a fresh redactor: the PEM
    // masker's multi-line state can't meaningfully span tee'd lines
    // (we receive lines after the upstream printer has already laid
    // them out one at a time). PEM blocks therefore reduce to
    // single-line masking — which is fine for the transcript: any
    // BEGIN-line tee will fire the marker, and an interior tee that
    // happens to land here standalone (without its BEGIN) is treated
    // as an unrecognized line and passes through. In practice the
    // L7 stdout pipeline produces the marker line and suppresses
    // interiors before they ever reach us.
    let r = crate::redact::OutputRedactor::new(false, false);
    r.mask_line(line).map(|cow| cow.into_owned())
}

static TRANSCRIPT: OnceLock<Mutex<Option<TranscriptContext>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<TranscriptContext>> {
    TRANSCRIPT.get_or_init(|| Mutex::new(None))
}

/// Initialize the per-process transcript context from the operator's
/// argv. Should be called once at the top of `main()`. Subsequent
/// calls are ignored — the test harness sometimes spawns the binary
/// multiple times in the same process under cargo's harness, and we
/// don't want the second init to clobber the first.
pub fn init(argv: &[String]) {
    let argv_line = redact_argv_line(argv);
    let now = Utc::now();
    let mut g = slot().lock().unwrap_or_else(|p| p.into_inner());
    if g.is_none() {
        *g = Some(TranscriptContext::new(argv_line, now));
    }
}

/// Build the `$ inspect ...` line shown at the top of each fenced
/// block. Argv is shell-quoted to round-trip exotic operator inputs.
/// Secret-shaped tokens (`--password=...`) are masked here too so
/// the transcript never carries credentials passed on the command
/// line — defense-in-depth on top of the L7 stdout pipeline.
fn redact_argv_line(argv: &[String]) -> String {
    let mut out = String::with_capacity(64 + argv.iter().map(|a| a.len()).sum::<usize>());
    out.push_str("$ ");
    for (i, a) in argv.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let display = if let Some(rest) = a.strip_prefix("--password=") {
            format!("--password=<redacted:{}>", rest.len())
        } else if let Some(rest) = a.strip_prefix("--token=") {
            format!("--token=<redacted:{}>", rest.len())
        } else {
            a.clone()
        };
        out.push_str(&shell_quote(&display));
    }
    out
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let needs_quote = s
        .chars()
        .any(|c| !matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | '/' | '=' | ':' | '@' | ','));
    if !needs_quote {
        return s.to_string();
    }
    let mut q = String::with_capacity(s.len() + 2);
    q.push('\'');
    for c in s.chars() {
        if c == '\'' {
            q.push_str("'\\''");
        } else {
            q.push(c);
        }
    }
    q.push('\'');
    q
}

/// Resolve the namespace + redaction mode + per-ns disabled flag for
/// this invocation. Called once the verb's selector resolution lands
/// on a concrete namespace; idempotent for repeated calls (last
/// caller wins for the rare case where one verb crosses namespaces).
pub fn set_namespace(ns: &str) {
    let mut g = slot().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(ctx) = g.as_mut() {
        ctx.namespace = Some(ns.to_string());
        let (mode, disabled) = resolve_per_ns_policy(ns);
        ctx.redact_mode = mode;
        ctx.disabled = disabled;
    }
}

/// Cross-link from transcript → audit. Called once per audit append.
/// First-write-wins so the transcript's footer points at the
/// **parent** entry on multi-audit verbs (F17 `--steps`).
pub fn set_audit_id(id: &str) {
    let mut g = slot().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(ctx) = g.as_mut() {
        if ctx.audit_id.is_none() {
            ctx.audit_id = Some(id.to_string());
        }
    }
}

/// P8-C test helper (v0.1.3): observe the linked audit_id without
/// going through `finalize`. Lets unit tests verify the contract that
/// `AuditStore::append_without_transcript_link` does NOT clobber the
/// transcript footer. Cfg-gated to keep production builds free of
/// this introspection surface.
#[cfg(test)]
pub fn audit_id_for_test() -> Option<String> {
    let g = slot().lock().unwrap_or_else(|p| p.into_inner());
    g.as_ref().and_then(|ctx| ctx.audit_id.clone())
}

/// P8-C test helper (v0.1.3): force-reset the transcript context so
/// tests can run in isolation against the global slot. Pairs with
/// `audit_id_for_test`. Cfg-gated.
#[cfg(test)]
pub fn reset_for_test() {
    let mut g = slot().lock().unwrap_or_else(|p| p.into_inner());
    *g = None;
}

/// Tee a stdout line. Called by the rendering layer's `emit_*`
/// helpers — the operator-visible side-effect (println!) is the
/// caller's responsibility; this side stays silent on stdout and
/// only adds to the transcript buffer.
pub fn tee_stdout(line: &str) {
    let mut g = slot().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(ctx) = g.as_mut() {
        ctx.append(line);
    }
}

/// Tee a stderr line. Stderr emissions are inlined in the transcript
/// body without a stream marker — the operator saw them interleaved
/// with stdout on their terminal and the transcript should match.
pub fn tee_stderr(line: &str) {
    let mut g = slot().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(ctx) = g.as_mut() {
        ctx.append(line);
    }
}

/// Emit `line` to stdout AND to the transcript. Use this in central
/// rendering paths so callers don't have to remember to tee.
pub fn emit_stdout(line: &str) {
    println!("{line}");
    tee_stdout(line);
}

/// `println!`-shaped macro that tees its formatted output through
/// the transcript buffer in addition to writing to stdout. Used at
/// rendering layer call sites that already have rich format strings
/// where wrapping in `emit_stdout(&format!(...))` would obscure the
/// shape of the call.
#[macro_export]
macro_rules! tee_println {
    () => {{
        ::std::println!();
        $crate::transcript::tee_stdout("");
    }};
    ($($arg:tt)*) => {{
        let line = ::std::format!($($arg)*);
        ::std::println!("{}", line);
        $crate::transcript::tee_stdout(&line);
    }};
}

/// `eprintln!`-shaped macro that tees stderr writes into the
/// transcript buffer. Diagnostics, warnings, and pipeline status
/// messages should use this so the post-mortem captures what the
/// operator actually saw.
#[macro_export]
macro_rules! tee_eprintln {
    () => {{
        ::std::eprintln!();
        $crate::transcript::tee_stderr("");
    }};
    ($($arg:tt)*) => {{
        let line = ::std::format!($($arg)*);
        ::std::eprintln!("{}", line);
        $crate::transcript::tee_stderr(&line);
    }};
}

/// Finalize the transcript: write the fenced block to the per-day
/// transcript file. Best-effort — a failure to write does not
/// propagate (matches the `_ = maybe_run_lazy_gc()` discipline in
/// `AuditStore::append`).
///
/// `exit_code` is the verb's final exit code (0 for success).
/// `duration` is the wall-clock duration since `init()`.
pub fn finalize(exit_code: i32) {
    let mut g = slot().lock().unwrap_or_else(|p| p.into_inner());
    let Some(ctx) = g.take() else {
        return;
    };
    // F18 spec: "Every verb invocation **against a namespace**" gets
    // a transcript. Verbs that never resolve a namespace (`inspect
    // help`, `inspect list`, `inspect audit ls`, `inspect history
    // ...` etc.) do not produce transcript files — they're
    // operator-tooling verbs whose output is not the kind the spec
    // is trying to preserve for post-mortem. Skipping here also
    // avoids polluting `~/.inspect/history/` with `_global-*.log`
    // files that would always be near-empty.
    if ctx.namespace.is_none() {
        return;
    }
    if ctx.disabled {
        // Best-effort lazy rotation still fires even on disabled
        // namespaces so a once-disabled namespace's old transcripts
        // age out per the global retention policy.
        let _ = crate::transcript::rotate::maybe_run_lazy();
        return;
    }
    let _ = write_block(&ctx, exit_code);
    // Lazy rotation: best-effort, errors swallowed.
    let _ = crate::transcript::rotate::maybe_run_lazy();
}

fn write_block(ctx: &TranscriptContext, exit_code: i32) -> std::io::Result<()> {
    use std::io::Write;
    let _ = ensure_home();
    let dir = history_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    }
    let _ = set_dir_mode_0700(&dir);
    let ns = ctx.namespace.as_deref().unwrap_or("_global");
    let date = ctx.started_at.format("%Y-%m-%d").to_string();
    let path = dir.join(format!("{ns}-{date}.log"));

    let mut block = Vec::with_capacity(ctx.buf.len() + 256);
    let header = format!(
        "── {ts} {ns} {token} ──────────────────────────\n",
        ts = ctx
            .started_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ns = ns,
        token = ctx.verb_token,
    );
    block.extend_from_slice(header.as_bytes());
    // SMOKE 2026-05-09 fix: route the argv line through the same L7
    // four-masker the body lines use. Pre-fix, `redact_argv_line`
    // only masked `--password=`/`--token=` prefixes, so any embedded
    // credential in a shell-quoted arg leaked verbatim to the
    // transcript file. Surfaced live during P6.F18.2: four `Bearer`
    // leaks in the form `$ inspect run arte -- 'echo
    // "Authorization: Bearer abc..."'` (the L7 synthetic test typed
    // a literal Bearer in argv to verify body redaction; body lines
    // were correctly redacted but the argv line itself was not).
    // The F18 design says "every line tee'd to the transcript runs
    // through the L7 four-masker pipeline before being appended" —
    // argv-line treatment was the gap.
    let masked_argv = match mask_for_transcript(&ctx.argv_line, ctx.redact_mode) {
        Some(s) => s,
        None => ctx.argv_line.clone(),
    };
    block.extend_from_slice(masked_argv.as_bytes());
    block.push(b'\n');
    block.extend_from_slice(&ctx.buf);
    if !ctx.buf.is_empty() && !ctx.buf.ends_with(b"\n") {
        block.push(b'\n');
    }
    let dur_ms = duration_ms_since(ctx.started_at);
    let footer = match &ctx.audit_id {
        Some(id) => format!("── exit={exit_code} duration={dur_ms}ms audit_id={id} ──\n\n",),
        None => format!("── exit={exit_code} duration={dur_ms}ms ──\n\n",),
    };
    block.extend_from_slice(footer.as_bytes());

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    f.write_all(&block)?;
    f.sync_data().ok();
    let _ = set_file_mode_0600(&path);
    Ok(())
}

fn duration_ms_since(start: DateTime<Utc>) -> i64 {
    let now = Utc::now();
    (now - start).num_milliseconds().max(0)
}

/// Path to `~/.inspect/history/` (mode 0700; per-file 0600).
pub fn history_dir() -> PathBuf {
    crate::paths::inspect_home().join("history")
}

/// Resolve per-namespace history policy from
/// `~/.inspect/servers.toml`'s `[namespaces.<ns>.history]` block.
/// Returns `(redact_mode, disabled)` — defaults to `(Normal, false)`
/// when the per-ns block is absent.
fn resolve_per_ns_policy(ns: &str) -> (RedactMode, bool) {
    let Ok(file) = crate::config::file::load() else {
        return (RedactMode::Normal, false);
    };
    let Some(nsc) = file.namespaces.get(ns) else {
        return (RedactMode::Normal, false);
    };
    let h = nsc.history.as_ref();
    let mode = h
        .and_then(|h| h.redact.as_deref())
        .map(RedactMode::from_str_lossy)
        .unwrap_or_default();
    let disabled = h.and_then(|h| h.disabled).unwrap_or(false);
    (mode, disabled)
}

pub mod rotate;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_passes_simple_through() {
        assert_eq!(shell_quote("foo"), "foo");
        assert_eq!(shell_quote("foo-bar"), "foo-bar");
        assert_eq!(shell_quote("a/b.c"), "a/b.c");
    }

    #[test]
    fn shell_quote_wraps_special_chars() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn redact_mode_parses_from_str() {
        assert_eq!(RedactMode::from_str_lossy("normal"), RedactMode::Normal);
        assert_eq!(RedactMode::from_str_lossy("strict"), RedactMode::Strict);
        assert_eq!(RedactMode::from_str_lossy("off"), RedactMode::Off);
        assert_eq!(RedactMode::from_str_lossy("OFF"), RedactMode::Off);
        assert_eq!(RedactMode::from_str_lossy("disabled"), RedactMode::Off);
        assert_eq!(RedactMode::from_str_lossy("nonsense"), RedactMode::Normal);
    }

    #[test]
    fn redact_argv_masks_password_and_token() {
        let argv = vec![
            "inspect".to_string(),
            "connect".to_string(),
            "--password=hunter2".to_string(),
        ];
        let line = redact_argv_line(&argv);
        assert!(line.contains("--password=<redacted:7>"));
        assert!(!line.contains("hunter2"));
    }

    /// SMOKE 2026-05-09 regression: the argv line in the transcript
    /// file must run through the L7 four-masker, not just the
    /// `--password=`/`--token=` prefix masker. Pre-fix, an operator
    /// who typed a credential as part of a shell-quoted arg (e.g.
    /// `inspect run arte -- 'echo "Authorization: Bearer abc..."'`)
    /// would see the bearer leak verbatim into
    /// `~/.inspect/history/<ns>-<date>.log`. Surfaced live during
    /// P6.F18.2 (4 Bearer leaks). Verified at the
    /// `mask_for_transcript` chokepoint.
    #[test]
    fn p6_argv_line_runs_through_l7_four_masker() {
        // Bearer in a shell-quoted arg.
        let argv = vec![
            "inspect".to_string(),
            "run".to_string(),
            "arte".to_string(),
            "--".to_string(),
            "echo \"Authorization: Bearer abcdef0123456789xyz\"".to_string(),
        ];
        let raw = redact_argv_line(&argv);
        // The pre-fix shape — argv masker only handles --password=/--token=.
        assert!(
            raw.contains("Bearer abcdef0123456789xyz"),
            "redact_argv_line alone shouldn't strip non-flag credentials: {raw}"
        );
        // After the four-masker pass at write time:
        let masked = mask_for_transcript(&raw, RedactMode::Normal).unwrap();
        assert!(
            !masked.contains("Bearer abcdef"),
            "L7 four-masker must redact bearer tokens in the argv line: {masked}"
        );
        assert!(
            masked.contains("Authorization: <redacted>"),
            "L7 four-masker should rewrite to <redacted>: {masked}"
        );
    }

    #[test]
    fn append_respects_truncation_cap() {
        let mut ctx = TranscriptContext::new("$ inspect run".into(), Utc::now());
        // Simulate buffer near cap.
        let chunk = "x".repeat(MAX_BUFFER_BYTES - 32);
        ctx.append(&chunk);
        assert!(!ctx.truncated);
        ctx.append(&"y".repeat(64));
        assert!(ctx.truncated, "second large append should trip the cap");
        ctx.append("ignored after truncation");
        assert!(
            !String::from_utf8_lossy(&ctx.buf).contains("ignored"),
            "post-truncation writes should be no-ops"
        );
    }

    #[test]
    fn append_skips_when_disabled() {
        let mut ctx = TranscriptContext::new("$ inspect run".into(), Utc::now());
        ctx.disabled = true;
        ctx.append("nothing");
        assert!(ctx.buf.is_empty());
    }
}
