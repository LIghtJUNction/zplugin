# Performance evidence

Measured on 2026-11-12T01:55:00+08:00
(Asia/Taipei), Linux x86-64. This report records actual
runs of the included benchmark; macOS and Windows performance remains unmeasured.

## Result

The full typed proxy completed the scalar fixture in **3.644 ns/call**,
using the median of three run medians. This is approximately
**274.4 million calls/second** for this tiny workload.
The equivalent libloading baseline measured **3.665 ns/call**.
Their difference is small relative to shared-machine variation. These results
support low dispatch overhead, without a general speedup claim over cached
function pointers or libloading.

Every measured borrowed scalar/byte call recorded **zero host and plugin Rust
heap allocations**. Creating an example object allocated 16 bytes in the plugin;
ordinary calls then reused that object, its cached methods and caller buffers.

## Reproduce

From the repository root with the pinned toolchain installed:

```sh
python3 tools/bench/run.py
```

The runner builds `demo-plugin` and `zplugin-bench` in
`artifacts/benchmark-target`, with `--release --locked` and the plugin's
`instrument` feature. It writes three raw CSV files under `docs/benchmarks`.
An optional first argument selects a different output directory. On Linux it pins
the process to the first CPU in its permitted affinity set. The three fresh
processes rotate the order of the six dispatch baselines by 0, 2 and 4 positions.

Recorded machine: **AMD EPYC 9V74 80-Core Processor**, pinned CPU 0, a shared
execution environment. Compiler: **rustc 1.99.1 (58cf8f9c3 2026-10-13)**,
LLVM 23.1.1, target `x86_64-unknown-linux-gnu`. Build: `opt-level=3`,
`lto=thin`, `codegen-units=1`, `panic=unwind`, no additional `RUSTFLAGS`.
The runtime has `std`, `macros` and `hot-reload`; libloading 0.8.9 belongs only to
the unpublished benchmark package.

## Dispatch and buffer processing

Each cell is the median of three independent run medians. Scalar, 64 B and
4 KiB columns use nanoseconds; the 1 MiB column uses microseconds. The byte
operation uppercases ASCII into a reused caller-owned output buffer.

| Path | Scalar ns ↓ | 64 B ns ↓ | 4 KiB ns ↓ | 1 MiB µs ↓ |
| --- | ---: | ---: | ---: | ---: |
| static-inline | 1.354 | 3.733 | 159.725 | 38.607 |
| static-noinline | 1.635 | 3.929 | 160.989 | 38.678 |
| cached-extern-C | 2.508 | 5.755 | 158.503 | 39.003 |
| cached-generated-adapter | 3.789 | 13.211 | 163.718 | 38.089 |
| libloading-cached | 3.665 | 13.280 | 167.496 | 38.023 |
| zplugin-proxy | 3.644 | 13.523 | 169.580 | 37.770 |

The scalar proxy run medians ranged **3.418–3.657 ns**.
For the proxy's larger transfers they ranged **13.194–13.642 ns**
(64 B), **167.324–170.772 ns** (4 KiB), and
**31.980–38.917 µs**
(1 MiB). The large-transfer variation is material; the raw runs are preserved
without discarding the slower result.

All six paths execute the same arithmetic or byte transformation and update a
per-object call counter. Static inline, static noinline and the thin C function
pointer are lower-bound controls: they omit plugin validation, panic containment
and instance guards. The cached-generated-adapter, libloading and typed-proxy
paths use the **same generated adapter**, plugin object contract, creating
thread and live library ownership. The libloading control resolves an exported
function-pointer data symbol once; it never performs symbol lookup in the timed
loop. All foreign-call controls rebuild the same `repr(C)` argument record on
every call, matching the proxy's packing work.

`black_box` protects inputs, results and buffers from dead-code elimination.
The fixture's final call counter and transformed output are also consumed.
The generated proxy methods have `#[inline]`, exposing small packing/status
wrappers to downstream optimization. Method pointers are cached at binding;
individual calls perform no registry lookup, lock, serialization or reference
count clone. This design is visible in the generated code and supported by the
allocation measurements; no numerical improvement is attributed to a single
optimization without an isolated experiment.

## Loading, lifetimes and switching

These figures use nanoseconds per operation. The range covers the three run
medians. Rust allocation counts cover host and plugin for object/buffer rows;
load and hot-switch rows count host Rust allocations only.

| Operation | Median ns ↓ | Run-median range ns | Allocations/op |
| --- | ---: | ---: | ---: |
| lifecycle/remap-load-drop | 35188.150 | 34675.280–35429.500 | 2 |
| lifecycle/warm-load-drop | 3099.050 | 3069.410–5087.140 | 2 |
| lifecycle/core-warm-load-drop | 163.340 | 161.940–209.610 | 0 |
| lifecycle/interface-bind | 29.832 | 29.536–30.039 | 0 |
| lifecycle/create-drop | 17.759 | 17.424–17.864 | 1 |
| lifecycle/owned-64B-create-release | 43.189 | 42.754–44.314 | 1 |
| hot/request-lease | 0.706 | 0.706–0.706 | 0 |
| hot/session-reused-call | 3.469 | 3.466–3.592 | 0 |
| hot/switch-load-bind-create-collect | 29318.050 | 27065.650–29761.700 | 3 |

