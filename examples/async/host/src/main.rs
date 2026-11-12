use async_interface::{APPLICATION_ERROR, AsyncServiceApi, PANIC, READY, WORKER};
use std::{
    future::Future,
    path::Path,
    pin::Pin,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    thread::{self, Thread},
    time::{Duration, Instant},
};
use zplugin::{Error, Plugin, Result, Task, UnloadPolicy, hot::Generations};

struct Notifier {
    thread: Thread,
    wakes: AtomicUsize,
}

impl Wake for Notifier {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
        self.thread.unpark();
    }
}

fn notifier() -> (Arc<Notifier>, Waker) {
    let notify = Arc::new(Notifier {
        thread: thread::current(),
        wakes: AtomicUsize::new(0),
    });
    let waker = Waker::from(Arc::clone(&notify));
    (notify, waker)
}

fn poll(task: &mut Task<'_, u64>, waker: &Waker) -> Poll<Result<u64>> {
    Pin::new(task).poll(&mut Context::from_waker(waker))
}

fn wait_for_wake(notify: &Notifier) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while notify.wakes.load(Ordering::SeqCst) == 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "worker failed to wake the host within the test deadline"
        );
        thread::park_timeout(left);
    }
}

/// Minimal executor for the example. The deadline diagnoses a failed test; it
/// never forcibly unloads a library or abandons a plugin worker.
fn block_on<F: Future + Unpin>(mut future: F) -> F::Output {
    let (_, waker) = notifier();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match Pin::new(&mut future).poll(&mut Context::from_waker(&waker)) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "async example timed out");
                thread::park_timeout(left);
            }
        }
    }
}

unsafe fn load(path: &Path) -> Result<Plugin> {
    // SAFETY: this executable is run against the matching reviewed example;
    // its resource destructors join every worker before physical unload.
    unsafe { Plugin::load_path(path, UnloadPolicy::Cooperative) }
}

fn contracts(path: &Path) -> Result<usize> {
    let plugin = unsafe { load(path)? };
    let mut service = AsyncServiceApi::create(&plugin)?;
    let (_, waker) = notifier();
    let offset = if plugin.version().major == 2 { 1000 } else { 0 };

    assert_eq!(block_on(service.start(READY, 42)?), Ok(42 + offset));
    assert_eq!(
        block_on(service.start(APPLICATION_ERROR, 0)?),
        Err(Error::PluginStatus(77))
    );
    let mut panicking = service.start(PANIC, 0)?;
    assert_eq!(
        poll(&mut panicking, &waker),
        Poll::Ready(Err(Error::Panicked))
    );
    assert_eq!(
        poll(&mut panicking, &waker),
        Poll::Ready(Err(Error::Poisoned))
    );
    drop(panicking);
    // A failed task does not poison its independent factory object.
    assert_eq!(block_on(service.start(READY, 7)?), Ok(7 + offset));

    let mut pending = service.start(WORKER, 51)?;
    let (old, old_waker) = notifier();
    let (current, current_waker) = notifier();
    assert_eq!(poll(&mut pending, &old_waker), Poll::Pending);
    assert_eq!(poll(&mut pending, &current_waker), Poll::Pending);
    assert_eq!(service.workers()?, 1);
    service.signal()?;
    wait_for_wake(&current);
    assert_eq!(old.wakes.load(Ordering::SeqCst), 0);
    assert_eq!(
        poll(&mut pending, &current_waker),
        Poll::Ready(Ok(51 + offset))
    );
    assert_eq!(
        poll(&mut pending, &current_waker),
        Poll::Ready(Err(Error::Completed))
    );
    drop(pending);
    assert_eq!(service.workers()?, 0);

    let mut unpolled = service.start(WORKER, 0)?;
    unpolled.cancel()?;
    unpolled.cancel()?;
    assert_eq!(
        poll(&mut unpolled, &waker),
        Poll::Ready(Err(Error::Cancelled))
    );
    drop(unpolled);
    assert_eq!(service.workers()?, 0);

    let mut cancelled = service.start(WORKER, 0)?;
    let (notification, cancellation_waker) = notifier();
    assert_eq!(poll(&mut cancelled, &cancellation_waker), Poll::Pending);
    cancelled.cancel()?;
    // The worker emits a final wake during cancellation; cancel joins it and
    // retains every callback until that invocation and destructor have returned.
    assert!(notification.wakes.load(Ordering::SeqCst) >= 1);
    assert_eq!(
        poll(&mut cancelled, &waker),
        Poll::Ready(Err(Error::Cancelled))
    );
    drop(cancelled);
    assert_eq!(service.workers()?, 0);

    let mut dropped = service.start(WORKER, 0)?;
    assert_eq!(poll(&mut dropped, &waker), Poll::Pending);
    drop(dropped);
    assert_eq!(service.workers()?, 0);

    // Exercise the race between a worker scheduling its final wake and host
    // cancellation/release. Every iteration joins; no sleep makes it "safe".
    for iteration in 0..32 {
        let mut racing = service.start(WORKER, iteration)?;
        assert_eq!(poll(&mut racing, &waker), Poll::Pending);
        service.signal()?;
        if iteration % 2 == 0 {
            racing.cancel()?;
        }
        drop(racing);
        assert_eq!(service.workers()?, 0);
    }
    drop(service);
    assert!(plugin.try_close().is_ok());
    Ok(8)
}

