# Performance evidence: 0.2.0 extensions

Measured on 2026-11-13 (Asia/Taipei), Linux x86-64. This report separates
steady-state dispatch, task initialization, changed wakers, and worker shutdown.
It makes no macOS/Windows timing or universal speedup claim.

## Results

The typed synchronous proxy measured **3.474 ns/call** for the scalar fixture
(about 287.9 million calls/second). A warmed pending task measured **14.018 ns/poll**
through `Task::poll_raw`, versus **12.216 ns/poll** through its cached C function
pointer. Both paths recorded zero host and plugin Rust heap allocations.
The separate Rust `Future::poll` adapter measured **14.740 ns/poll**, also with
zero allocations while the same waker was reused.

Task creation and waker replacement are not allocation-free. The example's ready
task requests 56 plugin bytes on creation. First polling creates the needed
waker bridges; changing a C waker requests another 40 bytes, while changing a
Rust executor waker requests 40 bytes on each side. The optimization is reuse
of already-created bridges, not an assertion that every asynchronous operation
requires no memory.

## Reproduction and measurement scope

```sh
python3 tools/bench/run.py docs/benchmarks/v0.2.0
python3 tools/bench/extensions.py
```

The two runners build instrumented real `cdylib` plugins and run three fresh
processes, pinned to CPU 0. Measurements were not run concurrently with this
task's compiler jobs. The machine is a shared **AMD EPYC 9V74 80-Core Processor**,
Linux 6.18.57, glibc 2.41. Rust is **1.100.0** (`0ef1bc386`, LLVM 23.1.5), target
`x86_64-unknown-linux-gnu`; release settings are `opt-level=3`, thin LTO,
`codegen-units=1`, `panic=unwind`, with no extra `RUSTFLAGS`.

The runtime enables `std`, `macros`, and `hot-reload`. `libloading 0.8.9` is a
dependency only of the unpublished comparison tool. Compiler, CPU, build flags,
sample counts, and allocator observations are retained in the raw CSVs.
Extension CSVs additionally record the pre-commit source and dirty-file list:
the changes are the benchmark and CI/toolchain updates committed with this
report, not an unreported alternative runtime. The old 0.1.0 measurements remain
unchanged in [the historical report](performance.md).

Each table below uses the **median of three run medians**. Ordinary CSV p50,
p95, and p99 values are quantiles of **batch means**, not single-call latency
percentiles. `black_box`, observed outputs, and object counters prevent the
work from being optimized away. Counting allocators add atomic operations to
allocation/free paths and therefore affect their absolute timings.

## Synchronous dispatch and data transfer

All paths execute the same scalar arithmetic or ASCII uppercase transformation
into a reused caller-owned output buffer. Scalar, 64 B and 4 KiB columns use
nanoseconds; 1 MiB uses microseconds.

| Path | Scalar ns ↓ | 64 B ns ↓ | 4 KiB ns ↓ | 1 MiB µs ↓ |
| --- | ---: | ---: | ---: | ---: |
| Static inline | 1.370 | 3.761 | 166.118 | 38.375 |
| Static noinline | 1.680 | 3.906 | 163.431 | 38.723 |
| Cached thin extern-C | 2.558 | 5.740 | 166.341 | 38.687 |
| Cached generated C adapter | 3.668 | 13.225 | 135.139 | 38.142 |
| libloading, cached symbol | 3.725 | 13.683 | 167.928 | 35.448 |
| zplugin typed proxy | 3.474 | 13.370 | 174.716 | 38.279 |

Static calls and the thin C pointer are lower-bound controls: they omit plugin
validation, panic containment, and instance guards. The final three paths call
the same generated adapter with matching object ownership, creating-thread
rules, argument packing, and live library ownership. Symbol resolution is outside
timing. Baseline order rotates across the three processes.