`remap-load-drop` closes the only live handle each iteration, then loads again;
filesystem page caches are warm. `warm-load-drop` retains another handle, so it
measures acquiring/releasing an already mapped image plus the std path helper.
`core-warm-load-drop` reuses a NUL-terminated path and calls the no-allocator
loader. The std helper canonicalizes a path, which explains its two host
allocations. No cold-storage claim follows from these measurements.

`create-drop` includes both construction and destruction. The 64 B owned-return
row includes plugin allocation, copying the requested data, host wrapping and
release in the originating plugin. Instrumentation uses counting allocators,
which add atomic operations to allocation/free paths and can change their
absolute timings. OS loader allocations made inside libc are outside these
Rust allocator counters.

The hot-switch measurement uses two independent copied library files and a
resident-old-generation limit of two. Each iteration loads, validates, constructs
a new typed instance, commits the new generation, drops the instance and
collects the old image. File copying happens before timing. A request lease is
acquired once; the reused-session-call row then makes repeated calls without
cloning that lease. Hot rows count host allocations because every copied image
owns a separate, independently retired plugin allocator.

## Latency distribution

Ordinary rows contain 41 samples of batches: scalar 200,000 calls, 64 B 100,000,
4 KiB 4,000, 1 MiB 16, bind/create/owned-release 10,000, load 100, and full switch
20 operations per sample. CSV p50/p95/p99 values for those rows are quantiles of
**batch means**, not individual request latencies. Up to 1,024 operations warm
each row before timing.

A separate 20,001-sample, one-call measurement records individual elapsed times.
The timer floor is shown alongside it; these values include timer overhead.

| Run | Timer-floor p50 ns | Proxy p50 ns | Proxy p95 ns | Proxy p99 ns | Proxy max ns |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 20.000 | 21.000 | 30.000 | 31.000 | 431.000 |
| 2 | 20.000 | 21.000 | 30.000 | 31.000 | 143976.000 |
| 3 | 20.000 | 21.000 | 30.000 | 31.000 | 320.000 |

The timer floor is much larger than the batched scalar call time. Use batched
results for dispatch cost and the raw latency distribution to observe scheduling
and timer effects. Do not subtract medians to manufacture per-call tail latency.

## Memory

On this build, `Plugin` occupies **32 stack bytes**, `Binding` 64, `Instance` 72,
and the eight-method `DemoProxy` 136. These are Rust-side observations for this
feature/target combination, not frozen C ABI sizes. Borrowed instance creation
adds **zero host heap bytes** and requests **16 plugin heap bytes**. The
1,024-instance experiment requests exactly **16,384 plugin bytes**, then returns
to its prior plugin live-byte count after destruction; the host vector's
preallocated storage is excluded from that delta. These counts exclude the
system allocator's own bookkeeping and reserved arena capacity.

The three process-first loads took 107.081, 644.143, 250.086 µs,
and increased Linux RSS by 208, 224, 228 KiB.
RSS includes code/data pages and loader/runtime effects. It is coarser than the
allocator counts and depends on OS paging. Repeated data-buffer calls allocate
neither boundary buffers nor serialization storage.

## Evidence and review

Raw data: [run 1](benchmarks/linux-x86_64-run1.csv),
[run 2](benchmarks/linux-x86_64-run2.csv),
[run 3](benchmarks/linux-x86_64-run3.csv).
Measurement implementation: [benchmark](../tools/bench/src/main.rs),
[runner](../tools/bench/run.py),
[plugin allocator](../examples/plugin/src/measurements.rs).

| Claim | Evidence | Status |
| --- | --- | --- |
| Borrowed calls force no Rust heap allocations | Both allocators record zero across all scalar/byte rows | Measured on Linux |
| Full proxy has low dispatch overhead | Three scalar medians and equivalent cached adapter controls | Measured for this workload |
| Plugin resources are released in their origin | Owning buffer release row, object byte accounting, integration tests | Implemented and exercised |
| Request leases avoid per-call reference counting | Reused-session benchmark and proxy implementation | Implemented and measured |
| Performance generalizes to other CPUs and OSes | No measurements here | Future measurement |

Review: the contribution is typed ABI/lifetime management with small measured
cost; terminology and units are explicit; all reported numbers map to raw data;
controls cover dispatch, buffers, allocation, memory, loading and hot switching;
the shared-machine variance and narrower semantics of thin controls remain
visible. No asynchronous workload or cross-platform performance result is claimed.