fn lease_survives_proxy(path: &Path) -> Result<usize> {
    let plugin = Rc::new(unsafe { load(path)? });
    let offset = if plugin.version().major == 2 { 1000 } else { 0 };
    let weak = Rc::downgrade(&plugin);
    let mut service = AsyncServiceApi::wrap(Plugin::create_owned(&plugin, AsyncServiceApi::SPEC)?)?;
    let mut task = service.start(WORKER, 99)?;
    let (_, waker) = notifier();
    assert_eq!(poll(&mut task, &waker), Poll::Pending);
    service.signal()?;
    drop(service);
    drop(plugin);
    assert!(weak.upgrade().is_some(), "task must retain its library");
    assert_eq!(block_on(task), Ok(99 + offset));
    assert!(
        weak.upgrade().is_none(),
        "completed task drop releases its final library lease"
    );
    Ok(1)
}

fn generations(old_path: &Path, new_path: &Path) -> Result<usize> {
    let mut versions = Generations::new(unsafe { load(old_path)? }, 1);
    let old_library = versions.current();
    let mut old =
        AsyncServiceApi::wrap(Plugin::create_owned(&old_library, AsyncServiceApi::SPEC)?)?;
    let mut old_task = old.start(WORKER, 17)?;
    let (_, waker) = notifier();
    assert_eq!(poll(&mut old_task, &waker), Poll::Pending);
    let mut new = unsafe {
        versions.reload_path(new_path, UnloadPolicy::Cooperative, |plugin| {
            AsyncServiceApi::wrap(Plugin::create_owned(plugin, AsyncServiceApi::SPEC)?)
        })
    }?;
    assert_eq!(versions.retired_count(), 1);
    assert_eq!(block_on(new.start(READY, 17)?), Ok(1017));
    assert_eq!(versions.collect(), 0);
    old.signal()?;
    drop(old);
    drop(old_library);
    assert_eq!(
        versions.collect(),
        0,
        "pending old task retains the old image"
    );
    assert_eq!(block_on(old_task), Ok(17));
    assert_eq!(versions.collect(), 1);
    assert_eq!(versions.retired_count(), 0);
    drop(new);
    Ok(1)
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let path = args
        .first()
        .ok_or("usage: async-host <plugin> [v2-plugin]")?;
    let path = Path::new(path);
    let mut checks = contracts(path)? + lease_survives_proxy(path)?;
    if let Some(new) = args.get(1) {
        checks += generations(path, Path::new(new))?;
    }
    println!("{{\"suite\":\"async-dynamic\",\"checks\":{checks},\"status\":\"passed\"}}");
    Ok(())
}
