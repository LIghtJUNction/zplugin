//! Cross-library task measurements. Quantiles describe batch means, not calls.
use async_interface::{AsyncServiceApi, READY, WORKER};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    ffi::c_void,
    future::Future,
    hint::black_box,
    mem::MaybeUninit,
    path::Path,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};
use zplugin::{Instance, OwnedTask, Plugin, UnloadPolicy, abi};

struct Allocator;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The original layout and allocator result are preserved.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            LIVE.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: Forward the original live allocation and its layout.
        unsafe { System.dealloc(ptr, layout) };
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[derive(Clone, Copy)]
struct Counters {
    allocations: extern "C" fn() -> u64,
    bytes: extern "C" fn() -> u64,
    live: extern "C" fn() -> u64,
}
impl Counters {
    fn snapshot(self) -> [u64; 6] {
        [
            ALLOCS.load(Ordering::Relaxed),
            BYTES.load(Ordering::Relaxed),
            LIVE.load(Ordering::Relaxed),
            (self.allocations)(),
            (self.bytes)(),
            (self.live)(),
        ]
    }
}

fn measure(
    name: &str,
    iterations: usize,
    samples: usize,
    counters: Counters,
    mut op: impl FnMut(),
) {
    for _ in 0..iterations.min(32) {
        op();
    }
    let mut values = Vec::with_capacity(samples);
    let before = counters.snapshot();
    for _ in 0..samples {
        let start = Instant::now();
        for _ in 0..iterations {
            op();
        }
        values.push(start.elapsed().as_secs_f64() * 1e9 / iterations as f64);
    }
    let after = counters.snapshot();
    values.sort_unstable_by(f64::total_cmp);
    let percentile = |p| values[(samples - 1) * p / 100];
    let count = (samples * iterations) as f64;
    let delta = |i| (after[i] as i128 - before[i] as i128) as f64;
    println!(
        "{name},{iterations},{samples},{:.3},{:.3},{:.3},{:.3},{:.3},{:.6},{:.6},{:.6},{:.3},{:.3},{:.0},{:.0}",
        percentile(0),
        percentile(50),
        percentile(95),
        percentile(99),
        percentile(100),
        delta(0) / count,
        delta(3) / count,
        (delta(0) + delta(3)) / count,
        delta(1) / count,
        delta(4) / count,
        delta(2),
        delta(5)
    );
}

