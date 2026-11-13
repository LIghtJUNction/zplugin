# Stable values, tasks, resources and callbacks

## Scope

The extension protocols add richer data and execution lifetimes without changing
`zplugin_entry_v1`, `Header`, `PluginDescriptor`, `InterfaceDescriptor`, `Method`,
or any previously generated synchronous signature. A new business method still
requires a new application-interface version or a separate interface. The crate
version, discovery ABI, extension-table version, and application version are
independent.

The default crate remains allocator-free `no_std`. Raw tables and manual owners
work with `core`; `alloc` enables convenient Rust futures, closure callbacks,
allocated resources, and owning library leases. `std` additionally contains
unwind-enabled Rust panics. There is no dependency on an executor or FFI runtime.

| Boundary | Producer representation | Host representation | Retirement |
| --- | --- | --- | --- |
| Stable value | `AbiValue::Wire` | Checked Rust value | Copy, no resource ownership |
| Task | `RawTask` | `Task<'lib, T>` | Original cancel/release and independent lease |
| Resource | `RawResource` | `Resource<'lib>` | Original release and independent lease |
| Callback input | Borrowed `RawCallback` | `CallbackRef<'call>` | Retain before storage; release each retained owner |
| Poll notification | Borrowed `RawWaker` | Runtime-local Waker adapter | Thread-safe retain/wake/release |

## Stable values and generated declarations

`#[zplugin::abi_type(id = "app.point", version = 1)]` on a `Copy + Clone` struct
generates a separate `repr(C)` wire record. Fields use fixed-width numeric types
or other annotated values. Rust field layout is irrelevant: the adapter converts
field by field. The complete recursive schema includes ID, version, field names,
order, nested schemas, and enum tags. Binding compares full schema text.

Fieldless enums use a plain `u32` wire tag. Payload enums use a `repr(C)` record
containing a `u32` tag and a C union of `repr(C)` variant payloads. The adapter
validates the tag before reading the selected payload or creating a Rust enum.
The producer initializes every active field; inactive union storage and padding
need not be initialized and must never be treated as serialized bytes.
The schema distinguishes scalar-tag and tagged-union representations.

The first extension release accepts nonempty named structs, nested values, and
unit/tuple/named enum variants with literal `u32` tags or sequential tags. It
rejects generic declarations, reference/owned/container/array fields, `bool`,
`char`, pointer-width integers, and 128-bit integers. An unsafe manual `AbiValue`
implementation must provide a C representation with valid readable fields,
checked conversion and a complete schema. Private generated wire constructors
prevent safe Rust callers from forging a selected but uninitialized payload.

`InterfaceApi::write_c_header` writes declarations, signatures and descriptor
initializers to `core::fmt::Write`. C11 and C++17 consume the header directly.
The Zig example runs `zig translate-c` for the target and imports its
generated module. All participants must use the same platform C ABI, pointer
width, architecture, endianness and compatible system-library requirements.
This does not produce a platform-independent plugin binary.
The validation toolchain pins Zig 0.15.2 on macOS and 0.17.1 on Linux/Windows;
the example documents the macOS translator timeout behind that compiler choice.

The [foreign-language example](../examples/foreign/README.md) includes every build
configuration and the Rust host. It exercises nested payload enums, borrowed
buffers, owning buffers/resources, callbacks and task polling in C, C++ and Zig.
The unit/no-argument C typedefs are placeholders: the protocol writes no unit
output, and callers may pass a null unit-output pointer.

## Task creation, polling and cancellation

A task factory is a synchronous interface method returning `OwnedTask<T>`.
The plugin can construct it with `OwnedTask::new(async move { ... })`, where the
future is `'static` and returns `Result<T>`, and `T: AbiValue`. The generated
proxy returns `Task<T>`, which implements `Future` when `alloc` is enabled.
`Task::poll_raw` accepts a protocol waker without requiring host allocation.
An asynchronous result currently contains a stable value, not an owned buffer
or another resource. General owning resources remain separate interface results.

The task owns all data needed after creation. A factory receiving `&[u8]` or
`&str` must finish using that borrowed input before returning, or copy the needed
data into its own task state. The future cannot borrow its originating object.
Consequently its host wrapper may outlive the creating proxy, and a task from an
old generation can finish after a new generation has become current.

`RawTask` contains `context` and a pointer to `TaskVTable`. The host reads the
table's `u32 size` before its remaining mandatory prefix, requires extension
version 1 and all function slots, and verifies output size, alignment and full
schema against `T::Wire`. Invalid owned results pin the library because an
unknown release path cannot safely be invoked.

| Operation | Contract |
| --- | --- |
| `poll(context, waker, out, state)` | Serialized on the creating thread; waker borrowed for this call |
| `state = 0`, status 0 | Pending; no initialized result; arrange a wake when progress is possible |
| `state = 1`, status 0 | Ready; `out` contains one initialized `T::Wire` |
| Nonzero status | Error; do not read `out`; positive application codes remain errors |
| `cancel(context)` | Idempotent on completed/cancelled tasks; success stops future polling; Busy retains ownership for retry |
| `release(context)` | Consumes the one task owner, cancels/destroys remaining state; return only after required cleanup |

