# ABI v1 and ownership

## Contract and evidence

zplugin v1 connects independently compiled trusted modules on the **same target
platform and C ABI**. The stable boundary consists of `repr(C)` records, fixed
integer/float fields, pointers, native `size_t` lengths, and `extern "C"` functions.
Rust trait objects and Rust container layouts never cross it. Rust's reference
documents both the [C representation](https://doc.rust-lang.org/reference/type-layout.html)
and [native library output types](https://doc.rust-lang.org/reference/linkage.html).

The executable contract is `src/abi.rs`, the generator, and the real-library
tests. [verification.md](verification.md) records tested compiler/platform
combinations. ABI compatibility also requires the same architecture, endianness,
pointer width, target calling convention, system library requirements and CPU
instruction support. A Linux build is not a Windows or macOS artifact.

## Discovery and versioning

The fixed symbol is `zplugin_entry_v1`; its type is
`unsafe extern "C" fn() -> *const Header`. The returned pointer must be nonnull,
aligned and readable for the complete 16-byte v1 header. This minimal readability
is part of loading trusted native code. Arbitrary address validity cannot be
established by a metadata check.

| Header field | Type | Rule |
| --- | --- | --- |
| `magic` | `u64` | `0x315f4e4947554c50` |
| `major` | `u16` | Exactly 1 |
| `minor` | `u16` | Additive descriptor evolution only; current value 0 |
| `size` | `u32` | Bytes readable from the descriptor pointer |

The host checks this header before reading the plugin version, flags, interface
array or quiescence callback. Descriptors shorter than the full v1 prefix are
rejected. Future descriptor suffixes may be ignored; v1 prefix fields retain
their offsets and meaning. Unknown flag bits are ignored; cooperative unloading
requires its known flag and a present callback.

An interface array contains pointers to immutable `InterfaceDescriptor` records.
Each starts with a `u32` size. The loader reads that prefix before accessing its
ID, application version, method array, create or destroy functions. Array memory
must contain the declared number of entries. The loader limits interface count
to 4096. All descriptor memory and code addresses live at least as long as the
library handle.

An interface ID is an explicit ASCII name, never Rust `TypeId`. Macro IDs contain
1–255 ASCII letters/digits or `. _ : / -`. Versions are nonzero `u32` values.
IDs in a multi-interface export must be distinct. The host queries exact ID and
version, exact method count, present function slots, and exact canonical signature
bytes. It compares the actual canonical text; no collision-prone hash is trusted.

For example, `v1:add:mut(u64,u64)->result<u64>` fixes method name, receiver,
argument order/types, and success type. Parameter names are immaterial. Every
interface version freezes its whole table. Add a new version or an independent
extension interface for changed capabilities; do not insert optional methods into
an existing frozen table. Missing entry, bad ABI, short descriptor, wrong interface
version, absent method, and mismatched signature have separate error results.

Four version spaces remain independent:

| Version | Meaning |
| --- | --- |
| zplugin crate SemVer | Rust API and release package |
| ABI major/minor | Discovery and wire record contract |
| Application interface version | Frozen business method table |
| `PluginDescriptor.plugin_version` | Plugin business implementation |

Snapshot schema versions are an additional application-owned data-format version.

## Calls, buffers and status

Each method slot has the fixed dispatcher type
`unsafe extern "C" fn(object, args, out) -> i32`. Its canonical signature defines
the generated `repr(C)` argument and result records. Fixed scalars retain their
target C representation; slice and string fields become explicit pointer/length
records. The adapters validate nullness, alignment, lengths, UTF-8 and conflicting
mutable ranges before constructing Rust references.

Every nonempty range must refer to one valid allocation, fit in `isize::MAX`, and
have a nonoverflowing end address. Empty ranges may have null pointers; adapters
substitute aligned dangling pointers when constructing empty Rust slices.
Input bytes are initialized, readable and immutable during the call. Output slices
are initialized caller memory exclusively writable during the call. No pointer
may be retained afterward. Mutable ranges may not overlap inputs, other outputs,
argument-record storage or scalar-result storage. Object storage and raw record
validity remain explicit unsafe caller obligations.

Scalar success outputs are written only after a successful return. The host reads
them only after status zero. On an error the scalar output is unspecified and is
not read; generated adapters leave it untouched. Unit output may be null. A
mutable output slice may have partial writes before a business error or panic;
zplugin does not promise transactional output rollback. The example checks short
output before copying and tests that example's unchanged-buffer behavior.

| Status | Meaning |
| --- | --- |
| 0 | Success; success output initialized |
| -1 | Invalid argument |
| -2 | Invalid UTF-8 |
| -3 | Output too small |
| -4 | Busy / rejected reentry |
| -5 | Caught Rust panic |
| -6 | Previously poisoned instance |
| -7 | Unsupported operation |
| -8 | Task cancelled |
| -9 | Task already completed |
| Positive values | Application-defined errors |

Other nonzero integers are preserved as `Error::PluginStatus(i32)`, so invalid
Rust enum discriminants are never read from foreign code. Returning
`PluginStatus(0)` as an error maps to invalid argument.

## Objects and returned ownership

An object is created in its plugin and released exactly once by the matching
destroy function. A failed factory leaves its output pointer null. The generated
factory allocates `export::Object<T>` in the plugin allocator; manual descriptors
may use caller-managed storage. A successful factory returning null is rejected.
The host never uses its own allocator to free plugin object memory.

`Binding` borrows the library. An `Instance` either borrows it or owns an `Rc`
lease. Its destructor calls the plugin's destroy function before releasing that
lease. `Buffer` has a separate lease and may outlive its originating instance.
Its release callback runs before releasing its own lease. An `OwnedBytes` result
contains a data pointer, length, opaque context and the originating release
function; the raw record itself carries no Rust `Vec` or `Box` layout.

Custom `OwnedBytes::from_raw` implementations must keep both data and release
context valid independently of the originating instance. They must not point into
instance fields or call an already-destroyed object. `OwnedBytes::from_vec`
provides this property by owning a plugin-allocated boxed byte slice. Its release
returns to the same allocator and updates plugin resource accounting.

If destroy or release reports failure, the host pins the library instead of
unloading code still needed by leaked resources. A panicking factory likewise pins
the candidate. This is a conservative failure path, not a successful cleanup.
Low-level `method()` returns an unsafe function pointer; extraction does not extend
the library or object lifetime. Calling such a pointer requires retaining all
owners and honoring its exact records. Generated proxies handle this automatically.

## Threads, reentry and panic

Library handles, proxies, owning leases and buffers are neither `Send` nor `Sync`.
Create, use and destroy them on the same thread. Proxy methods always require
`&mut self`, including source methods with `&self`. Independent instances can be
used sequentially on that thread. A plugin may run its own synchronized workers;
it owns their lifetime and must satisfy the selected unload policy.

The raw adapter's single-thread busy guard is checked before borrowing its `T`.
Same-object reentry returns `Busy`; the guard is not a cross-thread lock, and
calling raw pointers concurrently violates the ABI contract. Compile-fail tests
cover `Send`, `Sync` and library-borrow escape.

With the plugin's `std` feature and unwind-enabled compilation, factory, method,
destroy and buffer-release adapters contain panics on their side of `extern "C"`.
A panicking method poisons its instance; subsequent calls return `Poisoned`.
Normal panic payloads are dropped inside a second guard. If a payload destructor
also panics, only that secondary payload is deliberately leaked to prevent an
unwind across the boundary. The ordinary Rust panic hook may still print.

Without `std`, no catch is available; with `panic=abort`, a panic terminates the
process. Foreign exceptions, segmentation faults, memory corruption and explicit
process termination are outside panic containment. These behaviors follow Rust's
[FFI unwind rules](https://doc.rust-lang.org/nomicon/ffi.html#ffi-and-unwinding).
The [extension protocol](extensions.md) adds independently versioned task,
waker, callback and resource tables. Their addition leaves the discovery header,
interface descriptor, method slot, and existing v1 signatures unchanged.

## Cooperative close and bounded generations

Pinned loading is the default. Dropping a pinned handle retains the native mapping
until process exit. It can support libraries whose background activity cannot be
proven quiescent. Repeated uncoordinated pinned loads intentionally retain their
loader references; callers needing a bound use `hot::Generations`.

Cooperative loading requires an explicit unsafe `UnloadSafe` promise for all
exported implementations and a quiescence callback. No thread, task, callback,
TLS/atexit destructor, buffer or escaped address may require the image after
quiescence succeeds. Destructors must join workers and cancel/unregister external
activity. Initializers and entry must leave no activity requiring a resident image
if metadata validation fails, because that cooperative failure path closes its
new handle. Failed factories must roll back their external activity.

`Plugin::try_close` consumes exclusive ownership. Borrowed objects prevent that
ownership statically, and owned objects prevent `Rc::try_unwrap` dynamically. The
plugin's generated quiescence function also requires its object/buffer count to
reach zero. `Busy` returns the still-owned `Plugin` for retry. Drop retains the
handle when cooperative close fails. There is no timeout-based forced close.

A generation manager reserves a retired slot before loading a candidate. It
rejects a path that resolves to an already managed OS module handle. It prepares
the new instance and restores an explicit versioned snapshot before publishing
the new current generation. Existing objects keep their prior generation.
Preparation errors or panics retain the candidate in the retired set until
`collect()` can reclaim it; escaped candidate leases are therefore counted.
Failed pinned load attempts conservatively consume a slot even if loading failed
before mapping. This may stop switching early, but prevents untracked failed
attempts from bypassing the configured limit.

The limit counts manager-owned retired generations and conservatively retained
failed attempts, in addition to one current image. It does not account for other
loaders or global OS behavior. Successful `dlclose`/`FreeLibrary` releases this
manager's reference; external loader references or OS pinning can keep a mapping
resident. A deployment requiring actual memory reclamation must exclude such
external references/pinning and verify the target system's behavior. Windows
[`LoadLibraryExW`](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-loadlibraryexw)
and [`FreeLibrary`](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-freelibrary)
define that platform's loader behavior.

Snapshots and side effects belong to the application. zplugin switches the current
library only after a successful preparation closure; it does not roll back writes
to databases, files or application state performed by that closure. Quiesce the
relevant business state before snapshotting if a consistent migration is required.

## Review and claim-evidence map

| Claim | Evidence | Limit |
| --- | --- | --- |
| Exact interface checks precede binding | Malformed real cdylibs and contract tests | Trusted readable metadata required |
| Resources release through their origin | Owned-buffer and destructor tests | Custom raw resources must honor the contract |
| Calls avoid forced allocation/refcounts | Generated code plus host/plugin allocation counters | Business methods may allocate |
| Versions coexist and failure preserves current | Real v1/v2 and failed-init tests | Application side effects are separate |
| Cross-compiler compatibility | Independently built 1.85.0/1.99.1 combinations | Same target/system ABI required |

Review dimensions: contribution is small typed native boundaries; terminology maps
directly to records and owners; performance claims use measurements; evaluation
includes failure cases and independently compiled libraries; the design keeps
native-code trust and cooperative unload obligations explicit.
