# zplugin 0.3.0 Wasm measurements

## Result and scope

The warmed sandbox calls in this experiment made zero Rust heap allocations.
A 64-byte echo round trip took 176.627 ns and a 1 MiB round trip took 78.186 µs
under the aggregation below. These numbers include fuel metering and the
input/output memory boundary. They describe this guest and machine, not arbitrary
plugin workloads, and do not imply native-call performance or zero-copy behavior.

The benchmark source is clean commit
[`e957b45e6f9a071a8ce3030fd75d3ca34334c71b`](https://github.com/LIghtJUNction/zplugin/commit/e957b45e6f9a071a8ce3030fd75d3ca34334c71b).
Later release changes update versions and evidence; the measured adapter and
benchmark are identified by this immutable source. The native ABI's prior
measurements remain in [the 0.2.0 report](performance-0.2.0.md).

## Method

The host was Linux 6.18.57, x86_64, glibc 2.41 on an AMD EPYC 9V74 80-Core
processor. Each benchmark process was pinned to CPU 0. Rust was 1.100.0
(`0ef1bc386059596dada8fb65b50425aab2ea637a`, LLVM 23.1.5). The release profile uses
optimization level 3, thin LTO, one codegen unit and host `panic=unwind`.
The Rust guest uses `panic=abort`, no allocator, and a 64 KiB linear-memory stack.
Its binary is 993 bytes, SHA-256
`3417f3905d2929420e5ae30c8f81fe2b705d355e8ae001b018587318a6874875`.

Three fresh processes each collect 31 batches per case, after up to 64 warm-up
iterations. Each batch has 10 load/drop cycles, 10,000 empty/64-byte/counter
calls, 1,000 4 KiB calls, or 16 1 MiB calls. Inputs, returned lengths and output
references pass through `black_box`; outputs and the final stateful counter
are checked. Timed success paths preserve the same guest work in both variants.
The allocator instrumentation is enabled for both variants, including load time.

The comparator is a cached Wasmi typed function, not a native function. It owns
its store on one thread, uses the same Wasmi 2.0 engine, finite fuel,
memory/table/stack limits, scratch negotiation, zeroing, copies and lifecycle.
zplugin adds its public error handling, size checks and poisoning. Benchmark
baseline errors abort the benchmark, so it is not a replacement safe loader API.
All comparisons below are successful calls with already-bound instances.

Quantiles apply to per-batch mean nanoseconds per operation. They use
`sorted[floor(p × (n − 1))]`, with `n = 31`. The headline value is the median of
the three process p50s. These are not individual-call p95/P99 latency estimates.
The machine was not an isolated real-time host; scheduling noise remains in the
raw samples and ranges rather than being removed as outliers.

## Timings

Smaller times are better. Ranges are the minimum and maximum process p50.

| Operation | zplugin median p50 | zplugin p50 range | Cached Wasmi median p50 |
| --- | ---: | ---: | ---: |
| Load + validate + bind + drop | 44.196 µs | 43.642–46.090 µs | 43.098 µs |
| Empty round trip | 145.229 ns | 132.466–158.200 ns | 139.701 ns |
| 64 B round trip | 176.627 ns | 167.381–177.618 ns | 164.530 ns |
| 4 KiB round trip | 321.324 ns | 320.934–322.786 ns | 299.942 ns |
| 1 MiB round trip | 78.186 µs | 78.082–79.734 µs | 81.052 µs |
| Stateful counter, 8 B output | 140.179 ns | 136.150–142.822 ns | 144.982 ns |

The small-buffer wrapper adds tens of nanoseconds in this run; this does not
justify a universal speed-ranking claim. The 1 MiB result includes host input
copy, guest copy, host output copy and output zeroing. Differences of a few
percent move with machine noise and code layout. The raw cached empty-call
process p50s span 131.838–178.054 ns, illustrating that variation.

## Allocation and memory

All warmed calls in the table recorded zero allocator calls and zero newly
requested allocation bytes. Guest code that grows memory, a different execution
stack shape, or a different workload may allocate; this result concerns the
reused fixed-buffer example. Every measured batch, including complete load/drop
cycles, ended with zero net change in requested live allocation bytes.

| Metric | Measured value |
| --- | ---: |
| Rust allocations per complete load/drop | 121 |
| Total requested bytes per load/drop | 2,256,498 B |
| Peak additional requested live bytes during load/drop | 2,248,188 B |
| Retained requested live bytes immediately after binding | 2,247,430 B |
| Guest linear memory | 2,228,224 B |
| Guest input + output scratch storage | 2 × 1 MiB |

The example deliberately reserves two 1 MiB buffers to cover the largest transfer
case. Smaller application buffers reduce that footprint. The explicit 64 KiB
guest stack also avoids reserving the Rust target's larger default stack for
this small guest. The default host linear-memory ceiling remains 16 MiB and is
independent of the instance's actual initial allocation.

Allocator counters measure requested `Layout` sizes, not RSS. They exclude
allocator metadata, OS memory and implementation-internal temporary storage
during `realloc`; a successful reallocation counts the requested new size.
Guest memory, retained allocator bytes and total process memory are distinct
measurements. This report makes no process-RSS cap or engine-audit claim.

## Reproduction and evidence

```sh
rustup target add wasm32-unknown-unknown
python3 scripts/wasm.py
python3 scripts/wasm_bench.py --output artifacts/wasm-bench
```

The benchmark implementation is [wasm_bench.rs](../examples/wasm_bench.rs);
the runner records machine/compiler/flags/module/source metadata in
[evidence.json](benchmarks/v0.3.0/evidence.json). Every sample is retained in
[run 1](benchmarks/v0.3.0/wasm-linux-x86_64-run1.csv),
[run 2](benchmarks/v0.3.0/wasm-linux-x86_64-run2.csv),
[run 3](benchmarks/v0.3.0/wasm-linux-x86_64-run3.csv), and
[summary.csv](benchmarks/v0.3.0/summary.csv).

## Evidence review

| Review dimension | Evidence-backed conclusion | Remaining boundary |
| --- | --- | --- |
| Contribution | A measured sandbox adapter with error/lifecycle policy | Not a new Wasm engine |
| Clarity | Immutable source, module hash, flags and full samples | Reproduction timings depend on the machine |
| Experimental strength | Matched-engine baseline; multiple sizes and fresh processes | One CPU architecture measured |
| Evaluation completeness | Lifecycle, steady calls, allocations, memory and batch distributions | No individual-call tail or OS RSS measurement |
| Design soundness | Warm reuse avoids allocator work in the tested guest | Other guests and memory growth can allocate |