Every measured scalar/buffer path recorded **zero host and plugin allocations**.
The proxy scalar run medians ranged 3.473–3.546 ns. Its 4 KiB run medians ranged
141.310–177.432 ns, and its 1 MiB medians ranged 30.803–38.727 µs. Variation is
large enough that the ordering of the same-adapter buffer controls does not
establish a general performance ranking. No slower sample has been discarded.

There are 41 samples per row: 200,000 scalar calls, 100,000 64 B calls, 4,000
4 KiB calls, or 16 1 MiB calls per sample. Up to 1,024 operations warm each row.

## Cross-library tasks

The cached C and proxy C controls use the same stateless C waker, retained
library lifetime, and pending worker implementation. That worker is blocked on
a condition variable; its poll takes the same plugin-side mutex in both paths.
The Rust Future rows separately measure the executor-waker bridge. Task/waker
setup is outside warmed poll timing; each poll row receives 32,000 warmup polls.

| Operation | Median ns/op ↓ | Three p50 range ns | Host allocs/op | Plugin allocs/op |
| --- | ---: | ---: | ---: | ---: |
| Ready task: create + drop | 20.741 | 20.431–23.055 | 0 | 1 |
| Ready task: create + C poll + drop | 64.367 | 62.484–70.836 | 0 | 2 |
| Ready task: create + Future poll + drop | 77.246 | 75.404–83.747 | 1 | 2 |
| Pending: cached C poll | 12.216 | 12.111–12.230 | 0 | 0 |
| Pending: proxy C poll | 14.018 | 13.948–14.097 | 0 | 0 |
| Pending: Rust Future poll | 14.740 | 14.739–14.777 | 0 | 0 |
| Pending: replace C waker each poll | 45.153 | 44.424–45.169 | 0 | 1 |
| Pending: replace Rust waker each poll | 73.983 | 73.477–74.272 | 1 | 1 |
| Worker: create + cancel + join + drop | 8,663.000 | 8,477.500–14,241.500 | 0 | 8 |
| Worker: create + signal + complete + join + drop | 8,197.500 | 7,972.000–29,054.000 | 0 | 8 |

The measured proxy-C delta is **1.802 ns/poll** over cached C in this fixture.
This is a local dispatch comparison, not an I/O throughput or executor benchmark.
The worker lifecycle rows include actual thread creation, scheduling, and joining;
their broad run ranges are not a portable cancellation-latency guarantee.

Each row has 101 samples. Pending rows use 10,000 operations per sample, ready
lifecycle rows use 1,000, and worker lifecycle rows use two. The median of the
three batch-p99 values is 22.781 ns for proxy C polling and 24.270 ns for Future
polling; these remain **batch-mean quantiles**, not per-task tail latency.

Ready create/drop requests 56 bytes; first C poll plus lifecycle requests 96;
first Rust Future poll plus lifecycle requests 136. C-waker replacement requests
40 bytes and Rust-waker replacement 80 total. The worker lifecycle requests
327 plugin bytes across eight Rust allocations. Native OS/thread/loader
allocations are outside these counters. Every row has zero net live-byte change,
and final plugin live bytes return to zero after all objects, tasks, and workers
exit. The benchmark then verifies successful cooperative close.

## Loading, ownership, switching, and memory

| Operation | Median ns/op ↓ | Three p50 range ns | Rust allocations/op |
| --- | ---: | ---: | ---: |
| Remap load + drop, warm filesystem cache | 33,399.230 | 30,032.050–37,288.600 | 2 |
| Already-mapped std path load + drop | 3,123.810 | 3,113.790–3,185.500 | 2 |
| Already-mapped core C-string load + drop | 160.540 | 160.340–161.140 | 0 |
| Interface bind | 29.612 | 29.564–29.686 | 0 |
| Object create + drop | 19.127 | 17.770–19.456 | 1 |
| Owned 64 B result create + release | 41.649 | 41.536–53.604 | 1 |
| Request library lease | 0.706 | 0.706–0.706 | 0 |
| Reused-session scalar call | 3.435 | 3.312–3.446 | 0 |
| Hot switch: load + bind + create + collect | 26,385.800 | 24,940.600–36,099.400 | 3 host |

