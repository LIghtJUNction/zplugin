# Untrusted Wasm plugins

## Trust boundary

`zplugin::wasm::Plugin` executes untrusted Wasm32 code in Wasmi 2.0. The backend
validates the module, denies every import and start section, checks its interface,
and meters guest execution. Native `Plugin::load` remains an unsafe operation for
trusted machine code. Loading a native library never acquires Wasm isolation.

The boundary passes bytes through checked linear-memory offsets. No native
function pointer, Rust reference, allocator, task, waker, or resource handle is
exposed to the guest. Each instance owns its memory and state. No filesystem,
network, environment variable, clock, process or WASI API is linked. An interface
ID is a compatibility label, not proof of identity or permission.

The `wasm` feature is optional and implies `alloc`, not `std`. Native/default
features retain their zero-third-party-runtime dependency policy and Rust 1.85
MSRV. The Wasm host uses Rust 1.86+ and Wasmi 2.0.0 with `default-features = false`
and explicit `stable`, `validate`, `portable-dispatch`, and `libm`. Its locked
runtime graph adds eight packages: Wasmi, wasmi_core, wasmi_ir,
wasmi_collections, wasmparser, spin, libm, and bitflags. No JIT compiler, text
parser, WASI implementation, or third-party executor is enabled by this feature.
The guest SDK needs only the default zplugin crate and `core`.

## Build and call

The runnable [guest](../examples/wasm/guest.rs) implements `wasm_guest::Guest`.
Its state has a const initializer and its input/output scratch arrays have fixed
capacities. `export_wasm!` supplies the exports and checked status/length adapter:

```rust,ignore
zplugin::export_wasm!(Demo, constructor = Demo::new(),
    interface = 0x7a70_6c75_6769_6e01_u64, version = 1,
    input = 1_048_576, output = 1_048_576);
```

A standalone guest crate uses `crate-type = ["cdylib"]`, a default-features-disabled
zplugin dependency, and `panic = "abort"`. Compile with
`cargo build --release --target wasm32-unknown-unknown`. A `no_std` guest supplies
its own panic handler and, only if needed, allocator. A guest can also use Rust
`std` facilities that work without imports; unsupported I/O/WASI imports are
rejected at load. The example has no allocator and traps on panic.

The [host example](../examples/wasm_host.rs) reads at most the module-size limit
plus one byte, calls the safe `wasm::Plugin::new(bytes, interface, limits)`, and
then calls `plugin.call(operation, input, output)`. The constructor validates,
compiles and binds once. Repeated calls use cached typed exports and reusable
guest buffers. No text or structured serialization is imposed; application
interfaces define their own byte encoding, such as explicit little-endian values.

```sh
rustup target add wasm32-unknown-unknown
python3 scripts/wasm.py
cargo run --release --example wasm_host --features wasm,std -- target/wasm-pinned/demo.wasm
```

`scripts/wasm.py` also accepts `--guest-toolchain` and `--host-toolchain` to verify
independent compilation. The example's operations are echo (0), uppercase (1),
stateful counter (2), application error (3), fuel loop (4), trap (5), and denied
memory growth (6). Failure operations are deliberate test cases, not host APIs.

## Frozen byte-buffer protocol v1

This protocol is separate from native `zplugin_entry_v1`. A compatible core Wasm
binary exports exactly the required items below; unrelated exports are ignored.
The engine rejects imports, start sections, shared memory, and multiple memories.
The adapter additionally rejects memory64 even if another Cargo dependency
enables that engine feature. A module may have one bounded internal table.

| Export | Wasm signature | Purpose |
| --- | --- | --- |
| `memory` | Non-shared Wasm32 memory | Instance-local data |
| `zplugin_wasm_info_v1` | `(i32) -> i64` | Versioned metadata query |
| `zplugin_wasm_call_v1` | `(i32, i32, i32) -> i64` | Operation, input length, output capacity |

Metadata uses unsigned bit patterns in the integer results:

| Query field | Result |
| --- | --- |
| 0 | Protocol version, exactly 1 |
| 1 | Application-defined 64-bit interface ID |
| 2 | Application interface version; must fit `u32` and exactly match |
| 3, 4 | Input scratch offset and capacity; each fits `u32` |
| 5, 6 | Output scratch offset and capacity; each fits `u32` |

All seven queries share one fuel budget. Binding checks both complete scratch
ranges against current linear memory and host transfer limits. Nonempty buffers
must not overlap. Byte alignment is sufficient; offset zero is legal, including
for nonempty storage. Empty ranges must still identify an in-bounds position.
The negotiated offsets and capacities are cached for the instance lifetime.

Before each call the host copies the input and zeroes the granted output range.
The guest receives the operation number, actual input length, and
`min(host_output.len(), negotiated_output_capacity)`. It returns the signed
status in the low 32 bits and the output byte count in the high 32 bits. Status
zero means success. Nonzero statuses use the native status-number convention
(`-3` is short output, `-7` unsupported, positive codes are application errors)
but are surfaced as `wasm::Error::GuestStatus(i32)`. Output length is ignored on
failure. On success it must not exceed the granted capacity.

