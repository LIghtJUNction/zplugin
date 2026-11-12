# zplugin

Define a Rust trait, implement it in a `cdylib`, and call it through a typed host
proxy. zplugin uses a frozen target-platform C ABI, cached function tables, and
explicit library lifetimes. The default runtime is `no_std`, needs no allocator,
and has **zero third-party runtime dependencies**.

The same interface also generates C/C++ headers; Zig imports a module translated
from those headers. Additive
extension records support checked structs/enums, owned resources, callbacks,
and cross-library tasks that the Rust host can `.await`.

Rust 1.85.0 or newer; edition 2024. Development toolchain: 1.99.1, the stable
channel available when this release was prepared. Native loading targets Linux,
macOS, and Windows; see [verification](docs/verification.md) for tested machines
and compiler combinations. This is a library of primitives, with no background
watcher, runtime, transport, or plugin discovery service.

## Install

Shared interface crate:

```toml
[dependencies]
zplugin = { version = "0.1.0", features = ["macros"] }
```

Plugin and host crates can enable `std` independently. The interface itself may
remain `no_std`. The optional procedural macro depends on `syn`, `quote`, and
`proc-macro2` at **compile time**; these are not linked into the runtime.

## Define an interface

```rust
#[zplugin::interface(id = "example.calculator", version = 1)]
pub trait Calculator {
    fn add(&mut self, a: u64, b: u64) -> Result<u64, zplugin::Error>;
    fn copy(&self, input: &[u8], output: &mut [u8]) -> Result<u64, zplugin::Error>;
}
```

The macro generates `CalculatorApi`, `CalculatorBinding`, `CalculatorProxy`, and
`CalculatorExport<T>`. Argument names may change; method order, names, receiver
kind, types, and results define the frozen interface. Change its version when
changing that contract.

## Implement a plugin

Use the shared interface crate as a dependency and configure the plugin:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
zplugin = { version = "0.1.0", features = ["std", "macros"] }
calculator-api = { path = "../calculator-api" }
```

```rust,ignore
use calculator_api::{Calculator, CalculatorExport};
use zplugin::{Error, Result};

struct Implementation;
impl Calculator for Implementation {
    fn add(&mut self, a: u64, b: u64) -> Result<u64> {
        Ok(a.wrapping_add(b))
    }
    fn copy(&self, input: &[u8], output: &mut [u8]) -> Result<u64> {
        if output.len() < input.len() { return Err(Error::BufferTooSmall); }
        output[..input.len()].copy_from_slice(input);
        Ok(input.len() as u64)
    }
}
fn create() -> Result<Implementation> { Ok(Implementation) }

zplugin::export!(CalculatorExport<Implementation>,
    constructor = create, version = (1, 0, 0), unload = pinned);
```

The `pinned` policy keeps the image resident until process exit. To opt into
cooperative unloading, implement the unsafe `zplugin::export::UnloadSafe` contract
for the implementation, use `unload = cooperative`, and select the same policy
when loading. The complete runnable example contains that implementation.

## Load and call

```rust,ignore
use calculator_api::CalculatorApi;
use zplugin::{Plugin, UnloadPolicy};

// SAFETY: this file and its dependencies are trusted, and obey the v1 ABI.
let library = unsafe { Plugin::load_path("./libcalculator.so", UnloadPolicy::Pinned)? };
let mut calculator = CalculatorApi::create(&library)?;
assert_eq!(calculator.add(20, 22)?, 42);
let mut output = [0; 64];
let count = calculator.copy(b"hello", &mut output)?;
assert_eq!(&output[..count as usize], b"hello");
# Ok::<(), zplugin::Error>(())
```

Use `.dylib` on macOS and `.dll` on Windows. `load_path` preserves native Unicode
paths. Without `std`, `Plugin::load(&CStr)` is available; on Windows its narrow
path uses the ANSI code page, and `Plugin::load_wide` accepts UTF-16 without alloc.

The proxy borrows its library, so Rust rejects dropping that library first. For an
independent owning proxy, enable `alloc`, put the plugin in `Rc<Plugin>`, and call
`CalculatorApi::wrap(Plugin::create_owned(&library, CalculatorApi::SPEC)?)`.
Cloning occurs when acquiring resources; ordinary calls do not clone a lease.

## Run the complete example

The repository includes separate [interface](examples/interface),
[plugin](examples/plugin), and [host](examples/host) crates:

```sh
cargo build --release -p demo-plugin
cargo run --release -p demo-host -- target/release/libdemo_plugin.so --self-test
```

On macOS substitute `libdemo_plugin.dylib`; on Windows use `demo_plugin.dll`.
See [examples/README.md](examples/README.md) for the directory structure, full
Cargo configuration, explicit snapshot/restore, and owned outputs.

## Supported boundaries

| Rust interface type | Boundary representation |
| --- | --- |
| `u8/u16/u32/u64`, `i8/i16/i32/i64`, `f32/f64` | Fixed-width C ABI scalar |
| `&[u8]`, `&mut [u8]` | Pointer and native `size_t` length; one call only |
| `&str` | Validated UTF-8 view; one call only |
| `Result<T, zplugin::Error>` / `zplugin::Result<T>` | Signed status plus initialized success output |
| `()` or scalar success | No output or fixed-width result record |
| `zplugin::OwnedBytes` success | Buffer, origin release callback, independent library lease |
| Plugin object | Opaque plugin allocation and matching destroy function |
| `#[zplugin::abi_type]` struct or enum | Recursive C-layout record or checked tag/payload; independent of Rust layout |
| `zplugin::OwnedResource` success | Versioned opaque resource with call/release table and independent library lease |
| `zplugin::CallbackRef<'_>` input | Borrowed callback with explicit retain/invoke/release protocol |
| `zplugin::OwnedTask<T>` success | Task handle with poll/cancel/release; host receives `Task<T>: Future` with `alloc` |

