#![cfg(all(feature = "macros", feature = "std"))]

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, RawWaker, RawWakerVTable, Wake, Waker},
};
use zplugin::{Error, OwnedTask, Plugin, Task, abi};

#[zplugin::interface(id = "tests.waker.failures", version = 1)]
trait Service {
    fn start(&mut self, mode: u32) -> zplugin::Result<zplugin::OwnedTask<u64>>;
    fn polls(&self) -> zplugin::Result<u64>;
}

struct Factory {
    polls: Arc<AtomicUsize>,
}

impl Service for Factory {
    fn start(&mut self, mode: u32) -> zplugin::Result<zplugin::OwnedTask<u64>> {
        let polls = Arc::clone(&self.polls);
        Ok(OwnedTask::new(std::future::poll_fn(move |cx| {
            polls.fetch_add(1, Ordering::SeqCst);
            if mode != 0 {
                cx.waker().wake_by_ref();
            }
            if mode == 1 {
                Poll::Pending
            } else {
                Poll::Ready(Ok(42))
            }
        })))
    }

    fn polls(&self) -> zplugin::Result<u64> {
        Ok(self.polls.load(Ordering::SeqCst) as u64)
    }
}

// SAFETY: no background work or registrations exist. Every future owns its
// counter; zplugin retains the callback bridge through its final release.
unsafe impl zplugin::export::UnloadSafe for Factory {}

fn create() -> zplugin::Result<Factory> {
    Ok(Factory {
        polls: Arc::new(AtomicUsize::new(0)),
    })
}

zplugin::export!(
    ServiceExport<Factory>,
    constructor = create,
    version = (1, 0, 0),
    unload = cooperative
);
unsafe extern "C" {
    fn zplugin_entry_v1() -> *const abi::Header;
}

fn plugin() -> Plugin {
    // SAFETY: this entry and all referenced code/data are in the test executable.
    unsafe { Plugin::from_static(&*zplugin_entry_v1()) }.unwrap()
}

fn poll(task: &mut Task<'_, u64>, waker: &Waker) -> Poll<zplugin::Result<u64>> {
    Pin::new(task).poll(&mut Context::from_waker(waker))
}

unsafe fn panic_clone(_: *const ()) -> RawWaker {
    panic!("executor waker clone panic");
}
unsafe fn no_op(_: *const ()) {}
static PANIC_CLONE: RawWakerVTable = RawWakerVTable::new(panic_clone, no_op, no_op, no_op);

#[test]
fn executor_clone_panic_is_contained_and_latches_task_poison() {
    let plugin = plugin();
    let mut factory = ServiceApi::create(&plugin).unwrap();
    let mut task = factory.start(0).unwrap();
    // SAFETY: this stateless waker never dereferences its null data; all functions
    // are thread-safe. clone deliberately panics before acquiring a new owner.
    let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &PANIC_CLONE)) };
    assert_eq!(poll(&mut task, &waker), Poll::Ready(Err(Error::Panicked)));
    assert_eq!(factory.polls(), Ok(0));
    assert_eq!(poll(&mut task, &waker), Poll::Ready(Err(Error::Poisoned)));
    assert_eq!(factory.polls(), Ok(0));
    drop(task);
    // The failed task is independent of its still-usable factory.
    assert!(factory.start(0).is_ok());
}

struct PanicWake(AtomicUsize);
impl Wake for PanicWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("executor wake_by_ref panic");
    }
}

fn check_reverse_panic(mode: u32) {
    let plugin = plugin();
    let mut factory = ServiceApi::create(&plugin).unwrap();
    let mut task = factory.start(mode).unwrap();
    let state = Arc::new(PanicWake(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&state));
    // A plugin-side poll invokes the foreign C waker, which calls an executor
    // waker that panics. No unwind crosses either native ABI direction.
    assert_eq!(poll(&mut task, &waker), Poll::Ready(Err(Error::Panicked)));
    assert_eq!(factory.polls(), Ok(1));
    assert_eq!(state.0.load(Ordering::SeqCst), 1);
    assert_eq!(poll(&mut task, &waker), Poll::Ready(Err(Error::Poisoned)));
    assert_eq!(factory.polls(), Ok(1));
    assert_eq!(state.0.load(Ordering::SeqCst), 1);
    drop(task);
}

#[test]
fn reverse_wake_panic_is_contained_when_future_returns_pending() {
    check_reverse_panic(1);
}

#[test]
fn reverse_wake_panic_is_contained_when_future_returns_ready() {
    check_reverse_panic(2);
}
