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
  link line at the bottom of the file. For a release after the current
  0.3.0, that line is:

  ```
  [0.4.0]: https://github.com/PHPCraftdream/win-nice/compare/v0.3.0...v0.4.0
  ```

  `scripts/release-check.js` (run in CI) enforces the heading format, date
  sanity, the `package.json` match, and that the compare-base tag exists.

### 2. Merge to master, wait for CI green

### 3. Optional rehearsal

Run the "Release (draft)" workflow via `workflow_dispatch` on master. It
builds the artifacts exactly as a tag push would, but creates no release.
Useful before the first tagged release or after workflow changes.

### 4. Create and push the signed tag

```
git tag -s vX.Y.Z -m "win-nice vX.Y.Z"
git tag -v vX.Y.Z
git push origin vX.Y.Z
```

`-s` needs a GPG key configured for git signing; `git tag -v` is the local
self-check before pushing.

### 5. What runs automatically on a tag push

The tag push triggers both workflows. They are independent runs of the same
commit, each building `wnice.exe` with the identical pinned recipe
(`RELEASE_TOOLCHAIN=1.97.0`, `--remap-path-prefix=<workspace>=/src`,
`--locked`).

- `publish.yml` (first-come): publishes `win-nice@X.Y.Z` to npm via Trusted
  Publishing (OIDC) with `npm publish --provenance`.
- `release.yml`: builds the release assets and creates the draft release:
  - `wnice.exe` from the pinned-recipe release build, gated by the repo's
    release-check script;
  - the npm tarball via `npm pack` (npm pinned at 11.5.1);
  - `rust-deps.cdx.json` via `cargo cyclonedx -f json --spec-version 1.5
    --override-filename 'rust-deps.cdx'` (cargo-cyclonedx 0.5.10) and
    `npm-deps.cdx.json` via
    `npm sbom --sbom-format cyclonedx --sbom-type application`;
  - `SHA256SUMS` (basenames only, `sha256sum`) over those four files;
  - all five files attested with actions/attest-build-provenance v4.2.2
    (GitHub artifact attestations, SLSA provenance);
  - release text extracted from the matching `CHANGELOG.md` section by
    `scripts/release-notes.js`, then:

    ```
    gh release create "$TAG" --draft --verify-tag --title "win-nice $TAG" --notes-file <file> <assets...>
    ```

Publishing the draft is a manual owner action (step 7); npm publishing has
already happened by then.

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
- `SOURCE_DATE_EPOCH` is not used: rustc ignores it, and neither `cargo
  package` nor an npm-pack-based shipping flow is in play for the tarball.
- The npm tarball's byte-reproducibility is not claimed (`npm pack` does not
  honor `SOURCE_DATE_EPOCH`). Only `wnice.exe` bit-reproducibility is
  claimed, and the `repro` job checks it empirically: two clean builds from
  different directories with the pinned toolchain and identical `RUSTFLAGS`,
  SHA-256 compared, verdict in the GitHub Step Summary - deliberately
  informative (non-blocking).
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
  disappears. Integration is a signing step in release.yml using the Azure
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
release.yml step after the build, before `SHA256SUMS` and the attestation,
so the checksums and attestations cover the signed file. Until a
certificate exists, this stays unimplemented.