State and status are separate, so application error 1 cannot be confused with
Pending. Generated Rust adapters preserve the output on error. Re-polling a
completed task returns `Completed`; a cancelled task returns `Cancelled`; a
task whose poll panicked becomes `Poisoned`. The independent factory remains
usable. A failed release retains its library, with no timeout-forced unloading.

The adapter pins the concrete future in one plugin-owned allocation. It never
passes a Rust Future, Pin, Context or Waker representation through the C ABI.
Waker bridges are cached while the host executor's waker is unchanged. A warmed
Pending poll does not require allocation or a library-lease clone; the initial
bridge setup and a changed waker can allocate. A plugin may independently allocate
inside its future. Measurements distinguish these cases.

## Wakers and reverse callbacks

`RawWaker` contains an opaque context and size/version-prefixed table with
`retain`, `wake`, and `release`. All three operations are thread-safe, and wake
borrows rather than consumes its owner. A poll receives a borrowed view; before
storing it, the plugin retains one reference. Replacing a stored waker releases
the old reference. Each retained reference keeps its context and origin callback
code alive independently of the poll and task.

The Rust bridge uses atomic references and contains unwind-enabled panics in
host callbacks. A failed wake is latched, stops further use of that waker, and
surfaces as an error at the next poll. A waker that itself fails to schedule may
require the executor to poll again explicitly; zplugin cannot repair a broken
executor's notification mechanism. Task polling/cancellation/destruction remains
local-thread even though its notification operations may run on worker threads.

General callbacks deliberately use a stricter model. `Callback::new` owns a
`'static` closure accepting borrowed input/output byte slices; `as_ref()` yields
`CallbackRef<'_>`. Interface methods accept this borrowed view. The plugin may
call it during the method or obtain an owned `Callback` through `try_clone()`.
Retaining does not extend input-buffer borrows: each invocation gets its own
call-scoped buffers. Invoke, retain and release stay on the creating thread;
these callbacks are neither Send nor Sync. Same-context reentry returns Busy
before a second mutable borrow, and a caught callback panic poisons that context.

A raw callback constructor is unsafe: its caller retains the origin code and
context for every cloned owner. Safe generated callback parameters are borrowed
from the caller, and the Rust-owned callback factory participates in that
origin's resource accounting. This version does not generate callback-valued
returns or cross-thread general callbacks. Only the waker protocol is concurrent.

## Owning resources and cooperative shutdown

`OwnedResource::new(handler)` allocates a `ResourceHandler` in the plugin.
The handler declares a stable ID and resource version and handles a numeric
operation with borrowed input/output buffers. The host receives `Resource`, calls
`check(id, version)` before using its application protocol, and releases it
through its original library. Its state must be independent of its originating
instance. No Rust Box, trait object, file object or allocator layout crosses ABI.

Each task, resource and owned buffer holds its own host library lease. Destruction
runs the foreign release function before dropping that lease. Allocated Rust
plugin resources also participate in cooperative quiescence. Failed future,
object, or callback-capture cleanup latches the image as non-quiescent, including
destructors that panic after partially running. This intentionally retains code
instead of treating an interrupted cleanup as success.

Physical unloading additionally requires the unsafe `UnloadSafe` promise.
Reference-count zero alone does not establish that a worker's final waker/drop
function has returned through plugin code. Workers, reactor registrations,
callbacks and their return paths must be joined or synchronously drained before
quiescence can succeed. The real Rust async example joins its worker in the
future destructor on completion, cancellation and release, and stress-tests a
wake racing cancellation. Dropping a Future does not automatically shut down
Tokio or another external executor. Use pinned loading when such retirement
cannot be coordinated. No async runtime is installed or entered by zplugin.

## Evidence and limits

The executable evidence is `tests/{values,tasks,failed_cleanup,resources_callbacks}.rs`,
compile-fail fixtures, `scripts/async.py`, `scripts/foreign.py` and
`scripts/legacy.py`. The release report records exact executed platforms, compiler
versions and counts. Tests use real independently built dynamic libraries,
including both directions against the immutable published v0.1.0 source.

The protocols provide compatibility and ownership checks for trusted in-process
native code. They do not isolate memory faults, prevent malicious native code,
catch `panic=abort`, or portably intercept C++ exceptions or Zig panic/abort.
Foreign plugins must contain their own exceptions before an ABI return.

Design review: the contribution is three additive protocols with typed bindings;
the writing maps each promise to its ownership rule; measurements separate setup
from warm polling; validation covers supported native platforms and failure paths;
and the shutdown contract explicitly includes work that reference counts cannot
prove quiescent. Unexecuted configurations receive no support claim.
