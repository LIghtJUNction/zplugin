//! Protocol tests use real initialized storage, including negative size prefixes.
use core::{ffi::c_void, mem::size_of, task::Poll};
use zplugin::{AbiValue, Error, OwnedTask, abi};

unsafe extern "C" fn noop(_: *const c_void) -> i32 {
    0
}
static WAKE_TABLE: abi::WakerVTable = abi::WakerVTable {
    size: size_of::<abi::WakerVTable>() as u32,
    abi_version: 1,
    retain: Some(noop),
    wake: Some(noop),
    release: Some(noop),
};
fn raw_waker() -> abi::RawWaker {
    abi::RawWaker {
        context: core::ptr::null(),
        vtable: &WAKE_TABLE,
    }
}

#[test]
fn checks_table_prefix_and_output_contract() {
    let short: u32 = 4;
    let raw = abi::RawTask {
        context: core::ptr::NonNull::<u8>::dangling().as_ptr().cast(),
        vtable: (&short as *const u32).cast(),
    };
    assert!(matches!(
        unsafe { OwnedTask::<u64>::from_raw(raw) },
        Err(Error::DescriptorTooShort { actual: 4, .. })
    ));
    unsafe extern "C" fn poll(
        _: *mut c_void,
        _: *const abi::RawWaker,
        _: *mut c_void,
        _: *mut u32,
    ) -> i32 {
        -1
    }
    unsafe extern "C" fn drop_task(_: *mut c_void) -> i32 {
        0
    }
    let table = abi::TaskVTable {
        size: size_of::<abi::TaskVTable>() as u32,
        abi_version: 1,
        output_size: 8,
        output_align: 8,
        output_schema: abi::Bytes::from_static(b"u32"),
        poll: Some(poll),
        cancel: Some(drop_task),
        release: Some(drop_task),
    };
    let raw = abi::RawTask {
        context: core::ptr::NonNull::<u8>::dangling().as_ptr().cast(),
        vtable: &table,
    };
    assert!(matches!(
        unsafe { OwnedTask::<u64>::from_raw(raw) },
        Err(Error::SignatureMismatch { .. })
    ));
}

#[test]
fn manual_core_task_checks_pending_state_and_owned_release() {
    struct State {
        polls: usize,
        releases: usize,
    }
    unsafe extern "C" fn poll(
        ptr: *mut c_void,
        _: *const abi::RawWaker,
        out: *mut c_void,
        state: *mut u32,
    ) -> i32 {
        let value = unsafe { &mut *ptr.cast::<State>() };
        value.polls += 1;
        unsafe {
            state.write(if value.polls == 1 {
                abi::TASK_PENDING
            } else {
                abi::TASK_READY
            });
        }
        if value.polls == 2 {
            unsafe {
                out.cast::<u64>().write(42);
            }
        }
        0
    }
    unsafe extern "C" fn cancel(_: *mut c_void) -> i32 {
        0
    }
    unsafe extern "C" fn release(ptr: *mut c_void) -> i32 {
        unsafe {
            (*ptr.cast::<State>()).releases += 1;
        }
        0
    }
    let table = abi::TaskVTable {
        size: size_of::<abi::TaskVTable>() as u32,
        abi_version: 1,
        output_size: 8,
        output_align: core::mem::align_of::<u64>(),
        output_schema: abi::Bytes::from_static(u64::SCHEMA.as_bytes()),
        poll: Some(poll),
        cancel: Some(cancel),
        release: Some(release),
    };
    let mut state = State {
        polls: 0,
        releases: 0,
    };
    let mut task = unsafe {
        OwnedTask::<u64>::from_raw(abi::RawTask {
            context: (&mut state as *mut State).cast(),
            vtable: &table,
        })
        .unwrap()
    };
    assert_eq!(unsafe { task.poll_raw(&raw_waker()) }, Poll::Pending);
    assert_eq!(unsafe { task.poll_raw(&raw_waker()) }, Poll::Ready(Ok(42)));
    assert_eq!(
        unsafe { task.poll_raw(&raw_waker()) },
        Poll::Ready(Err(Error::Completed))
    );
    drop(task);
    assert_eq!(state.polls, 2);
    assert_eq!(state.releases, 1);
}

#[cfg(feature = "alloc")]
mod allocated {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{
        future::Future,
        pin::Pin,
        sync::Arc,
        task::{Context, Waker},
    };
    struct Drops(Arc<AtomicUsize>);
    impl Drop for Drops {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn cancellation_drops_future_once_and_is_idempotent() {
        let drops = Arc::new(AtomicUsize::new(0));
        let guard = Drops(drops.clone());
        let mut task = OwnedTask::<u64>::new(async move {
            let _guard = guard;
            core::future::pending::<()>().await;
            Ok(9)
        });
        assert_eq!(unsafe { task.poll_raw(&raw_waker()) }, Poll::Pending);
        task.cancel().unwrap();
        task.cancel().unwrap();
        assert_eq!(
            unsafe { task.poll_raw(&raw_waker()) },
            Poll::Ready(Err(Error::Cancelled))
        );
        drop(task);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn completion_and_application_error_preserve_output_rules() {
        let mut task = OwnedTask::new(async { Ok::<_, Error>(17u64) });
        assert_eq!(unsafe { task.poll_raw(&raw_waker()) }, Poll::Ready(Ok(17)));
        let task = OwnedTask::<u64>::new(async { Err(Error::PluginStatus(1)) });
        let raw = task.into_raw();
        let mut output = 0xfeed_u64;
        let mut state = u32::MAX;
        let status = unsafe {
            ((*raw.vtable).poll.unwrap())(
                raw.context,
                &raw_waker(),
                (&mut output as *mut u64).cast(),
                &mut state,
            )
        };
        assert_eq!(status, 1);
        assert_eq!(state, abi::TASK_READY);
        assert_eq!(output, 0xfeed);
        assert_eq!(unsafe { ((*raw.vtable).release.unwrap())(raw.context) }, 0);
    }