The host copies output only after successful status and length checks. Caller
output is unchanged on any error. A trap, engine failure, or invalid success
length permanently poisons the instance. Application statuses and oversized
host input leave the instance usable; application code must document whether
it modifies its own state before an error. Calls use a fresh budget, never an
accumulating fuel balance. Dropping the plugin releases its store and allocations
without executing a guest destructor.

Any language that emits these exports can implement this protocol. The delivered
Wasm SDK and independently compiled example are Rust; the existing generated
C/C++/Zig native ABI headers describe native libraries, not Wasm memory offsets.
Arbitrary WASI programs, browser/wasm-bindgen modules requiring JavaScript imports,
Component Model binaries and native dynamic libraries are not accepted by this ABI.

## Limits and lifecycle

Default limits are explicit and customizable per instance:

| Resource | Default limit | Enforcement point |
| --- | ---: | --- |
| Binary module bytes | 4 MiB | Before parsing |
| Guest linear memory | 16 MiB, one memory | Instantiation and every growth |
| Table elements | 4,096, one table | Instantiation and every growth |
| Input / output capacity | 1 MiB each | Binding and calls |
| Guest execution | 10,000,000 fuel | Combined metadata queries; each subsequent call |
| Interpreter value stack | 64 KiB | Execution |
| Guest call depth | 128 | Execution |
| Store instances | 1 | Instantiation |

Wasmi's strict validation/compilation limits and eager compilation are always
enabled. Over-limit `memory.grow` and `table.grow` return Wasm's normal failure
sentinel; guests can handle it and continue. Actual traps stop the call. A plugin
may be dropped immediately after failure because it has no external callbacks,
threads or host resources to drain. The host type is deliberately `!Send + !Sync`;
all operations are serialized through `&mut self`. Construct separate instances
on worker threads to obtain independent state.

Reuse also retains guest knowledge: a malicious stateful plugin can remember
earlier inputs within its own memory. Use a fresh instance for a new tenant or
confidential session. No host-side instance/module cache is created implicitly.
The application controls the number of resident instances and aggregate budgets.
To switch implementations, construct and validate a new instance first, then
replace the active instance at a request boundary. Keep the current instance
if the replacement fails. Native `hot::Generations` is not a Wasm manager.

## Security boundaries

The memory limit covers guest linear memory, not whole-process RSS. The parser,
compiled representation, store metadata, host buffers, allocator and interpreter
stack add memory. The binary-size cap and strict engine limits constrain accepted
input, but they do not impose a process memory quota. The host must bound input
file reads and the number/rate of module compilations and active instances.

Fuel limits metered guest execution, including the metadata queries and dynamic
bulk-memory operations. It is not a wall-clock timeout for validation/compilation,
and different Wasmi releases may meter differently. No host callbacks are present
whose execution could evade the guest budget. This version does not implement
asynchronous Wasm tasks, WASI capability grants, or host-call bridges.

Isolation depends on the correctness of Wasmi and its dependencies. This adapter
uses safe engine APIs and forbids unsafe Rust in the host module. Its tests do
not certify immunity to engine defects, hardware side channels, or host OOM.
For high-risk multi-tenant execution, add a separate OS process/container with
its own memory/CPU/syscall policy. That additional boundary is outside this library.

Primary references: [WebAssembly security](https://webassembly.org/docs/security/),
[Wasmi 2.0](https://docs.rs/crate/wasmi/2.0.0),
[engine configuration](https://docs.rs/wasmi/latest/wasmi/struct.Config.html),
[resource-limiter scope](https://docs.rs/wasmi/latest/wasmi/trait.ResourceLimiter.html).
The Wasmi project lists audits of older releases; this document makes no audit
claim for the new adapter or Wasmi 2.0.

## Claim and evidence review

The [binary conformance suite](../tests/wasm.rs) uses no third-party fixture
generator. The [independent build runner](../scripts/wasm.py) executes the real
Rust guest; CI uses Linux, macOS and Windows native hosts. Run both locally:

```sh
cargo test --locked -p zplugin --features wasm --test wasm
cargo test --locked -p zplugin --features wasm,wasmi/memory64 --test wasm
python3 scripts/wasm.py
```

| Review dimension | Claim and evidence | Boundary |
| --- | --- | --- |
| Contribution | Import-free safe Wasm load/call path; real guest round trips | Does not retrofit native isolation |
| Clarity | Export table, scratch rules, defaults and complete examples above | Only this protocol, not arbitrary Wasm applications |
| Experimental strength | Traps, fuel, memory/table growth, buffers, malformed modules, mutation cases | Regression tests, not exhaustive formal verification |
| Evaluation completeness | MSRV, native-host CI, cross-compiler guests, feature-unification regression | CI evidence identifies actual runner targets |
| Design soundness | Safe engine APIs, mandatory limits, poisoned-state handling, no guest Drop code | In-process engine and allocator remain trusted |