// This stateless C waker lives in the executable for the complete process.
// Every retained reference remains valid; no counter is required for static data.
unsafe extern "C" fn noop(_: *const c_void) -> i32 {
    0
}
static WAKER_TABLE: abi::WakerVTable = abi::WakerVTable {
    size: core::mem::size_of::<abi::WakerVTable>() as u32,
    abi_version: abi::EXTENSION_VERSION,
    retain: Some(noop),
    wake: Some(noop),
    release: Some(noop),
};
static CONTEXTS: [u8; 2] = [0, 1];
fn c_waker(index: usize) -> abi::RawWaker {
    abi::RawWaker {
        context: (&CONTEXTS[index] as *const u8).cast(),
        vtable: &WAKER_TABLE,
    }
}
struct WakeCounter(AtomicU64);
impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[repr(C)]
struct StartArgs {
    mode: u32,
    value: u64,
}
struct CachedTask<'a> {
    raw: abi::RawTask,
    poll: abi::TaskPoll,
    // Identical ownership/thread rule to the proxy: retain the original library.
    _owner: &'a Instance<'a>,
}
impl<'a> CachedTask<'a> {
    fn new(owner: &'a Instance<'a>) -> Self {
        let args = StartArgs {
            mode: WORKER,
            value: 42,
        };
        let mut out = MaybeUninit::<abi::RawTask>::uninit();
        // SAFETY: Exact repr(C) generated method layout; the validated binding
        // and its single-threaded object remain live through all calls/release.
        let status = unsafe {
            owner.method(0)(
                owner.handle(),
                (&args as *const StartArgs).cast(),
                out.as_mut_ptr().cast(),
            )
        };
        assert_eq!(status, 0);
        let raw = unsafe { OwnedTask::<u64>::from_raw(out.assume_init()).unwrap() }.into_raw();
        let poll = unsafe { (*raw.vtable).poll.unwrap() };
        Self {
            raw,
            poll,
            _owner: owner,
        }
    }
    fn pending(&mut self, waker: &abi::RawWaker) {
        let mut value = MaybeUninit::<u64>::uninit();
        let mut state = abi::TASK_PENDING;
        // SAFETY: Validated task/schema and static thread-safe C waker, exclusive poll.
        let status = unsafe {
            (self.poll)(
                self.raw.context,
                waker,
                value.as_mut_ptr().cast(),
                &mut state,
            )
        };
        assert_eq!(black_box((status, state)), (0, abi::TASK_PENDING));
    }
}
impl Drop for CachedTask<'_> {
    fn drop(&mut self) {
        // SAFETY: This uniquely-owned handle is released before its owner/library.
        assert_eq!(
            unsafe { ((*self.raw.vtable).release.unwrap())(self.raw.context) },
            0
        );
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: extensions PLUGIN [--smoke]")?;
    let smoke = std::env::args().any(|s| s == "--smoke");
    let samples = if smoke { 3 } else { 101 };
    let iterations = if smoke { 100 } else { 10_000 };
    let lifecycle = if smoke { 2 } else { 1000 };
    // SAFETY: Only the reviewed example plugin is accepted by the driver.
    let plugin = unsafe { Plugin::load_path(Path::new(&path), UnloadPolicy::Cooperative)? };
    // Benchmark-only symbol lookup; both loaders stay live until all tasks exit.
    let library = unsafe { libloading::Library::new(&path)? };
    let counters = unsafe {
        Counters {
            allocations: *library.get(b"demo_allocations\0")?,
            bytes: *library.get(b"demo_allocated_bytes\0")?,
            live: *library.get(b"demo_live_bytes\0")?,
        }
    };
    let mut service = AsyncServiceApi::create(&plugin)?;
    let wakers = [c_waker(0), c_waker(1)];
    let rust_wakers = [
        Waker::from(Arc::new(WakeCounter(AtomicU64::new(0)))),
        Waker::from(Arc::new(WakeCounter(AtomicU64::new(0)))),
    ];
    println!(
        "# quantiles=batch_means_ns_per_op; allocations=Rust_global_allocators_only; worker rows include OS scheduling but not OS allocation accounting"
    );
    println!(
        "# warm polls use a pending worker blocked on a condition variable; no busy-worker contention; task/waker setup is outside timing"
    );
    println!(
        "# raw and proxy C-waker rows share stateless static C wakers and borrowed library lifetimes; Future rows add the Rust-waker bridge"
    );
    println!(
        "name,iterations,samples,min_ns,p50_ns,p95_ns,p99_ns,max_ns,host_allocations_per_op,plugin_allocations_per_op,total_allocations_per_op,host_bytes_per_op,plugin_bytes_per_op,host_live_delta_bytes,plugin_live_delta_bytes"
    );
    measure("ready/create-drop", lifecycle, samples, counters, || {
        drop(black_box(service.start(READY, black_box(42)).unwrap()));
    });
    measure("ready/c-poll-drop", lifecycle, samples, counters, || {
        let mut task = service.start(READY, black_box(42)).unwrap();
        assert_eq!(unsafe { task.poll_raw(&wakers[0]) }, Poll::Ready(Ok(42)));
    });
    measure(
        "ready/future-poll-drop",
        lifecycle,
        samples,
        counters,
        || {
            let mut task = service.start(READY, black_box(42)).unwrap();
            assert_eq!(
                Pin::new(&mut task).poll(&mut Context::from_waker(&rust_wakers[0])),
                Poll::Ready(Ok(42))
            );
        },
    );
    let owner = plugin.bind(AsyncServiceApi::SPEC)?.create()?;
    {
        let mut task = CachedTask::new(&owner);
        for _ in 0..32_000 {
            task.pending(&wakers[0]);
        }
        std::thread::sleep(Duration::from_millis(2));
        measure(
            "pending/cached-c-poll",
            iterations,
            samples,
            counters,
            || task.pending(black_box(&wakers[0])),
        );
    }
    {
        let mut task = service.start(WORKER, 42)?;
        for _ in 0..32_000 {
            assert_eq!(unsafe { task.poll_raw(&wakers[0]) }, Poll::Pending);
        }
        std::thread::sleep(Duration::from_millis(2));
        measure(
            "pending/proxy-c-poll",
            iterations,
            samples,
            counters,
            || {
                assert_eq!(
                    black_box(unsafe { task.poll_raw(black_box(&wakers[0])) }),
                    Poll::Pending
                );
            },
        );
        let mut index = 0;
        measure(
            "pending/proxy-c-replace-waker",
            iterations,
            samples,
            counters,
            || {
                index ^= 1;
                assert_eq!(
                    black_box(unsafe { task.poll_raw(black_box(&wakers[index])) }),
                    Poll::Pending
                );
            },
        );
    }
    {
        let mut task = service.start(WORKER, 42)?;
        for _ in 0..32_000 {
            assert_eq!(
                Pin::new(&mut task).poll(&mut Context::from_waker(&rust_wakers[0])),
                Poll::Pending
            );
        }
        std::thread::sleep(Duration::from_millis(2));
        measure("pending/future-poll", iterations, samples, counters, || {
            assert_eq!(
                black_box(
                    Pin::new(&mut task).poll(&mut Context::from_waker(black_box(&rust_wakers[0])))
                ),
                Poll::Pending
            );
        });
        let mut index = 0;
        measure(
            "pending/future-replace-waker",
            iterations,
            samples,
            counters,
            || {
                index ^= 1;
                assert_eq!(
                    black_box(
                        Pin::new(&mut task)
                            .poll(&mut Context::from_waker(black_box(&rust_wakers[index])))
                    ),
                    Poll::Pending
                );
            },
        );
    }
    measure(
        "worker/create-cancel-join-drop",
        2,
        samples,
        counters,
        || {
            let mut task = service.start(WORKER, black_box(42)).unwrap();
            assert_eq!(unsafe { task.poll_raw(&wakers[0]) }, Poll::Pending);
            task.cancel().unwrap();
            drop(task);
            assert_eq!(service.workers().unwrap(), 0);
        },
    );
    measure(
        "worker/create-signal-complete-join-drop",
        2,
        samples,
        counters,
        || {
            let mut task = service.start(WORKER, black_box(42)).unwrap();
            assert_eq!(unsafe { task.poll_raw(&wakers[0]) }, Poll::Pending);
            service.signal().unwrap();
            loop {
                match unsafe { task.poll_raw(&wakers[0]) } {
                    Poll::Ready(result) => {
                        assert_eq!(result, Ok(42));
                        break;
                    }
                    Poll::Pending => std::thread::yield_now(),
                }
            }
            drop(task);
            assert_eq!(service.workers().unwrap(), 0);
        },
    );
    drop(owner);
    drop(service);
    assert_eq!(
        (counters.live)(),
        0,
        "all plugin task, waker and service resources must be released"
    );
    plugin.try_close().map_err(|(error, _)| error)?;
    drop(library);
    Ok(())
}
