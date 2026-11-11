# Changelog

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