Unsupported signatures produce compile errors: generics, direct `async fn`, default methods,
associated types, conditional methods, `usize/isize`, `bool/char`, 128-bit
integers, unannotated structs, native `String/Vec/Box/Arc`, and native Future layouts.
Stable values require `Copy + Clone`; reference, owned, generic and array fields
are excluded. Task outputs use `AbiValue`, including nested structs/enums and unit.
Plugin internals may use ordinary Rust facilities and dependencies.

## Asynchronous and cross-language plugins

The interface declares a task factory, so borrowed input ends at the factory call:

```rust
#[zplugin::interface(id = "example.jobs", version = 1)]
pub trait Jobs {
    fn start(&mut self, input: u64) -> zplugin::Result<zplugin::OwnedTask<u64>>;
}

# #[cfg(feature = "alloc")]
struct Worker;
# #[cfg(feature = "alloc")]
impl Jobs for Worker {
    fn start(&mut self, input: u64) -> zplugin::Result<zplugin::OwnedTask<u64>> {
        Ok(zplugin::OwnedTask::new(async move { Ok(input.wrapping_add(1)) }))
    }
}
```

The generated host proxy returns a lease-owning `Task<u64>`: use
`jobs.start(41)?.await?`, or `cancel()` for cooperative cancellation. Polling,
cancelling and destruction stay on the creating thread; retained wakers support
concurrent notification. No native `Future` or `Waker` layout crosses the boundary.
The plugin manages any runtime it needs. Dropping a future does not implicitly
shut down an external executor; cooperative unloading requires workers and
callback return paths to be fully joined/drained.

`JobsApi::write_c_header(&mut writer)` emits the exact argument layouts, method
signatures and descriptor helpers. C11 and C++17 include the header; Zig 0.17.1
uses its automatically translated module. The generator itself writes through
`core::fmt::Write` without needing an allocator.

```sh
python3 scripts/async.py
python3 scripts/foreign.py --install-zig
```

See the [async three-crate example](examples/async), the complete
[C/C++/Zig example](examples/foreign/README.md), and the
[extension protocol and ownership rules](docs/extensions.md).

## Features

| Feature | Additional capability | Runtime dependency |
| --- | --- | --- |
| default: none | ABI, native loader, borrowed instances, manual resource/factory contracts | `core` and platform loader |
| `alloc` | Factories, `Rc` leases, owned bytes/resources, closure callbacks, Future/Waker adapters | Rust allocator |
| `std` | Native paths, error integration, panic containment | `std`, implies `alloc` |
| `macros` | Trait-to-ABI generation | Compilation-only macro crate |
| `hot-reload` | Bounded generations and request leases | `alloc` |

For allocator-free plugins, implement the public ABI with application-owned
object storage. `export::Object<T>` and the entrypoint guards also work without
an allocator. Convenience `export!` factories deliberately require `alloc`.

## Multiple interfaces and hot switching

`export_interfaces!` exports independently versioned interfaces through one entry:

```rust,ignore
zplugin::export_interfaces!(version = (1, 0, 0), unload = cooperative;
    CalculatorExport<Implementation> => create,
    SnapshotExport<State> => create_state,
);
```

With `hot-reload`, `hot::Generations::new(plugin, maximum_retired)` tracks the
current image and old generations. `current()` acquires a request/session lease.
`reload` or `reload_path` loads an independent image, runs your creation/restore
closure, and switches only on success. Old objects continue using their old code.
Use an explicitly versioned byte snapshot for state migration. `collect()` asks
unreferenced cooperative generations to retire; `Busy` generations remain and
count toward the limit. No timeout forces an unload.

## Safety and performance

Loading trusted native code remains `unsafe`: initializers and methods execute
with the host's privileges. ABI checks detect incompatible metadata; native code
must still honor memory validity, ownership, and unwind contracts. Proxies are
neither `Send` nor `Sync`; all calls, including shared trait methods, require a
mutable proxy. Same-object raw reentry returns `Busy` before a second borrow.

Under `std` and an unwind-enabled plugin build, adapters catch Rust panics on the
plugin side and poison that instance. `panic=abort`, foreign exceptions, and
native crashes retain their process-level behavior. C++ plugins contain their
own exceptions before returning; the C++ example includes that boundary.
Task/callback extensions use independent size/version-prefixed tables. Failed
resource cleanup prevents cooperative unloading, including a future or callback
capture destructor that panics.

Steady borrowed calls use cached function pointers without forced allocation,
serialization, symbol lookup, or per-call reference counting. Actual comparisons
against inline/noinline calls, cached C pointers and cached `libloading` symbols
are in [performance.md](docs/performance.md), including raw samples and noise.
An inline static call remains a distinct, cheaper baseline for tiny functions.

## Validation and release

```sh
python3 scripts/check.py
python3 scripts/check.py --cross-version
python3 scripts/legacy.py
python3 scripts/foreign.py --install-zig
python3 tools/bench/run.py
```

CI runs actual hosts and dynamic libraries on Linux, macOS, and Windows, plus
Rust 1.85.0/stable, core-only embedded/WASM compilation, negative macro fixtures,
and release transaction tests. The manual release workflow supports
`patch/minor/major/custom`, `dry_run`, exact-commit validation, dependency-ordered
publication, and recovery after a partial upload. It uses the step-scoped
`CARGO_REGISTRY_TOKEN` secret. See [release operations](docs/releases.md).

Further detail: [ABI and ownership](docs/abi.md), [validation evidence](docs/verification.md),
[changelog](CHANGELOG.md). Licensed MIT OR Apache-2.0.

