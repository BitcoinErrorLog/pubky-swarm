# Releasing Torky

Production releases are built from immutable `vMAJOR.MINOR.PATCH` tags by
`.github/workflows/release.yml`. The workflow builds on native Linux, macOS, and
Windows runners, signs the resulting installers, generates a CycloneDX SBOM and
SHA-256 manifest, creates a GitHub artifact attestation, and opens a **draft**
GitHub Release. A maintainer must verify the draft before publishing it.

The workflow is intentionally fail-closed. Missing credentials, invalid native
signatures, missing timestamps, failed notarization, absent bundles, or a
version mismatch stop the release. It never falls back to unsigned production
artifacts.

## Repository setup

Create a GitHub Environment named `production-release`. Protect it with required
reviewers, limit deployment branches and tags to the release policy, and store
the production credentials there. The release jobs all target this environment.

Configure these environment secrets:

- `APPLE_CERTIFICATE`: base64-encoded Developer ID Application `.p12`.
- `APPLE_CERTIFICATE_PASSWORD`: export password for that `.p12`.
- `APPLE_SIGNING_IDENTITY`: exact Developer ID Application identity.
- `APPLE_ID`: Apple account used for notarization.
- `APPLE_PASSWORD`: app-specific password for that Apple account.
- `APPLE_TEAM_ID`: Apple Developer team identifier.
- `WINDOWS_CERTIFICATE`: base64-encoded, exportable Authenticode `.pfx`.
- `WINDOWS_CERTIFICATE_PASSWORD`: export password for that `.pfx`.
- `TAURI_SIGNING_PRIVATE_KEY`: Tauri updater/signing private-key content.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: password protecting that key.

Configure `TAURI_SIGNING_PUBLIC_KEY` as an environment **variable**, not a
secret. It is the base64 public-key value emitted by the Tauri signer and is
safe to publish. The workflow requires it so releases cannot proceed without a
declared verification key.

Do not put signing material in repository files, workflow inputs, command-line
arguments, release assets, caches, or logs. Keep offline backups of the Tauri
private key and its password in separate controlled locations. Losing this key
prevents compatible updates for clients that pin its public key. Compromise
requires a new application release that pins a new public key; an updater key
cannot be silently replaced for already-installed clients.

The Windows workflow currently expects an exportable PFX. Hardware-backed EV
certificates and cloud signing services require a Tauri `signCommand` integration
and are not supported by this skeleton. The Apple certificate must be valid for
Developer ID distribution, and the Apple account must be authorized to
notarize for `APPLE_TEAM_ID`.

Artifact attestations for private repositories require an eligible GitHub plan.
Confirm that the repository has artifact attestations enabled before the first
production run.

## Creating a release

1. Update all three application versions to the same semantic version:
   `Cargo.toml` under `[workspace.package]`, `apps/desktop/package.json`, and
   `apps/desktop/src-tauri/tauri.conf.json`.
2. Run CI and the dependency/license audit on the exact commit.
3. Create and push an immutable `vMAJOR.MINOR.PATCH` tag.
4. Approve the `production-release` environment deployment.
5. Wait for all platform builds and the `Attest and create draft release` job.
6. Download the finalized workflow artifact and verify it locally.
7. Inspect the draft release assets, SBOM, checksums, signatures, and generated
   notes. Publish the draft only after verification.

`workflow_dispatch` can rebuild an existing tag. It does not create tags and
refuses to overwrite an existing GitHub Release.

## Local build checks

The repository has a local external-volume Cargo target override. Release and CI
commands deliberately override it and use an ordinary runner-local directory;
they do not assume that the APFS mount workaround exists.

From the repository root:

```bash
export CARGO_TARGET_DIR="$PWD/target"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --exclude pubky-adapter --exclude dataset-homeserver
cd apps/desktop
npm ci
npm run build
npm run tauri build -- --debug --no-bundle --ci
```

The two Pubky integration packages require PostgreSQL:

```bash
export TEST_PUBKY_CONNECTION_STRING='postgres://USER:PASSWORD@127.0.0.1:5432/postgres?pubky-test=true'
cargo test -p pubky-adapter -p dataset-homeserver
```

Use a dedicated local database role and password. Do not reuse the example
variable names as credentials or commit a connection string.

## Verifying release artifacts

Download the complete finalized artifact without renaming or removing files.
Install `minisign` and set `TAURI_SIGNING_PUBLIC_KEY` to the published Tauri
public-key value:

