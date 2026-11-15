# Releasing

Maintainer-only procedure. A release is: a release PR, a merge, a signed tag,
and a manual review of the draft release CI produces. Everything on the tag
push is automatic; publishing the draft is not. How users verify the result
is documented in docs/VERIFYING.md.

## The procedure

### 1. Release PR

Bump the version in `Cargo.toml` and `package.json`, regenerate `Cargo.lock`,
and write the changelog section.

- `Cargo.lock`: run `cargo check` once without `--locked`; that refreshes the
  `wnice` entry. Commit the updated lockfile.
- `CHANGELOG.md`: add the `## [X.Y.Z] - YYYY-MM-DD` section and the compare
  link line at the bottom of the file:

  ```
  [X.Y.Z]: https://github.com/PHPCraftdream/win-nice/compare/v<PREV>...vX.Y.Z
  ```

  `scripts/release-check.js` (run in CI) enforces the heading format, date
  sanity, the `package.json` match, and that the compare-base tag exists.

### 2. Merge to master, wait for CI green

### 3. Optional rehearsal

Run publish.yml via `workflow_dispatch` on master. It runs everything a tag
push would run - through `SHA256SUMS`, extracting the release text (the
version is taken from `package.json`) and ending with `npm publish
--dry-run` of the same tarball. Only the attestation, the draft-release
creation, the real publish and the tag=version check are skipped. Useful
before the first tagged release or after workflow changes.

### 4. Create and push the signed tag

```
git tag -s vX.Y.Z -m "win-nice vX.Y.Z"
git tag -v vX.Y.Z
git push origin vX.Y.Z
```

`git tag -s vX.Y.Z -m "win-nice vX.Y.Z"` signs the tag (release tags are
signed as policy; `-s` needs a GPG key configured for git signing), and
`git tag -v` is the local self-check before pushing.

### 5. What runs automatically on a tag push

The tag push triggers one workflow, publish.yml, which runs the whole
pipeline in one job, in order:

- release build of `wnice.exe` with the pinned recipe
  (`RELEASE_TOOLCHAIN=1.97.0`, `--remap-path-prefix=<workspace>=/src`,
  `--locked`);
- `npm test`, then the Pester suite, then the repo's release-check script;
- `npm pack --pack-destination release-assets` - the tarball is packed once
  (`win-nice-<version>.tgz`, npm pinned at 11.5.1);
- `rust-deps.cdx.json` via `cargo cyclonedx -f json --spec-version 1.5
  --override-filename 'rust-deps.cdx'` (cargo-cyclonedx 0.5.10) and
  `npm-deps.cdx.json` via
  `npm sbom --sbom-format cyclonedx --sbom-type application`;
- `SHA256SUMS` (basenames only, `sha256sum`) over the four build outputs;
- all five files attested with actions/attest-build-provenance v4.2.2
  (GitHub artifact attestations, SLSA provenance);
- draft GitHub Release: release text extracted from the matching
  `CHANGELOG.md` section by `scripts/release-notes.js`, then:

  ```
  gh release create "$TAG" --draft --verify-tag --title "win-nice $TAG" --notes-file <file> <assets...>
  ```

  On a re-run a stale DRAFT of the same tag is deleted and recreated; a
  published release is never touched;
- `npm publish release-assets/win-nice-<version>.tgz --provenance --access
  public` via Trusted Publishing (OIDC) - the LAST step.

The registry tarball and the Release tarball are the same bytes by
construction (the published file is the packed tarball itself), and any
failure before the last step publishes nothing to npm. npm Trusted Publisher
is bound to the publish.yml workflow name, which is why publishing stays in
that file.

Publishing the draft is a manual owner action (step 7); it happens after npm
publishing.

ci.yml's `repro` job is not part of the release, but it reports whether two
clean builds with the pinned recipe are bit-identical - the verdict lands in
the job summary.

### 6. Verify before publishing the draft

Run the user-facing checks from docs/VERIFYING.md against the draft's
assets:

- `gh attestation verify <file> --repo PHPCraftdream/win-nice` for all five
  files;
- `sha256sum -c SHA256SUMS` in the download directory;
- `npm audit signatures` in a scratch install of the published package;
- the draft's release notes match the `CHANGELOG.md` section;
- the pushed tag shows "Verified" on GitHub.

On a re-run after a failed attempt the draft is recreated (idempotent), but
a published release of the same tag is an explicit error.

### 7. Publish the draft

In the GitHub UI, or:

```
gh release edit vX.Y.Z --draft=false
```

### 8. Post-publish

The npm package page shows the provenance attestation for the published
version. Nothing else to do: the Release, its assets and their attestations
are already public.

## Reproducibility notes

What is pinned, and why it is pinned there:

- The release recipe pins the toolchain explicitly (`RELEASE_TOOLCHAIN=1.97.0`
  in the workflows) instead of using a repo-wide `rust-toolchain.toml`. A
  repo-wide file would silently switch ci.yml's floating-stable jobs off
  current-stable tracking - losing the early warning when a new Rust release
  breaks something - and force every contributor onto the pinned compiler.
  The pin exists for release artifacts, so it lives in the release recipe.
- `SOURCE_DATE_EPOCH` is not used anywhere in the release recipe (rustc
  ignores it), and `npm pack` does not honor it either - so the npm tarball
  is not byte-reproducible. Its integrity is anchored by `SHA256SUMS` and
  npm's registry integrity (`dist.integrity`) instead.
- `wnice.exe` reproducibility is measured by ci.yml's `repro` job: two clean
  builds from different directories with the pinned toolchain and identical
  `RUSTFLAGS`, SHA-256 compared, verdict in the GitHub Step Summary -
  deliberately informative (non-blocking). The achieved-identity result is
  added here only after the first CI run has measured it; no result is
  claimed until then.
- `--remap-path-prefix=<workspace>=/src`: rustc embeds the absolute path of
  the build workspace into the binary; remapping it to `/src` erases the
  machine-specific prefix, so builds from different directories produce
  identical bytes.

## Authenticode signing (owner decision, not implemented)

`wnice.exe` is not Authenticode-signed; no certificate exists today and this
is explicitly deferred. Unsigned builds trigger the Microsoft Defender
SmartScreen "unknown publisher" warning for fresh downloads.

What the owner would need:

- Azure Trusted Signing. Microsoft-managed certificates; an individual
  developer tier at ~$9.99/month; Microsoft validates the identity. The
  certificates are time-stamped and short-lived, so key handling
  disappears. Integration is a signing step in publish.yml using the Azure
  CLI or the Trusted Signing action. SmartScreen: reputation builds per
  publisher over time; a fresh certificate starts like any other new one.
- SignPath Foundation. Free for approved open-source projects, but the
  project has to pass a review/approval. Signing happens through SignPath's
  service with a foundation-issued certificate; integration is the SignPath
  action or API run against the Release artifacts. SmartScreen: comparable
  to other OV certificates - reputation builds with downloads.
- A purchased code-signing certificate (OV or EV). OV runs ~$100-400/yr and
  requires a hardware token or HSM, so key storage becomes the owner's
  operational burden. EV historically got faster SmartScreen reputation,
  but Microsoft has moved to reputation-based models where that advantage
  is no longer guaranteed; for OV, SmartScreen behaves like the other
  options.

Common to all three: signing removes the SmartScreen attribution warning,
but reputation still accrues per publisher. Where it would slot in: a
publish.yml step after the build, before the assets are collected,
`SHA256SUMS`'d, attested and published, so the checksums and attestations
cover the signed file. Until a certificate exists, this stays unimplemented.
