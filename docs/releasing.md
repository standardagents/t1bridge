# Releasing packages

Documentation changes do not require new packages. Build each package once;
the install checks and signing job consume those same artifacts.

## GitHub signing

The `release` environment permits workflow runs from `main`, without a manual
reviewer approval step. Administrator bypass is disabled.

Only the replaceable signing subkey and its passphrase are environment secrets:
`T1BRIDGE_SIGNING_SUBKEY` and `T1BRIDGE_SIGNING_SUBKEY_PASSPHRASE`.
The primary private key stays outside GitHub. Public fingerprints are stored
as `T1BRIDGE_SIGNING_PRIMARY_FINGERPRINT` and
`T1BRIDGE_SIGNING_SUBKEY_FINGERPRINT` environment variables.

1. Update package versions/releases as needed and re-pin the source archive
   in both core and DKMS recipes before committing. The workflow checks those
   pins against the exact release tree.
2. Create and push an annotated, signed `v<version>` tag matching the core
   package version. Use the approved release key; do not move an existing tag.
3. Run **Release candidate** from the `main` branch, set `ref` to that tag,
   and enable `ci_sign`. The unsigned-only mode instead takes a full commit
   SHA and does not access signing secrets.
4. After the single build and install checks, signing starts automatically.
   The signing job verifies the
   exact artifact manifest and tag signature, then signs packages and repository
   databases. It rejects an exported primary private key.
5. Review the resulting **draft** GitHub release before publishing. This job
   does not upload to the package host; publishing to `linux.standardagents.ai`
   remains a separate step using the signed artifacts, without rebuilding.

Environment configuration and secret presence are not proof of a successful
CI signing run. Confirm that first run before treating the path as validated.

## Publish the complete package repository

The CI draft contains the four core packages. Its generated repository indexes
are candidate indexes, not a replacement for the complete public repository.
Since [v0.1.8](https://github.com/standardagents/t1bridge/releases/tag/v0.1.8),
the public repository also contains `t1bridge-omarchy`. Publishing the CI
indexes directly would remove that package from discovery.

Until the guarded publisher in [#24](https://github.com/standardagents/t1bridge/issues/24)
is implemented, publication requires one maintainer to coordinate an exclusive
publication window and retain a resumable local staging directory:

1. Obtain the current database, files index, checksum manifest and their
   signatures. Verify them against the pinned release key from the
   [installation instructions](../README.md#1-trust-the-signing-key), and
   retain the original bytes as the publication baseline. Inventory every
   current package name, version and artifact, including optional packages.
2. Copy that complete verified repository into staging and merge only the
   intended package updates using `repo-add --include-sigs`. Retain unrelated
   entries and previously published immutable artifacts. Compare the resulting
   inventory with the baseline; stop on any unexpected removal or downgrade.
3. Sign the complete database and files index through the existing signing
   boundary. Assemble the matching source archives, recipes and patches,
   retaining the optional package's public build inputs. Regenerate and sign
   the complete `SHA256SUMS` manifest and verify every signature locally.
4. Upload new immutable package, signature and versioned source objects before
   changing public indexes. Use origin object metadata for collision checks;
   do not preflight absent download URLs through the CDN. A cached 404 can
   outlive an upload. Never replace different bytes at an existing immutable
   filename.
5. Download the uploaded immutable objects anonymously and compare their bytes,
   checksums and pinned-key signatures with staging. A missing object or cached
   404 stops publication. Retain staging and resume verification after the
   objects become available; do not rebuild or rename artifacts to bypass it.
6. Recheck the current indexes at the origin against the retained baseline
   before replacing mutable indexes and manifests. Stop if another publication
   changed them. This manual check depends on the exclusive publication window;
   it is not an atomic concurrency guard.
7. Publish the combined indexes, matching signatures and manifests. Verify the
   anonymous repository view, every referenced package and the complete
   inventory, including packages outside the core update. Keep the GitHub draft
   open if any check fails.
8. Replace the draft's candidate indexes and manifests with the verified
   combined set and include the retained optional package's matching assets.
   Verify GitHub downloads against the same staging manifest before publishing
   the release page. Record exact versions, checks and remaining validation.

The v0.1.8 publication verified five package entries, 31 public artifacts and
eight signatures. Those counts describe that release, not a fixed inventory
for future releases. Automated concurrency, failure and resume checks remain
open in #24; this procedure does not establish that automation as complete.

## Key custody and rotation

Keep the encrypted primary key, recovery copy and revocation certificate
outside CI. To rotate signing authority, add a replacement subkey and distribute
the updated public key before switching CI. Revoke the old subkey after clients
can verify its replacement. If a key is compromised, stop signing and require
an explicit trusted-key refresh before resuming releases.
