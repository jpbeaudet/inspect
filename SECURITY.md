# Security policy

## Reporting a vulnerability

**Please do not open a public issue for security reports.**

Instead, send a private report:

1. Use GitHub's [private vulnerability reporting](https://github.com/jpbeaudet/inspect/security/advisories/new)
   on this repo (preferred), or
2. Email the maintainer listed on the GitHub profile of
   [@jpbeaudet](https://github.com/jpbeaudet).

Include:

- The version of `inspect` (`inspect --version`).
- Your OS and architecture.
- A minimal reproduction (smallest input that triggers the issue).
- The expected vs. actual behavior.
- Any draft patch you may already have.

You should receive an acknowledgement within a few business days. If
you do not, please re-send through the alternate channel above.

## Disclosure timeline

The default coordinated-disclosure window is **90 days** from the
acknowledgement. Critical issues (active exploitation, secret
leakage) may be disclosed faster after a hotfix is available.

## Scope

In scope:

- Secret leakage in `inspect` output, profile files, or audit log.
- Authentication / authorization bypasses against the local SSH or
  Docker boundary.
- Arbitrary command execution that bypasses the `--apply` /
  `--allow-exec` safety gates.
- Path traversal in `cp`, `edit`, or any read verb.
- Supply-chain integrity of release artifacts (cosign, SHA-256).
- Crashes that leak memory or file contents.

Out of scope:

- Issues that require pre-existing root on the operator host.
- Misconfigurations of the operator's own `~/.ssh/config`.
- Issues that affect only the v2 deferred features (TUI, k8s, password
  auth, etc.) — those are not yet shipped.

## Hardening that is part of the contract

These properties are tested and considered breakage if regressed:

- All profile files are written mode `0600`.
- Mutating verbs are dry-run by default.
- `inspect exec` requires `--allow-exec` in addition to `--apply`.
- Output is run through a deterministic redactor before printing or
  logging.
- Snapshots written before mutation make every `--apply` reversible.

If you find a way to bypass any of these, that is a security bug —
please report it.

## Supported versions

| Version | Status |
|---|---|
| `0.1.x` | active |
| anything older | unsupported |

Hotfixes are issued as `0.1.Z` patch releases per the runbook
(`docs/RUNBOOK.md` §2).
