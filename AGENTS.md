# zplugin

Keep the default crate no_std and free of third-party runtime dependencies.
Use explicit `alloc`, `std`, `macros`, and `hot-reload` features.
The optional `wasm` host feature uses Wasmi and needs Rust 1.86; default/native
features and the allocator-free Wasm guest SDK retain Rust 1.85. Keep Wasm imports
and start sections denied, validation/fuel/resource limits mandatory, and host
adapters free of unsafe code. Guest-memory caps are not total process-RSS limits.
Freeze the v1 C ABI; change application interface versions when signatures change.
Centralize unsafe contracts and test real independently built dynamic libraries.
Do not claim performance or platform support without recorded measurements or CI.
Run `python3 scripts/check.py` before a release. Release only verified commits,
using the manual release workflow or its explicit release-request input.
Scope CARGO_REGISTRY_TOKEN to publication steps. Never log or persist its value.

