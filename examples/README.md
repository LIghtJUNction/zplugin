# Run the independent native plugin example

The interface, implementation and host are separate Cargo packages. The host
depends on the interface package, and loads the implementation only at runtime.

```sh
cargo build -p demo-plugin
cargo run -p demo-host -- target/debug/libdemo_plugin.so
```

On macOS pass `target/debug/libdemo_plugin.dylib`. On Windows pass
`target/debug/demo_plugin.dll`. With no argument, `demo-host` chooses the correct
filename for the current platform.

`interface/src/lib.rs` is `no_std`. Its `#[zplugin::interface]` declaration produces
the typed host proxy and plugin adapter. `plugin/Cargo.toml` requests a `cdylib`;
`plugin/src/lib.rs` implements the ordinary Rust trait and exports the versioned
entry. `host/src/main.rs` demonstrates scalar calls, caller-owned output buffers,
UTF-8 input, versioned snapshots, and a plugin-owned buffer that outlives its
creating object.

The plugin promises cooperative unloading through an explicit unsafe
`UnloadSafe` implementation. It has no background activity or external
registrations. A plugin with threads, tasks or callbacks must retire all such
activity before making this promise. The host's unsafe load is the trust boundary.

## Build variants for generation tests

Use separate target directories so the artifacts have independent paths:

```sh
cargo build -p demo-plugin --target-dir target/first
cargo build -p demo-plugin --features v2 --target-dir target/second
cargo build -p demo-plugin --features fail-init --target-dir target/rejected
```

The first version uppercases ASCII bytes; `v2` lowercases them. Both preserve the
same v1 application interface and snapshot schema. `fail-init` returns plugin
status `1001` during instance construction, which exercises failed-switch
rollback. All three are test variants, independent of the published crate version.

## Reproduce performance measurements

```sh
cargo build --release -p demo-plugin --features instrument
cargo run --release -p zplugin-bench -- target/release/libdemo_plugin.so
```

Use the platform-specific library extension above. `instrument` adds a counting
allocator inside the plugin; the benchmark also counts host allocations. The
normal example does not enable this instrumentation. Raw CSV includes sample
quantiles, allocation counts, Rust version, build flags and Linux RSS measurements.
See `docs/performance.md` for the measured results and interpretation.
