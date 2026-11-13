//! Wasm sandbox measurements; sample quantiles describe batch averages, not calls.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    env,
    hint::black_box,
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
use wasmi::{
    CompilationMode, Config, EnforcedLimits, Engine, ExternType, Instance, Memory, Module, Store,
    StoreLimits, StoreLimitsBuilder, TypedFunc,
};
use zplugin::wasm::{Interface, Limits, Plugin};

struct Counted;
static CALLS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);

fn account(size: usize) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    BYTES.fetch_add(size as u64, Ordering::Relaxed);
    let live = LIVE.fetch_add(size as u64, Ordering::Relaxed) + size as u64;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: Each allocation and deallocation is forwarded unchanged to System;
// counters inspect only the supplied layout, never allocated memory.
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller's layout is forwarded to the backing allocator.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Preserve System's zero-initialization and allocation contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            account(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: Forward the live pointer with its original allocation layout.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: The original allocation/layout and requested new size are kept.
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if !replacement.is_null() {
            LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
            account(size);
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: Counted = Counted;

const INTERFACE: Interface = Interface {
    id: 0x7a70_6c75_6769_6e01,
    version: 1,
};

/// A benchmark-only Wasmi baseline. It owns its Store and uses the same fuel,
/// memory/table/stack limits, buffer protocol and one-thread policy as zplugin.
/// Binding assertions fail the benchmark; this is not an alternative public API.
struct CachedWasmi {
    store: Store<StoreLimits>,
    memory: Memory,
    call: TypedFunc<(i32, i32, i32), i64>,
    input: usize,
    output: usize,
    input_capacity: usize,
    output_capacity: usize,
    fuel: u64,
    _local: PhantomData<*mut ()>,
}

impl CachedWasmi {
    fn new(bytes: &[u8]) -> Self {
        let limits = Limits::default();
        assert!(bytes.len() <= limits.max_module_bytes);
        assert!(bytes.starts_with(b"\0asm\x01\0\0\0"));
        let mut config = Config::default();
        config
            .consume_fuel(true)
            .enforced_limits(EnforcedLimits::strict())
            .allow_start_fn(false)
            .ignore_custom_sections(true)
            .compilation_mode(CompilationMode::Eager)
            .wasm_multi_memory(false)
            .wasm_custom_page_sizes(false)
            .set_min_stack_height(1024)
            .set_max_stack_height(limits.max_stack_bytes)
            .set_max_recursion_depth(limits.max_call_depth)
            .set_max_cached_stacks(1);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes).unwrap();
        assert!(module.imports().next().is_none());
        assert!(matches!(module.get_export("memory"), Some(ExternType::Memory(ty)) if !ty.is_64()));
        let store_limits = StoreLimitsBuilder::new()
            .memory_size(limits.max_memory_bytes)
            .memories(1)
            .tables(1)
            .table_elements(limits.max_table_elements)
            .instances(1)
            .build();
        let mut store = Store::new(&engine, store_limits);
        store.limiter(|limits| limits);
        store.set_fuel(limits.fuel_per_call).unwrap();
        let instance = Instance::new(&mut store, &module, &[]).unwrap();
        let memory = instance.get_memory(&store, "memory").unwrap();
        let info = instance
            .get_typed_func::<i32, i64>(&store, "zplugin_wasm_info_v1")
            .unwrap();
        let call = instance
            .get_typed_func::<(i32, i32, i32), i64>(&store, "zplugin_wasm_call_v1")
            .unwrap();
        let mut query = |field| info.call(&mut store, field).unwrap() as u64;
        assert_eq!(query(0), 1);
        assert_eq!(query(1), INTERFACE.id);
        assert_eq!(query(2), u64::from(INTERFACE.version));
        let input = u32::try_from(query(3)).unwrap() as usize;
        let input_capacity = u32::try_from(query(4)).unwrap() as usize;
        let output = u32::try_from(query(5)).unwrap() as usize;
        let output_capacity = u32::try_from(query(6)).unwrap() as usize;
        let memory_size = memory.data_size(&store);
        let input_end = input.checked_add(input_capacity).unwrap();
        let output_end = output.checked_add(output_capacity).unwrap();
        assert!(input_capacity <= limits.max_input_bytes && input_end <= memory_size);
        assert!(output_capacity <= limits.max_output_bytes && output_end <= memory_size);
        assert!(input_end <= output || output_end <= input);
        Self {
            store,
            memory,
            call,
            input,
            output,
            input_capacity,
            output_capacity,
            fuel: limits.fuel_per_call,
            _local: PhantomData,
        }
    }

    fn call(&mut self, operation: u32, input: &[u8], output: &mut [u8]) -> usize {
        assert!(input.len() <= self.input_capacity);
        let capacity = output.len().min(self.output_capacity);
        self.store.set_fuel(self.fuel).unwrap();
        self.memory
            .write(&mut self.store, self.input, input)
            .unwrap();
        self.memory.data_mut(&mut self.store)[self.output..self.output + capacity].fill(0);
        let result = self
            .call
            .call(
                &mut self.store,
                (operation as i32, input.len() as i32, capacity as i32),
            )
            .unwrap() as u64;
        assert_eq!(result as i32, 0);
        let written = (result >> 32) as usize;
        assert!(written <= capacity);
        self.memory
            .read(&self.store, self.output, &mut output[..written])
            .unwrap();
        written
    }
}

struct Sample {
    elapsed_ns: u128,
    allocations: u64,
    bytes: u64,
    net_live: i128,
    peak_extra: u64,
}

fn measure(
    name: &str,
    iterations: usize,
    samples: usize,
    guest_memory: usize,
    mut operation: impl FnMut(),
) {
    for _ in 0..iterations.min(64) {
        operation();
    }
    let mut results = Vec::with_capacity(samples);
    for _ in 0..samples {
        let calls = CALLS.load(Ordering::Relaxed);
        let bytes = BYTES.load(Ordering::Relaxed);
        let live = LIVE.load(Ordering::Relaxed);
        PEAK.store(live, Ordering::Relaxed);
        let start = Instant::now();
        for _ in 0..iterations {
            operation();
        }
        let elapsed_ns = start.elapsed().as_nanos();
        let allocations = CALLS.load(Ordering::Relaxed) - calls;
        let bytes = BYTES.load(Ordering::Relaxed) - bytes;
        let net_live = i128::from(LIVE.load(Ordering::Relaxed)) - i128::from(live);
        let peak_extra = PEAK.load(Ordering::Relaxed).saturating_sub(live);
        results.push(Sample {
            elapsed_ns,
            allocations,
            bytes,
            net_live,
            peak_extra,
        });
    }
    for (index, sample) in results.iter().enumerate() {
        println!(
            "{name},{},{iterations},{},{:.3},{},{},{},{},{guest_memory}",
            index + 1,
            sample.elapsed_ns,
            sample.elapsed_ns as f64 / iterations as f64,
            sample.allocations,
            sample.bytes,
            sample.net_live,
            sample.peak_extra,
        );
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args_os()
        .nth(1)
        .ok_or("usage: wasm_bench PATH.wasm [--smoke]")?;
    let smoke = env::args_os().any(|argument| argument == "--smoke");
    let bytes = std::fs::read(path)?;
    let samples = if smoke { 3 } else { 31 };
    let live = LIVE.load(Ordering::Relaxed);
    let temporary = Plugin::new(&bytes, INTERFACE, Limits::default())?;
    let guest_memory = temporary.memory_size();
    let instance_live = LIVE.load(Ordering::Relaxed) - live;
    drop(temporary);
    assert_eq!(LIVE.load(Ordering::Relaxed), live);
    println!("# quantiles=batch_mean_ns_per_operation; not individual-call latency percentiles");
    println!(
        "# allocations=Rust_global_allocator; excludes allocator metadata and OS memory; realloc counts requested new size"
    );
    println!(
        "# baseline=cached Wasmi typed call with matching fuel, limits, zero-fill and byte copies; errors abort benchmark, zplugin adds recoverable errors/poisoning"
    );
    println!(
        "# guest_memory_bytes={guest_memory}; zplugin_instance_live_rust_bytes={instance_live}"
    );
    println!(
        "name,sample,iterations,elapsed_ns,ns_per_op,allocations,allocated_bytes,net_live_bytes,peak_extra_bytes,guest_memory_bytes"
    );
    let lifecycle_iterations = if smoke { 1 } else { 10 };
    measure(
        "zplugin_load_validate_bind_drop",
        lifecycle_iterations,
        samples,
        guest_memory,
        || {
            let plugin = Plugin::new(black_box(&bytes), INTERFACE, Limits::default()).unwrap();
            black_box(plugin.memory_size());
            drop(plugin);
        },
    );
    measure(
        "cached_wasmi_load_validate_bind_drop",
        lifecycle_iterations,
        samples,
        guest_memory,
        || {
            let plugin = CachedWasmi::new(black_box(&bytes));
            black_box(plugin.memory.data_size(&plugin.store));
            drop(plugin);
        },
    );
    for size in [0, 64, 4096, 1_048_576] {
        let iterations = if smoke {
            3
        } else {
            match size {
                0 | 64 => 10_000,
                4096 => 1000,
                _ => 16,
            }
        };
        let input: Vec<_> = (0..size).map(|index| (index % 251) as u8).collect();
        let mut output = vec![0xa5; size];
        let mut plugin = Plugin::new(&bytes, INTERFACE, Limits::default())?;
        measure(
            &format!("zplugin_echo_{size}"),
            iterations,
            samples,
            guest_memory,
            || {
                let written = plugin
                    .call(black_box(0), black_box(&input), black_box(&mut output))
                    .unwrap();
                assert_eq!(black_box(written), size);
                black_box(&output);
            },
        );
        assert_eq!(output, input);
        drop(plugin);
        output.fill(0xa5);
        let mut cached = CachedWasmi::new(&bytes);
        measure(
            &format!("cached_wasmi_echo_{size}"),
            iterations,
            samples,
            guest_memory,
            || {
                let written = cached.call(black_box(0), black_box(&input), black_box(&mut output));
                assert_eq!(black_box(written), size);
                black_box(&output);
            },
        );
        assert_eq!(output, input);
    }
    let iterations = if smoke { 3 } else { 10_000 };
    let expected_count = (iterations.min(64) + samples * iterations) as u64;
    let mut output = [0; 8];
    let mut plugin = Plugin::new(&bytes, INTERFACE, Limits::default())?;
    measure(
        "zplugin_counter_8",
        iterations,
        samples,
        guest_memory,
        || {
            assert_eq!(
                plugin
                    .call(black_box(2), &[], black_box(&mut output))
                    .unwrap(),
                8
            );
            black_box(&output);
        },
    );
    assert_eq!(u64::from_le_bytes(output), expected_count);
    drop(plugin);
    let mut cached = CachedWasmi::new(&bytes);
    measure(
        "cached_wasmi_counter_8",
        iterations,
        samples,
        guest_memory,
        || {
            assert_eq!(cached.call(black_box(2), &[], black_box(&mut output)), 8);
            black_box(&output);
        },
    );
    assert_eq!(u64::from_le_bytes(output), expected_count);
    Ok(())
}
