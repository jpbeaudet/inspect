# Releasing `inspect`

Short version: **tag and push** — the rest is automated.

```sh
# from main, clean working tree
cargo test --locked
cargo build --release --locked

# bump version in Cargo.toml + CHANGELOG.md
git commit -am "release: v0.1.1"
git tag -s v0.1.1 -m "v0.1.1"
git push origin main
git push origin v0.1.1
```

The tag push triggers `.github/workflows/release.yml`, which:

1. Builds static-musl Linux (`x86_64`, `aarch64`) and Apple Darwin
   (`x86_64`, `aarch64`) tarballs.
2. Generates per-artifact `sha256` plus aggregate `SHA256SUMS`.
3. Signs each tarball with cosign keyless via GitHub OIDC.
4. Publishes a GitHub Release with all artifacts attached.
5. Optionally publishes to crates.io if the repo variable
   `PUBLISH_CRATE = "true"` and the secret `CARGO_REGISTRY_TOKEN` are
   both set.

## Hosting the install script

The install snippet in the README is:

```sh
curl -fsSL https://raw.githubusercontent.com/jpbeaudet/inspect/main/scripts/install.sh | sh
```

That URL is served directly by GitHub from the default branch of this
repo. **You do not need to host anything yourself.** Pushing
`scripts/install.sh` to `main` is the only deployment step.

If you ever rename the default branch, or want a vanity install URL,
either:

- Update the URL in the README to point at the new branch, or
- Add a CNAME / redirect on a domain you control (e.g.
  `https://get.example.com/inspect.sh` → the raw GitHub URL).
  No changes to the script are required.

## Cutting a hotfix

For a security or correctness fix shipped between minor releases:

1. Branch from the affected tag: `git checkout -b hotfix/0.1.1 v0.1.0`.
2. Land the smallest possible patch + a regression test.
3. Bump patch version in `Cargo.toml` and append a `## [0.1.1]` entry
   to `CHANGELOG.md`.
4. Tag `v0.1.1`, push, let the release workflow run.
5. Update the Homebrew formula sha256s (see below) if a tap is
   configured.
6. Operators upgrade with `scripts/install.sh --version v0.1.1`. The
   installer refuses to clobber a newer installed version unless
   `--force` is passed.

## Updating the Homebrew tap

The formula at [`packaging/homebrew/inspect.rb`](../packaging/homebrew/inspect.rb)
is a template that lives in this repo for reference. To publish:

1. Create `homebrew-tap` under your user/org on GitHub
   (`<owner>/homebrew-tap`).
2. After tagging, copy `packaging/homebrew/inspect.rb` to
   `<owner>/homebrew-tap/Formula/inspect.rb` and replace the four
   `__SHA256__*__` placeholders with the values from the release
   artifacts (`<artifact>.tar.gz.sha256`).
3. Bump the `version` line and commit to the tap repo.
4. Users then install with:

   ```sh
   brew tap <owner>/tap
   brew install inspect
   ```

> Do **not** publish to `homebrew/core` for v0.1.0 — `homebrew/core`
> requires notable usage and a stable release cadence.

## Verifying a release locally

```sh
# Checksum
shasum -a 256 -c inspect-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256

# Cosign keyless
cosign verify-blob \
  --certificate-identity-regexp 'https://github.com/jpbeaudet/inspect/.*' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  --certificate inspect-0.1.0-x86_64-unknown-linux-musl.tar.gz.pem \
  --signature   inspect-0.1.0-x86_64-unknown-linux-musl.tar.gz.sig \
  inspect-0.1.0-x86_64-unknown-linux-musl.tar.gz
```

## Post-release smoke

On a staging host:

```sh
inspect --version
inspect setup arte
inspect ps arte
inspect status arte
inspect search '{server="arte", source="logs"} |= "error"' --tail 50
```

If anything regresses, see [RUNBOOK.md](RUNBOOK.md) §3 for incident
handling and §2 for the hotfix flow.