```bash
export TAURI_SIGNING_PUBLIC_KEY='the published base64 public-key value'
scripts/release/verify-artifacts.sh /path/to/release-assets
```

The script verifies every entry in `SHA256SUMS`, decodes Tauri's base64 key and
signature wrappers, and verifies every `.sig` sidecar with Minisign. A checksum
or signature failure invalidates the release set.

Also verify the native platform signature:

macOS:

```bash
codesign --verify --deep --strict --verbose=2 /path/to/Torky.app
xcrun stapler validate /path/to/Torky.app
spctl --assess --type execute --verbose=2 /path/to/Torky.app
xcrun stapler validate /path/to/Pubky\ Swarm.dmg
```

Windows PowerShell:

```powershell
$signature = Get-AuthenticodeSignature .\Torky-installer.exe
$signature.Status
$signature.SignerCertificate
$signature.TimeStamperCertificate
```

`Status` must be `Valid`, and both certificate fields must be present. Inspect
the signer subject and certificate chain rather than checking status alone.

Linux packages currently have SHA-256, detached Tauri/Minisign signatures, and
GitHub provenance, but no distribution-native repository signature. Verify the
detached signature before installation.

The release SBOM is `torky-vVERSION.sbom.cdx.json`. Inspect it with a
CycloneDX-compatible tool and compare it to the lockfiles. Verify GitHub
provenance with the GitHub CLI against the repository:

```bash
gh attestation verify /path/to/artifact --repo BitcoinErrorLog/pubky-swarm
```

## Signing and updater key handling

Generate the Tauri keypair on a controlled offline machine:

```bash
cd apps/desktop
npm ci
npm run tauri signer generate -- -w /secure/offline/pubky-swarm.key
```

Store the private key content and password only in the protected environment and
offline backup. Publish the generated public value as
`TAURI_SIGNING_PUBLIC_KEY`. Before enabling an in-app updater, embed that exact
public value in the application and test update installation from a private
staging feed. Never regenerate a key merely because a CI secret was lost.

The current release workflow creates detached Tauri signatures, but the desktop
application does not yet configure the Tauri updater plugin, updater endpoints,
or `createUpdaterArtifacts`. Therefore these signatures support manual
verification only; they do not make automatic updates operational.

## Rollback

Git tags and published release assets are immutable. Do not replace an asset,
move a release tag, or reuse a version.

Before publication, delete or leave the failed draft and create a new tag only
after correcting the cause. After publication:

1. Mark the affected release as withdrawn in its release notes.
2. Direct users to the last known-good signed release and its checksum manifest.
3. Prepare a new patch version that corrects or reverts the change.
4. Run the complete release workflow for the new tag.

The application has no production updater or automated downgrade path today.
Rollback is a manually verified reinstall, and data/schema compatibility must be
assessed for each release before directing users to an older binary.

## Current limitations

- Full Pubky/Homeserver integration runs only on Linux CI because GitHub service
  containers are not available on hosted macOS and Windows runners. Portable
  workspace tests, frontend builds, and Tauri build smoke tests run on all
  three operating systems.
- Tauri desktop, mobile, Windows, and macOS icon assets are generated from
  `apps/desktop/app-icon.png`; production jobs must still verify each bundle
  renders the expected artwork.
- macOS releases are Apple Silicon builds from `macos-14`; universal or Intel
  bundles are not produced.
- Windows hardware-token and cloud signing are not configured.
- Linux package-manager repository metadata and native repository signing are
  not configured.
- Automatic updates, update manifests, staged rollout, and automatic rollback
  are not implemented.
- All workspace crates inherit the MIT license. RUSTSEC-2026-0194 and
  RUSTSEC-2026-0195 are remediated by version-matched, source-scoped
  `quick-xml` backports under `vendor/security-patches/`; the root
  `[patch.crates-io]` entries select them without changing the published
  0.37.5/0.38.4 versions. Each backport records its exact crates.io base and
  official security commit provenance in `SECURITY-BACKPORT.md`.
- Any vulnerability reported for a registry dependency remains a release
  blocker. Do not add advisory ignores or weaken the dependency checks to ship.
  Unmaintained informational advisories must be reported separately and require
  an upstream dependency upgrade or explicit release risk acceptance.
- Notarization, Authenticode timestamping, protected-environment approvals, and
  private-repository attestations depend on external provider configuration and
  must be proven by a real release run.