    #[test]
    fn waker_retention_replacement_and_worker_wake_are_balanced() {
        struct WakeCounts {
            retained: AtomicUsize,
            released: AtomicUsize,
            wakes: AtomicUsize,
        }
        unsafe extern "C" fn retain(p: *const c_void) -> i32 {
            unsafe { &*p.cast::<WakeCounts>() }
                .retained
                .fetch_add(1, Ordering::SeqCst);
            0
        }
        unsafe extern "C" fn release(p: *const c_void) -> i32 {
            unsafe { &*p.cast::<WakeCounts>() }
                .released
                .fetch_add(1, Ordering::SeqCst);
            0
        }
        unsafe extern "C" fn wake(p: *const c_void) -> i32 {
            unsafe { &*p.cast::<WakeCounts>() }
                .wakes
                .fetch_add(1, Ordering::SeqCst);
            0
        }
        let table = abi::WakerVTable {
            size: size_of::<abi::WakerVTable>() as u32,
            abi_version: 1,
            retain: Some(retain),
            wake: Some(wake),
            release: Some(release),
        };
        let make = || WakeCounts {
            retained: AtomicUsize::new(0),
            released: AtomicUsize::new(0),
            wakes: AtomicUsize::new(0),
        };
        let a = make();
        let b = make();
        let raw_a = abi::RawWaker {
            context: (&a as *const WakeCounts).cast(),
            vtable: &table,
        };
        let raw_b = abi::RawWaker {
            context: (&b as *const WakeCounts).cast(),
            vtable: &table,
        };
        struct Retain {
            seen: usize,
            held: Arc<std::sync::Mutex<Option<Waker>>>,
        }
        impl Future for Retain {
            type Output = Result<u64, Error>;
            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                self.seen += 1;
                *self.held.lock().unwrap() = Some(cx.waker().clone());
                Poll::Pending
            }
        }
        let held = Arc::new(std::sync::Mutex::new(None));
        let mut task = OwnedTask::<u64>::new(Retain {
            seen: 0,
            held: held.clone(),
        });
        assert_eq!(unsafe { task.poll_raw(&raw_a) }, Poll::Pending);
        assert_eq!(unsafe { task.poll_raw(&raw_a) }, Poll::Pending);
        assert_eq!(a.retained.load(Ordering::SeqCst), 1);
        assert_eq!(unsafe { task.poll_raw(&raw_b) }, Poll::Pending);
        assert_eq!(a.released.load(Ordering::SeqCst), 1);
        let waker = held.lock().unwrap().take().unwrap();
        task.cancel().unwrap();
        drop(task);
        assert_eq!(b.released.load(Ordering::SeqCst), 0);
        std::thread::spawn(move || {
            waker.wake_by_ref();
            drop(waker);
        })
        .join()
        .unwrap();
        assert_eq!(b.wakes.load(Ordering::SeqCst), 1);
        assert_eq!(b.released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unit_task_accepts_null_output_and_invalid_waker_is_rejected() {
        let raw = OwnedTask::new(async { Ok(()) }).into_raw();
        let mut state = 99;
        assert_eq!(
            unsafe {
                ((*raw.vtable).poll.unwrap())(
                    raw.context,
                    &raw_waker(),
                    core::ptr::null_mut(),
                    &mut state,
                )
            },
            0
        );
        assert_eq!(state, abi::TASK_READY);
        assert_eq!(unsafe { ((*raw.vtable).release.unwrap())(raw.context) }, 0);
        let short = 4u32;
        let waker = abi::RawWaker {
            context: core::ptr::null(),
            vtable: (&short as *const u32).cast(),
        };
        let mut task = OwnedTask::<u64>::new(async { Ok(2) });
        assert!(matches!(
            unsafe { task.poll_raw(&waker) },
            Poll::Ready(Err(Error::DescriptorTooShort { .. }))
        ));
        assert_eq!(unsafe { task.poll_raw(&raw_waker()) }, Poll::Ready(Ok(2)));
    }

    #[cfg(feature = "std")]
    #[test]
    fn poll_panic_poison_and_panicking_future_drop_are_contained() {
        let mut task = OwnedTask::<u64>::new(async { panic!("poll panic") });
        assert_eq!(
            unsafe { task.poll_raw(&raw_waker()) },
            Poll::Ready(Err(Error::Panicked))
        );
        assert_eq!(
            unsafe { task.poll_raw(&raw_waker()) },
            Poll::Ready(Err(Error::Poisoned))
        );
        drop(task);
        struct BadDrop;
        impl Drop for BadDrop {
            fn drop(&mut self) {
                panic!("future destructor");
            }
        }
        let mut task = OwnedTask::<u64>::new(async {
            let _bad = BadDrop;
            core::future::pending::<()>().await;
            Ok(0)
        });
        assert_eq!(unsafe { task.poll_raw(&raw_waker()) }, Poll::Pending);
        assert_eq!(task.cancel(), Err(Error::Panicked));
        drop(task);
    }
}

#[test]
fn error_statuses_do_not_overlap_pending_state() {
    assert_eq!(zplugin::status_result(-8), Err(Error::Cancelled));
    assert_eq!(zplugin::status_result(-9), Err(Error::Completed));
    assert_eq!(zplugin::status_result(1), Err(Error::PluginStatus(1)));
}
