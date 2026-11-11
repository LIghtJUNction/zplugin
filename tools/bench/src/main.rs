//! Reproducible native dispatch and lifecycle measurements. No framework is
//! used so the timed operations and allocation accounting remain inspectable.

use core::ffi::c_void;
use demo_interface::DemoApi;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use zplugin::abi::{Bytes, BytesMut, Call};
use zplugin::{Plugin, UnloadPolicy};

struct MeasuredAllocator;
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Preserve the allocator layout and return its result unchanged.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: The caller supplies the original allocation and layout.
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;

#[derive(Clone, Copy)]
struct Counters {
    allocations: extern "C" fn() -> u64,
    bytes: extern "C" fn() -> u64,
    live: extern "C" fn() -> u64,
}

impl Counters {
    fn allocated(self) -> (u64, u64) {
        (
            ALLOCATIONS.load(Ordering::Relaxed) + (self.allocations)(),
            ALLOCATED_BYTES.load(Ordering::Relaxed) + (self.bytes)(),
        )
    }
}

extern "C" fn zero() -> u64 {
    0
}

const HOST_ONLY: Counters = Counters {
    allocations: zero,
    bytes: zero,
    live: zero,
};

#[repr(C)]
struct AddArgs {
    a: u64,
    b: u64,
}

#[repr(C)]
struct TransformArgs {
    input: Bytes,
    output: BytesMut,
}

#[derive(Default)]
struct StaticProcessor {
    calls: u64,
}

#[inline(always)]
fn static_add(state: &mut StaticProcessor, a: u64, b: u64) -> u64 {
    state.calls = state.calls.wrapping_add(1);
    a.wrapping_add(b)
}

#[inline(never)]
fn noinline_add(state: &mut StaticProcessor, a: u64, b: u64) -> u64 {
    static_add(state, a, b)
}

#[inline(never)]
unsafe extern "C" fn extern_add(
    object: *mut c_void,
    arguments: *const c_void,
    output: *mut c_void,
) -> i32 {
    // SAFETY: This benchmark owns each correctly aligned argument and output.
    unsafe {
        let state = &mut *object.cast::<StaticProcessor>();
        let arguments = &*arguments.cast::<AddArgs>();
        output
            .cast::<u64>()
            .write(static_add(state, arguments.a, arguments.b));
    }
    0
}

#[inline(always)]
fn static_transform(state: &mut StaticProcessor, input: &[u8], output: &mut [u8]) -> u64 {
    assert!(output.len() >= input.len());
    for (source, target) in input.iter().zip(output.iter_mut()) {
        *target = source.to_ascii_uppercase();
    }
    state.calls = state.calls.wrapping_add(1);
    input.len() as u64
}

#[inline(never)]
fn noinline_transform(state: &mut StaticProcessor, input: &[u8], output: &mut [u8]) -> u64 {
    static_transform(state, input, output)
}

#[inline(never)]
unsafe extern "C" fn extern_transform(
    object: *mut c_void,
    arguments: *const c_void,
    output: *mut c_void,
) -> i32 {
    // SAFETY: The benchmark supplies initialized, disjoint borrowed buffers.
    unsafe {
        let state = &mut *object.cast::<StaticProcessor>();
        let arguments = &*arguments.cast::<TransformArgs>();
        let input = core::slice::from_raw_parts(arguments.input.ptr, arguments.input.len);
        let target = core::slice::from_raw_parts_mut(arguments.output.ptr, arguments.output.len);
        output
            .cast::<u64>()
            .write(static_transform(state, input, target));
    }
    0
}

