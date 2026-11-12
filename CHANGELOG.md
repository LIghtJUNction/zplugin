# Changelog

## [Unreleased]

- Add cross-library owned tasks with poll/cancel/release and a thread-safe retained
  waker protocol; host tasks implement Future without exposing Rust Future layout.
- Add checked, recursively versioned ABI structs and fieldless/payload enums.
- Generate exact C/C++ headers from Rust interfaces and translate them for Zig.
- Add opaque owned resources and retained local-thread reverse callbacks, with
  origin-matched release functions, independent leases, and panic poisoning.
- Retain libraries after failed future/callback/resource cleanup; require joined
  worker and callback return paths for cooperative unloading.
- Add real Rust asynchronous and C11/C++17/Zig plugins, cross-version lifecycle
  tests, and bidirectional tests against the published 0.1.0 ABI.

## [0.1.0] - 2026-11-12

- Introduce no_std ABI v1 and zero-dependency Linux/macOS/Windows native loaders.
- Generate checked function tables and typed proxies from restricted Rust traits.
- Support borrowed bytes, writable outputs, UTF-8, scalar/status results, plugin
  objects, and owned buffers with their originating release function.
- Add single and multiple-interface exports, explicit thread/ownership contracts,
  unwind containment, poisoned instances, and guarded reentry.
- Add bounded generation switching, explicit state migration, rollback, library
  leases, and cooperative quiescence without forced unloading.
- Include runnable independent host/plugin examples, malformed-library and
  cross-compiler tests, allocation-counted benchmarks, and resumable releases.
