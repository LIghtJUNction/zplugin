use async_interface::{APPLICATION_ERROR, AsyncService, AsyncServiceExport, PANIC, READY, WORKER};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicU32, Ordering},
    },
    task::{Context, Poll, Waker},
    thread::JoinHandle,
};
use zplugin::{Error, OwnedTask, Result};

#[cfg(feature = "instrument")]
#[path = "../../../plugin/src/measurements.rs"]
mod measurements;

#[cfg(not(feature = "v2"))]
const OFFSET: u64 = 0;
#[cfg(feature = "v2")]
const OFFSET: u64 = 1000;

static WORKERS: AtomicU32 = AtomicU32::new(0);

#[derive(Default)]
struct Gate {
    signalled: bool,
    cancelled: bool,
    ready: bool,
    waker: Option<Waker>,
}

#[derive(Default)]
struct Shared {
    gate: Mutex<Gate>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Gate> {
        self.gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct WorkerFuture {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    value: u64,
}

impl WorkerFuture {
    fn start(shared: Arc<Shared>, value: u64) -> Result<Self> {
        let background = Arc::clone(&shared);
        WORKERS.fetch_add(1, Ordering::SeqCst);
        let worker = match std::thread::Builder::new()
            .name("zplugin-example".into())
            .spawn(move || {
                struct Exited;
                impl Drop for Exited {
                    fn drop(&mut self) {
                        WORKERS.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                let _exited = Exited;
                let mut gate = background.lock();
                while !gate.signalled && !gate.cancelled {
                    gate = background
                        .changed
                        .wait(gate)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                gate.ready = !gate.cancelled;
                let waker = gate.waker.take();
                drop(gate);
                // Cancellation may race a scheduled wake. Drop joins this worker,
                // so the callback and its destructor finish before library release.
                if let Some(waker) = waker {
                    waker.wake();
                }
            }) {
            Ok(worker) => worker,
            Err(_) => {
                WORKERS.fetch_sub(1, Ordering::SeqCst);
                return Err(Error::PluginStatus(89));
            }
        };
        Ok(Self {
            shared,
            worker: Some(worker),
            value,
        })
    }
}

impl Future for WorkerFuture {
    type Output = Result<u64>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut gate = self.shared.lock();
        if gate.ready {
            return Poll::Ready(Ok(self.value.wrapping_add(OFFSET)));
        }
        if gate.cancelled {
            return Poll::Ready(Err(Error::Cancelled));
        }
        if gate
            .waker
            .as_ref()
            .is_none_or(|old| !old.will_wake(cx.waker()))
        {
            gate.waker = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

impl Drop for WorkerFuture {
    fn drop(&mut self) {
        {
            let mut gate = self.shared.lock();
            gate.cancelled = true;
            self.shared.changed.notify_one();
        }
        // No timeout and no detached thread: cooperative resource destruction
        // waits until every worker-held waker and code reference has retired.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Default)]
struct Service {
    latest: Option<Arc<Shared>>,
}

impl AsyncService for Service {
    fn start(&mut self, mode: u32, value: u64) -> Result<OwnedTask<u64>> {
        match mode {
            READY => Ok(OwnedTask::new(std::future::ready(Ok(
                value.wrapping_add(OFFSET)
            )))),
            WORKER => {
                let shared = Arc::new(Shared::default());
                let future = WorkerFuture::start(Arc::clone(&shared), value)?;
                self.latest = Some(shared);
                Ok(OwnedTask::new(future))
            }
            APPLICATION_ERROR => Ok(OwnedTask::new(std::future::ready(Err(
                Error::PluginStatus(77),
            )))),
            PANIC => Ok(OwnedTask::new(std::future::poll_fn(
                |_| -> Poll<Result<u64>> {
                    panic!("intentional async example panic");
                },
            ))),
            _ => Err(Error::InvalidArgument),
        }
    }

    fn signal(&mut self) -> Result<()> {
        let shared = self.latest.as_ref().ok_or(Error::InvalidArgument)?;
        let mut gate = shared.lock();
        gate.signalled = true;
        shared.changed.notify_one();
        Ok(())
    }

    fn workers(&self) -> Result<u32> {
        Ok(WORKERS.load(Ordering::SeqCst))
    }
}

// SAFETY: task state owns its data independently of Service. Each worker is
// joined by its future destructor on success, cancellation, and task release.
// zplugin tracks all task/waker resources until their matching destructors run.
unsafe impl zplugin::export::UnloadSafe for Service {}

fn create() -> Result<Service> {
    Ok(Service::default())
}

#[cfg(not(feature = "v2"))]
zplugin::export!(
    AsyncServiceExport<Service>,
    constructor = create,
    version = (1, 0, 0),
    unload = cooperative
);
#[cfg(feature = "v2")]
zplugin::export!(
    AsyncServiceExport<Service>,
    constructor = create,
    version = (2, 0, 0),
    unload = cooperative
);