fn measure(
    name: &str,
    iterations: usize,
    samples: usize,
    counters: Counters,
    mut operation: impl FnMut(),
) {
    for _ in 0..iterations.min(1024) {
        operation();
    }
    let mut ns_per_operation = Vec::with_capacity(samples);
    let before = counters.allocated();
    for _ in 0..samples {
        let start = Instant::now();
        for _ in 0..iterations {
            operation();
        }
        ns_per_operation.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    let after = counters.allocated();
    ns_per_operation.sort_unstable_by(f64::total_cmp);
    let percentile = |percent: usize| ns_per_operation[(samples - 1) * percent / 100];
    let operations = (iterations * samples) as f64;
    println!(
        "{name},{iterations},{samples},{:.3},{:.3},{:.3},{:.3},{:.3},{:.6},{:.3}",
        percentile(0),
        percentile(50),
        percentile(95),
        percentile(99),
        percentile(100),
        (after.0 - before.0) as f64 / operations,
        (after.1 - before.1) as f64 / operations,
    );
}

fn rss_kib() -> Option<u64> {
    let contents = std::fs::read_to_string("/proc/self/status").ok()?;
    contents
        .lines()
        .find(|line| line.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn print_metadata() {
    println!(
        "# zplugin benchmark; durations are ns/operation, allocations include host and plugin Rust allocators"
    );
    println!(
        "# os={},arch={}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    if let Ok(output) = std::process::Command::new("rustc").arg("-Vv").output() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            println!("# {line}");
        }
    }
    if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
        if let Some(cpu) = cpuinfo.lines().find(|line| line.starts_with("model name")) {
            println!(
                "# cpu={}",
                cpu.split_once(':').map_or(cpu, |(_, value)| value).trim()
            );
        }
    }
    println!("# profile=release,opt-level=3,lto=thin,codegen-units=1,panic=unwind");
    println!(
        "# RUSTFLAGS={}",
        std::env::var("RUSTFLAGS").unwrap_or_default()
    );
    println!(
        "# batch quantiles describe independent sample means; single-call quantiles include Instant overhead"
    );
    println!(
        "name,iterations,samples,min_ns,p50_ns,p95_ns,p99_ns,max_ns,allocations_per_op,allocated_bytes_per_op"
    );
}

#[inline(always)]
fn raw_call(call: Call, object: *mut c_void, arguments: *const c_void) -> u64 {
    let mut output = 0_u64;
    // SAFETY: All callers retain the library and object, construct the exact
    // repr(C) method arguments, and use one creating thread without reentry.
    let status = unsafe { call(object, arguments, (&mut output as *mut u64).cast()) };
    assert_eq!(status, 0);
    black_box(output)
}

#[inline(always)]
fn raw_add(call: Call, object: *mut c_void, a: u64, b: u64) -> u64 {
    let arguments = AddArgs { a, b };
    raw_call(call, object, (&arguments as *const AddArgs).cast())
}

#[inline(always)]
fn raw_transform(call: Call, object: *mut c_void, input: &[u8], output: &mut [u8]) -> u64 {
    let arguments = TransformArgs {
        input: Bytes {
            ptr: input.as_ptr(),
            len: input.len(),
        },
        output: BytesMut {
            ptr: output.as_mut_ptr(),
            len: output.len(),
        },
    };
    raw_call(call, object, (&arguments as *const TransformArgs).cast())
}

fn measure_hot(path: &Path, samples: usize) -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::temp_dir().join(format!("zplugin-bench-{}", std::process::id()));
    std::fs::create_dir(&directory)?;
    let paths = [
        directory.join("generation-a.bin"),
        directory.join("generation-b.bin"),
    ];
    for target in &paths {
        std::fs::copy(path, target)?;
    }
    // SAFETY: Independent files contain the same reviewed cooperative fixture.
    let initial = unsafe { Plugin::load_path(&paths[0], UnloadPolicy::Cooperative)? };
    let mut generations = zplugin::hot::Generations::new(initial, 2);
    println!(
        "# hot rows count host allocations; copied plugin allocators are independent of the fixed-plugin counters"
    );
    measure("hot/request-lease", 200_000, samples, HOST_ONLY, || {
        black_box(generations.current());
    });
    {
        let owner = generations.current();
        let mut proxy = DemoApi::wrap(Plugin::create_owned(&owner, DemoApi::SPEC)?)?;
        measure(
            "hot/session-reused-call",
            200_000,
            samples,
            HOST_ONLY,
            || {
                black_box(proxy.add(black_box(19), black_box(23)).unwrap());
            },
        );
    }
    let mut index = 0;
    measure(
        "hot/switch-load-bind-create-collect",
        20,
        samples,
        HOST_ONLY,
        || {
            index = 1 - index;
            // SAFETY: Both paths are copies of the reviewed cooperative fixture.
            let next =
                unsafe { Plugin::load_path(&paths[index], UnloadPolicy::Cooperative).unwrap() };
            let proxy = generations
                .replace(next, |owner| {
                    DemoApi::wrap(Plugin::create_owned(owner, DemoApi::SPEC)?)
                })
                .unwrap();
            black_box(proxy);
            generations.collect();
        },
    );
    assert_eq!(generations.retired_count(), 0);
    drop(generations);
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: zplugin-bench <plugin-path>")?;
    let path = Path::new(&path);
    let samples = 41;
    print_metadata();
    let rss_before = rss_kib();
    let first_start = Instant::now();
    // SAFETY: The benchmark loads the reviewed, cooperative example plugin.
    let plugin = unsafe { Plugin::load_path(path, UnloadPolicy::Cooperative)? };
    let first_load_ns = first_start.elapsed().as_nanos();
    let rss_after = rss_kib();
    println!(
        "# first_load_ns={first_load_ns},rss_before_kib={rss_before:?},rss_after_kib={rss_after:?}"
    );
    drop(plugin);
    measure("lifecycle/remap-load-drop", 100, samples, HOST_ONLY, || {
        // SAFETY: No objects or other handles retain this cooperative fixture.
        black_box(unsafe { Plugin::load_path(path, UnloadPolicy::Cooperative).unwrap() });
    });
    // SAFETY: Load the same reviewed plugin for the remaining comparisons.
    let plugin = unsafe { Plugin::load_path(path, UnloadPolicy::Cooperative)? };
    // SAFETY: The fixture is trusted; symbols below have the exact published
    // benchmark signatures and libloading remains alive until every call ends.
    let baseline_library = unsafe { libloading::Library::new(path)? };
    let (libloading_add, libloading_transform, counters): (Call, Call, Counters) = unsafe {
        (
            **baseline_library.get::<*const Call>(b"demo_add_function\0")?,
            **baseline_library.get::<*const Call>(b"demo_transform_function\0")?,
            Counters {
                allocations: *baseline_library.get(b"demo_allocations\0")?,
                bytes: *baseline_library.get(b"demo_allocated_bytes\0")?,
                live: *baseline_library.get(b"demo_live_bytes\0")?,
            },
        )
    };
    measure("lifecycle/warm-load-drop", 100, samples, HOST_ONLY, || {
        // SAFETY: The cooperative fixture has no active objects yet.
        black_box(unsafe { Plugin::load_path(path, UnloadPolicy::Cooperative).unwrap() });
    });
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let absolute = std::fs::canonicalize(path)?;
        let encoded = std::ffi::CString::new(absolute.as_os_str().as_bytes())?;
        measure(
            "lifecycle/core-warm-load-drop",
            100,
            samples,
            HOST_ONLY,
            || {
                // SAFETY: The reviewed fixture has no objects; path storage is reused.
                black_box(unsafe { Plugin::load_cooperative(&encoded).unwrap() });
            },
        );
    }
    let binding = plugin.bind(DemoApi::SPEC)?;
    let cached_add = binding.method(0);
    let cached_transform = binding.method(1);
    let host_before_object = LIVE_BYTES.load(Ordering::Relaxed);
    let plugin_before_object = (counters.live)();
    let instance = binding.create()?;
    println!(
        "# instance_host_bytes={},instance_plugin_bytes={}",
        LIVE_BYTES.load(Ordering::Relaxed) - host_before_object,
        (counters.live)() - plugin_before_object
    );
    let object = instance.handle();
    let mut proxy = DemoApi::create(&plugin)?;
    let mut objects = Vec::with_capacity(1024);
    let host_before_many = LIVE_BYTES.load(Ordering::Relaxed);
    let plugin_before_many = (counters.live)();
    for _ in 0..1024 {
        objects.push(binding.create()?);
    }
    println!(
        "# 1024_instances_host_extra_bytes={},1024_instances_plugin_extra_bytes={}",
        LIVE_BYTES.load(Ordering::Relaxed) - host_before_many,
        (counters.live)() - plugin_before_many
    );
    drop(objects);
    assert_eq!((counters.live)(), plugin_before_many);
    let mut state = StaticProcessor::default();
    let args = AddArgs { a: 19, b: 23 };
    let bare_call: Call = black_box(extern_add);
    measure("scalar/static-inline", 200_000, samples, counters, || {
        black_box(static_add(
            black_box(&mut state),
            black_box(args.a),
            black_box(args.b),
        ));
    });
    measure("scalar/static-noinline", 200_000, samples, counters, || {
        black_box(noinline_add(
            black_box(&mut state),
            black_box(args.a),
            black_box(args.b),
        ));
    });
    measure("scalar/cached-extern-C", 200_000, samples, counters, || {
        raw_add(
            bare_call,
            (&mut state as *mut StaticProcessor).cast(),
            black_box(args.a),
            black_box(args.b),
        );
    });
    measure(
        "scalar/cached-generated-adapter",
        200_000,
        samples,
        counters,
        || {
            raw_add(cached_add, object, black_box(args.a), black_box(args.b));
        },
    );
    measure(
        "scalar/libloading-cached",
        200_000,
        samples,
        counters,
        || {
            raw_add(libloading_add, object, black_box(args.a), black_box(args.b));
        },
    );
    measure("scalar/zplugin-proxy", 200_000, samples, counters, || {
        black_box(proxy.add(black_box(args.a), black_box(args.b)).unwrap());
    });
    measure("latency/timer-floor", 1, 20_001, counters, || {
        black_box(());
    });
    measure("latency/zplugin-proxy", 1, 20_001, counters, || {
        black_box(proxy.add(black_box(args.a), black_box(args.b)).unwrap());
    });
    measure(
        "lifecycle/interface-bind",
        10_000,
        samples,
        counters,
        || {
            black_box(plugin.bind(black_box(DemoApi::SPEC)).unwrap());
        },
    );
    measure("lifecycle/create-drop", 10_000, samples, counters, || {
        black_box(binding.create().unwrap());
    });
    measure(
        "lifecycle/owned-64B-create-release",
        10_000,
        samples,
        counters,
        || {
            black_box(proxy.owned(black_box(&[42_u8; 64])).unwrap());
        },
    );

    for length in [64, 4096, 1024 * 1024] {
        let input = vec![b'a'; length];
        let mut output = vec![0_u8; length];
        let iterations = match length {
            64 => 100_000,
            4096 => 4_000,
            _ => 16,
        };
        measure(
            &format!("bytes-{length}/static-inline"),
            iterations,
            samples,
            counters,
            || {
                black_box(static_transform(
                    black_box(&mut state),
                    black_box(&input),
                    black_box(&mut output),
                ));
            },
        );
        measure(
            &format!("bytes-{length}/static-noinline"),
            iterations,
            samples,
            counters,
            || {
                black_box(noinline_transform(
                    black_box(&mut state),
                    black_box(&input),
                    black_box(&mut output),
                ));
            },
        );
        let bare_transform: Call = black_box(extern_transform);
        measure(
            &format!("bytes-{length}/cached-extern-C"),
            iterations,
            samples,
            counters,
            || {
                raw_transform(
                    bare_transform,
                    (&mut state as *mut StaticProcessor).cast(),
                    black_box(&input),
                    black_box(&mut output),
                );
            },
        );
        measure(
            &format!("bytes-{length}/cached-generated-adapter"),
            iterations,
            samples,
            counters,
            || {
                raw_transform(
                    cached_transform,
                    object,
                    black_box(&input),
                    black_box(&mut output),
                );
            },
        );
        measure(
            &format!("bytes-{length}/libloading-cached"),
            iterations,
            samples,
            counters,
            || {
                raw_transform(
                    libloading_transform,
                    object,
                    black_box(&input),
                    black_box(&mut output),
                );
            },
        );
        measure(
            &format!("bytes-{length}/zplugin-proxy"),
            iterations,
            samples,
            counters,
            || {
                black_box(
                    proxy
                        .transform(black_box(&input), black_box(&mut output))
                        .unwrap(),
                );
            },
        );
        assert!(output.iter().all(|&value| value == b'A'));
        black_box(&output);
    }
    println!(
        "# host_live_bytes={},plugin_live_bytes={},rss_final_kib={:?},static_calls={}",
        LIVE_BYTES.load(Ordering::Relaxed),
        (counters.live)(),
        rss_kib(),
        black_box(state.calls)
    );
    drop(proxy);
    drop(instance);
    drop(baseline_library);
    drop(plugin);
    measure_hot(path, samples)?;
    Ok(())
}
