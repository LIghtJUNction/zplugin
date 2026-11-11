# Verification record

Prepared 2026-11-12 (Asia/Taipei). This file separates executed checks from the
platform matrix that is completed by GitHub Actions before publication.

## Baseline

- Repository: `LIghtJUNction/zplugin`; initial repository had no files or AGENTS.md.
- Project Z conventions consulted: zcall, zclap and zharness manifests/workflows.
  Adopted edition 2024, explicit features, dual MIT/Apache-2.0 licensing, minimal
  runtime dependencies, exact internal release versions and a step-scoped token.
- Stable toolchain verified from `static.rust-lang.org/dist/channel-rust-stable.toml`:
  manifest date 2026-10-15, Rust 1.99.1 (`58cf8f9c3`, 2026-10-13).
- MSRV: Rust 1.85.0 (`4d91de4e4`, 2025-02-17), the edition 2024 baseline.
- Initialization name check: the crates.io sparse-index entries for `zplugin`
  and `zplugin-macros` both returned HTTP 404. The release workflow repeats an
  authoritative registry/owner/version check immediately before publication.

## Executed Linux checks

The local execution host is x86_64 Linux. Full commands and output can be
reproduced with `python3 scripts/check.py`; a successful run writes
`artifacts/check-<toolchain>.json`. The suite runs:

| Area | Executed contract |
| --- | --- |
| Rust source | fmt, workspace/all-targets/all-features clippy with warnings denied |
| Core and macros | ABI validation, status conversion, pointer/range validation, all scalar types, reentry, panic poisoning, multiple interfaces |
| Macro diagnostics | 18 forbidden-signature/type/cooperative-contract fixtures |
| Lifetime diagnostics | Library-borrow escape, non-Send Plugin, non-Sync Instance |
| Feature isolation | 8 core/alloc/std/macros/hot-reload configurations |
| Real dynamic libraries | Separate shared-interface, cdylib and host builds |
| Malformed libraries | 10 independently compiled raw ABI fixtures |
| Dynamic lifecycle | 8 groups of real image/buffer/generation/rollback/quiescence assertions |
| Release transactions | 13 Python tests, including real disposable Git remotes |
| Package correctness | Both dependent crates packaged and compiled through Cargo's temporary registry; multi-package publication dry-run |

The independently built compatibility matrix has passed locally:

| Host compiler | Plugin compiler | Plugin profile | Result |
| --- | --- | --- | --- |
| 1.85.0 | 1.85.0 | debug | Passed |
| 1.85.0 | 1.99.1 | release | Passed |
| 1.99.1 | 1.85.0 | debug | Passed |
| 1.99.1 | 1.99.1 | release | Passed |

Run `ZPLUGIN_STABLE_TOOLCHAIN=1.99.1 python3 scripts/check.py --cross-version` to
reproduce the exact local combination, or omit the variable to use current stable.
Each host loads each separately built plugin and runs typed/owned/error/panic
tests. The host and plugin have separate Cargo target directories.

Core-only compilation passed for `thumbv7em-none-eabi` and
`wasm32-unknown-unknown`. These verify allocator-free protocol portability, not
native dynamic loading on those targets.

## Platform release gate

The GitHub Actions matrix runs Linux, macOS and Windows on both Rust stable and
1.85.0. Every job builds real local dynamic libraries and runs the same contract
suite. The portability job builds the core-only targets and the independent
compiler matrix. Stable jobs validate both crates' actual archives. The release
workflow waits for this reusable matrix on its exact candidate commit before
creating the tag and publishing.

Completed remote run links and publication identities are appended after the
matrix and registry operations return success. Before those results exist, the
presence of a workflow alone is not recorded as a platform pass.

## Performance evidence and limits

[performance.md](performance.md) records the measured machine, toolchain, build
flags, repeated raw samples, allocation counters, image memory and comparison
semantics. No speedup is inferred from implementation language. The data measure
the included arithmetic/byte-copy examples on one shared execution host.

Thread tests enforce a single-thread object contract, rather than claiming
concurrent same-object use. Physical close follows the cooperative contract and
can return Busy or leave an image pinned; the OS may retain a mapping after a
successful loader-reference release. Async/task/waker and cross-boundary callback
protocols are outside ABI v1. See [abi.md](abi.md) for the complete safety boundary.

## Claim review

The implementation's contribution is the small typed ABI/lifetime layer. Its
terms map to public structs and explicit rules. Empirical statements reference
reproducible test/benchmark outputs. Evaluation includes malformed independent
libraries, multiple compilers and lifecycle failures. Native-code trust,
cooperative quiescence and application-owned state migration remain explicit
design constraints.
