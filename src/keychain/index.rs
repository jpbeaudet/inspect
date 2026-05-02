//! L2 (v0.1.3): on-disk namespace index for the OS keychain.
//!
//! `~/.inspect/keychain-index` (mode 0600) — one namespace per line,
//! sorted alphabetically. Holds **no** secret material; only names.
//! Atomic writes via `<file>.tmp.<pid>` → `rename(2)` so a crash
//! mid-write can't leave an empty index that nukes the operator's
//! record of saved entries.

use std::io::Write;
use std::path::Path;

/// Read the on-disk index. Returns an empty Vec when the file
/// does not exist (the common case before the first save).
pub(super) fn read(path: &Path) -> std::result::Result<Vec<String>, std::io::Error> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(parse(&s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Write the index atomically. Sorts the input first so the on-disk
/// shape is canonical regardless of the caller's order. Sets mode
/// 0600 on creation so no other user on the host can read which
/// namespaces this operator has saved (the names alone are minor
/// metadata leakage but we close the gap).
pub(super) fn write(path: &Path, names: &[String]) -> std::result::Result<(), std::io::Error> {
    super::ensure_home().map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut sorted: Vec<String> = names.to_vec();
    sorted.sort();
    sorted.dedup();
    let body = sorted.join("\n");
    // Atomic-write idiom: write to a sibling temp file then rename.
    // The rename(2) is atomic on POSIX, so a reader that lands
    // mid-write either sees the old file or the new one — never a
    // truncated half.
    let mut tmp = path.to_path_buf();
    let fname = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("keychain-index");
    let pid = std::process::id();
    tmp.set_file_name(format!(".{fname}.tmp.{pid}"));

    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(body.as_bytes())?;
            if !body.is_empty() {
                f.write_all(b"\n")?;
            }
            f.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(body.as_bytes())?;
            if !body.is_empty() {
                f.write_all(b"\n")?;
            }
        }
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn parse(body: &str) -> Vec<String> {
    let mut names: Vec<String> = body
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpfile() -> tempfile::NamedTempFile {
        tempfile::Builder::new()
            .prefix("inspect-keychain-index-")
            .tempfile()
            .unwrap()
    }

    #[test]
    fn l2_index_round_trip_preserves_sorted_names() {
        let f = tmpfile();
        let path = f.path();
        write(path, &["bravo".into(), "alpha".into(), "charlie".into()]).unwrap();
        let got = read(path).unwrap();
        assert_eq!(got, vec!["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn l2_index_dedups_duplicates() {
        let f = tmpfile();
        write(f.path(), &["arte".into(), "arte".into(), "bravo".into()]).unwrap();
        let got = read(f.path()).unwrap();
        assert_eq!(got, vec!["arte", "bravo"]);
    }

    #[test]
    fn l2_index_handles_empty() {
        let f = tmpfile();
        write(f.path(), &[]).unwrap();
        let got = read(f.path()).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn l2_index_missing_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist");
        let got = read(&path).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn l2_index_strips_blank_lines_and_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ix");
        std::fs::write(&path, "\n  arte  \nbravo\n\n  \n").unwrap();
        let got = read(&path).unwrap();
        assert_eq!(got, vec!["arte", "bravo"]);
    }

    #[cfg(unix)]
    #[test]
    fn l2_index_file_mode_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let f = tmpfile();
        write(f.path(), &["arte".into()]).unwrap();
        let mode = std::fs::metadata(f.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode = {mode:o}");
    }
}
