# Verifying win-nice artifacts

Every published win-nice artifact can be traced back to a specific git commit
in `PHPCraftdream/win-nice` without trusting the download page: the GitHub
Release assets carry artifact attestations produced by this repository's
release workflow, a checksum file covers all four build outputs, and the npm package
is published with npm provenance from the same commit. This document shows
how to check each of those claims yourself.

## What exists

| Artifact | Where | Covered by |
| --- | --- | --- |
| `wnice.exe` | GitHub Release | artifact attestation + `SHA256SUMS` |
| `win-nice-<version>.tgz` | GitHub Release | artifact attestation + `SHA256SUMS` |
| `rust-deps.cdx.json` | GitHub Release | artifact attestation + `SHA256SUMS` |
| `npm-deps.cdx.json` | GitHub Release | artifact attestation + `SHA256SUMS` |
| `SHA256SUMS` | GitHub Release | artifact attestation |
| `win-nice@<version>` | npm registry | registry signature + npm provenance |

The release workflow packs the npm tarball once, attests it, attaches it to
the draft GitHub Release and, as its last step, publishes that exact file to
npm - so the Release tarball and the npm registry package are the same bytes
by construction. Each copy is still verified through its own mechanism.

Artifact attestations and Release assets exist starting with version 0.4.0 -
earlier releases predate this pipeline, so these checks do not apply to them.

## Verify the GitHub artifact attestations

The release workflow attests all five files as GitHub artifact attestations
(SLSA provenance). Verification needs the GitHub CLI: the `gh attestation`
command set is generally available since gh v2.63 (checked against the gh
2.91 manual). From the directory holding the downloaded assets, run:

```
gh attestation verify wnice.exe --repo PHPCraftdream/win-nice
```

and repeat for the other four files (`win-nice-<version>.tgz`,
`rust-deps.cdx.json`, `npm-deps.cdx.json`, `SHA256SUMS`). The stricter form
pins the signer workflow, so an attestation produced by any other workflow in
the repository fails:

```
gh attestation verify wnice.exe --repo PHPCraftdream/win-nice --signer-workflow "PHPCraftdream/win-nice/.github/workflows/publish.yml"
```

`--format json` switches to machine-readable output:

```
gh attestation verify wnice.exe --repo PHPCraftdream/win-nice --format json
```

What a passing check means: GitHub confirmed that the file's digest matches
an attestation signed by this repository's workflow, and the attestation
payload pins the commit SHA the build ran on, the workflow path that produced
the file, and the build environment. A modified file, or a build that did not
run in this repository's workflow, fails verification.

## Verify the npm package (registry signatures + provenance)

npm packages published from this repository carry a registry signature and a
provenance attestation (npm Trusted Publishing / OIDC). `npm audit
signatures` verifies both for every downloaded package in a project; it works
with npm 11 (checked against the npm 11.21.1 CLI docs):

```
mkdir verify && cd verify && npm init -y && npm install win-nice@<version> && npm audit signatures
```

`npm audit signatures --json --include-attestations` includes the full
sigstore bundles in the output.

### Verify the tarball matches the registry

`npm view win-nice@<version> dist.integrity` prints the registry's integrity
string for the published version: `sha512-` followed by the SHA-512 digest
of the tarball, base64-encoded (SRI format; the same `dist.integrity` field
npm's audit-signature template references). Compare it with the Release
tarball by computing the same digest. In Git Bash on Windows (which ships
openssl):

```
openssl dgst -sha512 -binary win-nice-<version>.tgz | openssl base64 -A
```

Prepend `sha512-` when comparing. PowerShell alternative:

```
$sha512 = [System.Security.Cryptography.SHA512]::Create(); 'sha512-' + [Convert]::ToBase64String($sha512.ComputeHash([System.IO.File]::ReadAllBytes((Resolve-Path '.\win-nice-<version>.tgz').Path)))
```

Equal strings mean the registry copy is byte-identical to the Release
tarball.

The published package page also displays the provenance attestation.

## Verify SHA256SUMS

`SHA256SUMS` lists SHA-256 digests for the four build outputs, basenames
only. In Git Bash on Windows (or on Linux/macOS), from the download
directory:

```
sha256sum -c SHA256SUMS
```

PowerShell alternative: hash a single file and compare against the value in
`SHA256SUMS`:

```
(Get-FileHash -Algorithm SHA256 .\wnice.exe).Hash
```

## Compare the installed binary with the Release asset

A global npm install keeps the package - binary included - under npm's global
prefix. Its digest must match the Release asset's: the exe in the npm
package and the Release asset are copies of ONE build - the release workflow
stages a single binary and packs/attaches it.

```
sha256sum "$(npm prefix -g)/node_modules/win-nice/bin/wnice.exe"
```

Compare the printed digest with the `wnice.exe` line in `SHA256SUMS`.

## SBOMs

Two CycloneDX 1.5 JSON SBOMs ship with every release:

- `rust-deps.cdx.json` - every crate in `Cargo.lock`, with licenses and purls
  resolved from cargo metadata. Generated by the official CycloneDX Cargo
  plugin (`cargo-cyclonedx` 0.5.10 from crates.io,
  github.com/CycloneDX/cyclonedx-rust-cargo):
  `cargo cyclonedx -f json --spec-version 1.5
  --override-filename 'rust-deps.cdx'`.
- `npm-deps.cdx.json` - the package's npm dependency inventory, generated by
  npm's built-in command:
  `npm sbom --sbom-format cyclonedx --sbom-type application`.

Both are attested like the other Release assets. As a consumer you can use
them as a dependency inventory, review licenses, or match the component
identifiers against vulnerability feeds without unpacking the artifacts.

## What is attested vs what is not

- The Release is created as a **draft** and published manually by the
  maintainer. Attestations cover the artifact bytes and their provenance;
  they say nothing about a draft's publication status.
- The npm tarball is **not** claimed to be byte-reproducible (`npm pack` does
  not honor `SOURCE_DATE_EPOCH`).
- `wnice.exe` reproducibility is **checked, not claimed**: CI's `repro` job
  builds `wnice.exe` twice from two clean directories with the pinned recipe
  and records whether the sha256 digests match in its GitHub Step Summary.
  Until that job has run and reported a verdict, treat reproducibility as
  unmeasured. The job is deliberately non-blocking - read the summary and do
  not infer from green.
- `wnice.exe` carries no Authenticode signature (owner decision, deferred;
  see docs/RELEASING.md), so fresh downloads trigger the SmartScreen
  "unknown publisher" warning.
- `SOURCE_DATE_EPOCH` is not used anywhere in the release recipe (rustc
  ignores it, and no `cargo package` flow is in play).

## Linking artifacts to the source commit

The attestation payload contains the repository and the exact commit SHA the
build ran on. Two ways to read it:

- Run the verify command with `--format json` (above) and extract the commit
  from the predicate.
- GitHub UI: each release page has an "Attestations" view listing the
  attestations per artifact, with the commit and the workflow run linked.

Release tags are signed (see docs/RELEASING.md). To verify the tag against
your local clone:

```
git fetch origin tag vX.Y.Z && git tag -v vX.Y.Z
```

A good signature means the tag - and therefore the commit the workflow built
from - was created by the maintainer's key.
