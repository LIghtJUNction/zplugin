# Release operations

The Release workflow tests one reproducible candidate commit before it changes
remote refs or uploads a crate. It then pushes the version commit and its
annotated tag atomically, publishes `zplugin-macros` before `zplugin`, verifies
each registry checksum, and creates the GitHub Release in the same workflow.
A tag is never moved and an existing registry version is never overwritten.

## Start a release

Open **Actions → Release → Run workflow**, select `main`, and choose:

| Input | Meaning |
| --- | --- |
| `bump=patch` | Increment the patch component and clear prerelease/build metadata. |
| `bump=minor` | Increment the minor component and reset patch to zero. |
| `bump=major` | Increment the major component and reset minor and patch to zero. |
| `bump=custom` | Use the exact `version`, or resume its existing release tag. |
| `version` | Accepted only with `custom`; strict SemVer without a `v` prefix. |
| `dry_run=true` | Run candidate and package validation without pushing or publishing. |
| `dry_run=false` | Complete publication after all validation jobs succeed. |

For the first release, use `custom`, version `0.1.0`. A custom version must
advance SemVer precedence unless it is the unpublished first version or names
an existing tag from this release workflow. Numeric prerelease identifiers must
not have leading zeroes. Build metadata alone is not a version increment.

The repository must have an Actions secret named `CARGO_REGISTRY_TOKEN`, with
permission to publish both crates. The token is injected only into the registry
upload step. It is never copied into the package, a configuration file, a cache,
or a diagnostic artifact. Other jobs run without the registry token, and only
the final publication job receives `contents: write`.

## Explicit committed requests

A maintainer can also commit `.github/release-request.json` to `main`:

```json
{
  "bump": "custom",
  "version": "0.1.0",
  "dry_run": false
}
```

Only a push that changes this exact file starts the release entry point. The
file is parsed as typed JSON data, requires exactly these three fields, and
rejects duplicate keys. This is useful when the connected GitHub client can
commit files but cannot dispatch Actions. An ordinary source push does not
publish a crate. No step depends on a push or tag created by `GITHUB_TOKEN`
triggering a later workflow.

## What validation proves

Every release matrix job reconstructs the same candidate commit from the
selected source, version and source timestamp. A mismatch rejects the release.
The matrix runs the real host/plugin tests on Linux, macOS and Windows with
stable Rust and the declared MSRV. The portability job checks allocator-free
protocol targets and independently compiled host/plugin toolchains. The stable
jobs use Cargo's multi-package package verification and publish dry-run so the
normalized archives can resolve the as-yet-unpublished macro crate.

The package audit checks crate identity, version, clean Git provenance and the
absence of path/Git dependencies in the normalized manifest. It records a
SHA-256 hash for each final archive. Before the atomic ref push, the workflow
checks that `main` has not advanced and that the crate names still belong to
this project. Package verification is repeated on the exact final candidate in
the publication job. Registry publication succeeds only when the uploaded bytes
match the recorded hashes and the dependency version is visible in the sparse
index.

A green workflow is evidence for its recorded runners and toolchains. It does
not establish support for untested targets, runtime environments, or plugin
implementations that violate the unsafe loading contract. See the actual run
linked from each GitHub Release and `docs/performance.md` for measurement scope.

## Resume a partial release

Crates.io publication is not atomic across multiple crates. For example, the
macro crate may upload successfully before the runtime upload fails. The job
preserves `target/release-state.json`, `target/release-packages.json` and the
archives as an Actions artifact. The state names the last completed stage and
contains hashes, never tokens.

1. Read the failed step and retained release state. Fix missing credentials or
   temporary registry access without moving the existing tag.
2. Rerun the failed publication job, or dispatch **Release** with `bump=custom`,
   the same exact version and `dry_run=false`.
3. The workflow verifies the annotated tag's provenance and re-tests that exact
   commit. Identical already-published archives are accepted; only missing
   packages are uploaded. The existing GitHub Release is reused if complete.

If `main` has advanced, an existing release can still be resumed from its tagged
commit. If package hashes disagree, ownership differs, the tag is foreign, or a
version is yanked, the workflow stops. Investigate that mismatch; do not force a
tag update or attempt to replace registry bytes. A code fix after an immutable
release needs a new version.

## Local checks

Run the regular contract suite and release failure-mode tests:

```sh
python3 scripts/check.py
python3 -m unittest discover -s scripts -p test_release.py -v
```

From a clean committed checkout on stable Rust, validate the two distributable
archives with:

```sh
python3 scripts/release.py packages
```

This command performs Cargo package verification and publication dry-run. It
does not upload either crate. Multi-package packaging requires Cargo 1.90 or
later; the library MSRV remains independent of the release tooling version.

The release script tests create disposable local Git repositories and cover
reproducible candidates, preservation of external dependency pins, dirty source,
concurrent branch updates, foreign tags, registry ownership, immutable checksum
matching, malformed inputs and missing publication credentials.

## Tooling references

Cargo's [multi-package publishing release note](https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/)
and [`cargo package` reference](https://doc.rust-lang.org/cargo/commands/cargo-package.html)
describe the package ordering and normalization used by the release tooling.
The workflow and script tests are the evidence for zplugin's additional commit,
owner, hash and recovery checks.
