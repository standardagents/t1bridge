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

Use the maintainer-only `t1-release` Rust tool. It is a workspace member so
`make quality` checks it, but runtime packages do not install it. It requires
`git`, `gh`, `curl`, Wrangler, GnuPG, `bsdtar`, `repo-add`, and `vercmp`.
Authenticate GitHub and Wrangler beforehand. The tool obtains a short-lived
Wrangler token in memory; it never stores credentials in staging or argv.
Use the existing signing wrapper rather than exporting a private key.

1. Download the signed CI draft into a candidate directory with
   `gh release download TAG --repo standardagents/t1bridge --dir CANDIDATE`.
   Add any intended optional package update, its detached signature and matching
   public source archive/recipe to that directory. Packages must already have
   passed their build/install checks. The publisher never rebuilds packages.
2. Create a private configuration file outside the checkout:

   ```json
   {
     "account": "CLOUDFLARE_ACCOUNT_ID",
     "bucket": "standardagents-linux-packages",
     "prefix": "arch/standardagents/x86_64/",
     "public_url": "https://linux.standardagents.ai/arch/standardagents/x86_64/",
     "repository": "standardagents/t1bridge",
     "tag": "vVERSION",
     "checkout": "/absolute/path/to/checkout",
     "keyring": "/secure/signing/public-keyring.kbx",
     "primary_fingerprint": "35B166F78B063B04DE1E3D913E6C4216EB03D371",
     "signing_fingerprint": "D3FC53C0DF306FD52332B34426F1223CF81289C4",
     "signer": "/secure/signing/gpg-release",
     "notes_file": "/private/release/notes.md"
   }
   ```

   Replace the placeholders with the release's actual values. The signer accepts
   GPG command-line arguments and must handle any key unlock through the existing
   signing boundary. No credentials belong in this configuration. Put an updated
   `PRERELEASE.md` in the candidate directory when updating the public setup guide.
3. Run `cargo run --locked -p t1-release -- prepare CONFIG CANDIDATE NEW_STAGE`.
   Review the resulting `state.json`, package inventory, `assets/` and `notes.md`.
   Preparation downloads and verifies the current signed manifest, both indexes
   and every package against the pinned key. It merges by package name, retains
   unrelated entries, rejects downgrades and changed bytes at the same version,
   and builds/signs the complete indexes and checksum manifests. Old versioned
   source archives remain in the complete manifest; replaced package files remain
   untouched at the origin. Preparation makes no remote writes.
4. Run `cargo run --locked -p t1-release -- publish STAGE`. The tool verifies
   staging, acquires the repository publication lock, compares the origin with
   the retained baseline, uploads immutable artifacts and verifies their public
   bytes before updating indexes. It checks the complete anonymous repository,
   synchronizes and verifies the GitHub draft against the same manifest, then
   publishes the prerelease and releases the lock.

### Conflicts and interrupted publication

The atomic Git ref `refs/heads/t1bridge-publication-lock` serializes publishers.
Its commit identifies the staged plan and operation, with no credentials or
machine data. An exclusive local file lock prevents concurrent use of the same
staging directory. Every publisher must use this tool; direct R2 writes do not
participate in the cooperative lock. Origin baseline checks detect changes
before the index transition, but do not make unrelated out-of-band writes safe.
Do not copy an active staging directory between machines.

If an upload, public download or GitHub operation fails, keep the staging
contents and remote lock. Run the same `publish STAGE` command again. It reuses
exact artifact bytes, verifies already-uploaded immutable objects and accepts
only the recorded baseline or this operation's intended mutable bytes during
resume. It never probes absent immutable CDN URLs before upload. Cached 404s or
stale bytes stop completion; wait for availability and resume without rebuilding,
renaming artifacts, or adding cache-purge permissions.

A competing lock or unexpected origin index stops publication. Investigate the
other operation before proceeding; do not force-delete its lock or edit an
active stage to bypass validation. For an abandoned operation, first establish
that no process can still write, inspect its durable state and the actual origin,
then explicitly recover its lock with a compare-and-delete Git lease. If index
publication began, prefer completing that original stage before preparing a new
release. The tool never automatically discards a lock or staging artifacts.

R2 cannot atomically replace all database/signature/manifest objects together.
There can be a brief interval when clients reject mismatched signatures; immutable
packages are verified first, and completion waits for a fully consistent public
view. A failure during that interval requires resuming the retained stage.

`make quality` covers package addition/upgrade with unrelated entries, immutable
collisions, changed origin indexes, cached 404s, interrupted uploads/index writes,
resume without reupload, and real Git lock conflicts and conditional deletion.
The publication itself verifies exact package metadata, inventories, checksums
and signatures across staging, R2 and GitHub.

## Key custody and rotation

Keep the encrypted primary key, recovery copy and revocation certificate
outside CI. To rotate signing authority, add a replacement subkey and distribute
the updated public key before switching CI. Revoke the old subkey after clients
can verify its replacement. If a key is compromised, stop signing and require
an explicit trusted-key refresh before resuming releases.