The std load helper canonicalizes a path; the core control reuses an existing
NUL-terminated path. Their allocation difference is not a difference in plugin
ABI. Load rows use 100 operations/sample, bind/create/owned-result rows 10,000,
and hot-switch rows 20. The hot test pre-copies two independent library files,
uses an old-generation limit of two, and excludes file copying from timing.
Hot rows count host allocations only because copied plugin images have separate
allocators. A successful OS close releases our loader reference, not a guarantee
that all external references or mapped pages disappear.

On this feature/target build, Rust-side `Plugin`, `Binding`, `Instance`, and the
eight-method demo proxy occupy 32, 64, 72, and 136 stack bytes. These are observed
Rust layouts, not public ABI guarantees. A borrowed demo object adds zero host
heap bytes and 16 plugin bytes; 1,024 objects add exactly 16,384 plugin bytes and
return to the preceding live-byte count after destruction. Preallocated host
vector storage and allocator bookkeeping are excluded from that delta.

The first load in each fresh process took 91.828, 88.854, and 124.007 µs. Linux
RSS increased by 224, 184, and 208 KiB respectively. RSS includes code pages,
loader/runtime state, and OS paging; it is not the runtime's per-object memory
requirement and is not a cold-storage result.

## Single-call timing and limits

The synchronous suite also measures 20,001 individual scalar calls per process.
All three runs recorded proxy p50/p95/p99 of 21/30/31 ns, while the timer-floor
p50 was 20 ns. Maximum observations were 44,066, 151, and 2,083 ns. Timer cost
dominates this tiny operation; do not subtract quantiles to manufacture a
single-call tail-latency claim. Use batch measurements for dispatch cost.

No production I/O workload, external async executor, cancellation deadline,
macOS/Windows timing, or other-CPU result is claimed. Different business methods
can allocate or block independently of the boundary. Physical unloading still
requires the documented join/drain contract; a benchmark cannot enforce that
promise for arbitrary native plugins.

## Evidence and review

Raw synchronous data: [run 1](benchmarks/v0.2.0/linux-x86_64-run1.csv),
[run 2](benchmarks/v0.2.0/linux-x86_64-run2.csv),
[run 3](benchmarks/v0.2.0/linux-x86_64-run3.csv).
Raw task data: [run 1](benchmarks/v0.2.0/extensions-linux-x86_64-run1.csv),
[run 2](benchmarks/v0.2.0/extensions-linux-x86_64-run2.csv),
[run 3](benchmarks/v0.2.0/extensions-linux-x86_64-run3.csv).
Implementations: [sync benchmark](../tools/bench/src/main.rs),
[task benchmark](../tools/bench/src/bin/extensions.rs),
[counting allocator](../examples/plugin/src/measurements.rs).

| Claim | Evidence | Status |
| --- | --- | --- |
| Warm borrowed sync calls and reused-waker polls require no Rust allocations in these fixtures | Both allocators, all three raw runs | Measured |
| Task setup and waker replacement allocate | Separate lifecycle/replacement rows and byte counts | Measured, not hidden in warm rows |
| Task wrappers retain and release their originating image | Real-library lifecycle tests and final live-byte/close assertions | Implemented and tested |
| Similar dispatch cost to equivalent cached C calls | Same-adapter sync controls and same-waker pending controls | Workload-specific observation |
| General cross-platform speedup | Not measured | No claim |

Five-dimension review: the contribution is typed ABI/lifetime management, not a
new executor; terminology separates a call, a poll and a whole worker lifecycle;
all numbers map to preserved samples; evaluation covers dispatch, buffers,
allocation, memory, initialization, switching and cancellation; design limits
retain the native-code trust and cooperative shutdown obligations. Shared-host
variance and missing production/cross-platform timing remain explicit.
