//! Owned cross-library tasks. A Rust Future and Waker never cross the C ABI.
use crate::{AbiValue, Error, Result, abi, export, runtime::Lease, status_result};
#[cfg(feature = "alloc")]
use core::ffi::c_void;
use core::{marker::PhantomData, mem::MaybeUninit, task::Poll};

/// A task owned by its producing plugin. Poll, cancel and release use its allocator.
/// The context never borrows the object that returned it. It is !Send and !Sync.
pub struct OwnedTask<T: AbiValue> {
    raw: Option<abi::RawTask>,
    terminal: Option<Error>,
    _local: PhantomData<(*mut (), T)>,
}

/// Validate the mandatory size/version prefix before reading an extension table.
/// # Safety
/// `table` points to a readable u32 size; the advertised bytes are readable and
/// immutable until all handles using this table have been released.
pub(crate) unsafe fn table_prefix<P>(table: *const P) -> Result<()> {
    export::check_pointer(table.cast::<u32>())?;
    let size = unsafe { table.cast::<u32>().read() };
    if (size as usize) < core::mem::size_of::<P>() {
        return Err(Error::DescriptorTooShort {
            expected: core::mem::size_of::<P>() as u32,
            actual: size,
        });
    }
    export::check_pointer(table)?;
    let version = unsafe { table.cast::<u32>().add(1).read() };
    if version != abi::EXTENSION_VERSION {
        return Err(Error::Unsupported);
    }
    Ok(())
}

impl<T: AbiValue> OwnedTask<T> {
    /// Adopt one unique task reference, validating its output representation.
    /// # Safety
    /// The context/table/code must remain live through release; every function
    /// obeys the task protocol, never unwinds across C, and accesses the context
    /// only on its creating thread. The context and all retained input data must
    /// survive the creating instance. A failed release leaves code resident.
    pub unsafe fn from_raw(raw: abi::RawTask) -> Result<Self> {
        unsafe { table_prefix(raw.vtable)? };
        let table = unsafe { &*raw.vtable };
        if raw.context.is_null()
            || table.poll.is_none()
            || table.cancel.is_none()
            || table.release.is_none()
        {
            return Err(Error::InvalidDescriptor);
        }
        if table.output_size != core::mem::size_of::<T::Wire>()
            || table.output_align != core::mem::align_of::<T::Wire>()
            || unsafe { table.output_schema.as_slice()? } != T::SCHEMA.as_bytes()
        {
            return Err(Error::SignatureMismatch { method: 0 });
        }
        Ok(Self {
            raw: Some(raw),
            terminal: None,
            _local: PhantomData,
        })
    }

    /// Export a 'static future. No executor is installed or required by zplugin.
    /// Poll/cancel/drop remain local to the creating thread; wakers are thread-safe.
    #[cfg(feature = "alloc")]
    pub fn new<F: core::future::Future<Output = Result<T>> + 'static>(future: F) -> Self {
        let object = alloc::boxed::Box::new(export::Object::new(adapter::FutureState::new(future)));
        export::resource_acquired();
        Self {
            raw: Some(abi::RawTask {
                context: alloc::boxed::Box::into_raw(object).cast(),
                vtable: &adapter::TaskTable::<F, T>::TABLE,
            }),
            terminal: None,
            _local: PhantomData,
        }
    }

    pub fn into_raw(mut self) -> abi::RawTask {
        self.raw.take().unwrap()
    }

    /// Poll using a borrowed C waker. No host allocation is required by this API.
    /// # Safety
    /// The waker obeys its thread-safe retain/wake/release contract. Every retained
    /// reference keeps its context and callback code alive, independently of this
    /// call and of the task. A pending task may retain and wake it after cancellation.
    pub unsafe fn poll_raw(&mut self, waker: &abi::RawWaker) -> Poll<Result<T>> {
        if let Some(error) = self.terminal {
            return Poll::Ready(Err(error));
        }
        if let Err(error) = unsafe { validate_waker(waker) } {
            return Poll::Ready(Err(error));
        }
        let raw = self.raw.as_ref().unwrap();
        let mut output = MaybeUninit::<T::Wire>::uninit();
        let mut state = abi::TASK_PENDING;
        let status = unsafe {
            ((*raw.vtable).poll.unwrap())(
                raw.context,
                waker,
                output.as_mut_ptr().cast(),
                &mut state,
            )
        };
        if let Err(error) = status_result(status) {
            self.terminal = Some(if error == Error::Panicked {
                Error::Poisoned
            } else {
                Error::Completed
            });
            return Poll::Ready(Err(error));
        }
        match state {
            abi::TASK_PENDING => Poll::Pending,
            abi::TASK_READY => {
                self.terminal = Some(Error::Completed);
                Poll::Ready(T::from_wire(unsafe { output.assume_init() }))
            }
            _ => {
                self.terminal = Some(Error::Completed);
                Poll::Ready(Err(Error::InvalidDescriptor))
            }
        }
    }

    /// Request cancellation. Success means the future will no longer be polled.
    /// Busy keeps the task alive so a cooperative implementation can be retried.
    /// Cancelling a completed/already-cancelled task is idempotent.
    pub fn cancel(&mut self) -> Result<()> {
        let raw = self.raw.as_ref().unwrap();
        let result = status_result(unsafe { ((*raw.vtable).cancel.unwrap())(raw.context) });
        if matches!(result, Err(Error::Panicked | Error::Poisoned)) {
            self.terminal = Some(Error::Poisoned);
        }
        result?;
        self.terminal = Some(Error::Cancelled);
        Ok(())
    }

    fn release(&mut self) -> i32 {
        self.raw.take().map_or(0, |raw| unsafe {
            ((*raw.vtable).release.unwrap())(raw.context)
        })
    }
}

impl<T: AbiValue> Drop for OwnedTask<T> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// A host task with an independent lease on its producing library. The task may
/// outlive its creating proxy and may finish while a newer generation is active.
pub struct Task<'a, T: AbiValue> {
    owned: OwnedTask<T>,
    lease: Lease<'a>,
    #[cfg(feature = "alloc")]
    waker: Option<adapter::HostWaker>,
}

impl<'a, T: AbiValue> Task<'a, T> {
    pub(crate) unsafe fn new_owned(raw: abi::RawTask, lease: Lease<'a>) -> Result<Self> {
        let owned = match unsafe { OwnedTask::from_raw(raw) } {
            Ok(value) => value,
            Err(error) => {
                lease.plugin().pin();
                return Err(error);
            }
        };
        Ok(Self {
            owned,
            lease,
            #[cfg(feature = "alloc")]
            waker: None,
        })
    }

    pub fn cancel(&mut self) -> Result<()> {
        self.owned.cancel()
    }

    /// # Safety
    /// See `OwnedTask::poll_raw`; retained wakers can outlive this task.
    pub unsafe fn poll_raw(&mut self, waker: &abi::RawWaker) -> Poll<Result<T>> {
        unsafe { self.owned.poll_raw(waker) }
    }
}

// The plugin future is pinned in its own allocation; moving this handle is safe.
impl<T: AbiValue> Unpin for Task<'_, T> {}

#[cfg(feature = "alloc")]
impl<T: AbiValue> core::future::Future for Task<'_, T> {
    type Output = Result<T>;
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(error) = this.owned.terminal {
            return Poll::Ready(Err(error));
        }
        let replace = this
            .waker
            .as_ref()
            .is_none_or(|old| !old.will_wake(cx.waker()));
        if replace {
            match adapter::HostWaker::new(cx.waker()) {
                Ok(waker) => this.waker = Some(waker),
                Err(error) => {
                    this.owned.terminal = Some(Error::Poisoned);
                    return Poll::Ready(Err(error));
                }
            }
        }
        let host = this.waker.as_ref().unwrap();
        let waker = host.raw();
        let result = unsafe { this.owned.poll_raw(&waker) };
        if let Err(error) = status_result(host.failure()) {
            this.owned.terminal = Some(Error::Poisoned);
            return Poll::Ready(Err(error));
        }
        result
    }
}

impl<T: AbiValue> Drop for Task<'_, T> {
    fn drop(&mut self) {
        if self.owned.release() != 0 {
            self.lease.plugin().pin();
        }
    }
}

unsafe fn validate_waker(raw: &abi::RawWaker) -> Result<()> {
    unsafe { table_prefix(raw.vtable)? };
    let table = unsafe { &*raw.vtable };
    if table.retain.is_none() || table.wake.is_none() || table.release.is_none() {
        return Err(Error::InvalidDescriptor);
    }
    Ok(())
}

#[cfg(feature = "alloc")]
mod adapter {
    use super::*;
    use alloc::{sync::Arc, task::Wake};
    use core::{
        future::Future,
        mem::ManuallyDrop,
        pin::Pin,
        sync::atomic::{AtomicI32, Ordering},
        task::{Context, Waker},
    };

    /// A single allocation pins F for its entire life. ManuallyDrop prevents any
    /// move of !Unpin F when completion/cancellation destroys it in place.
    pub(super) struct FutureState<F> {
        future: ManuallyDrop<F>,
        active: bool,
        cancelled: bool,
        wake: Option<WakeBridge>,
    }
    impl<F> FutureState<F> {
        pub(super) fn new(future: F) -> Self {
            Self {
                future: ManuallyDrop::new(future),
                active: true,
                cancelled: false,
                wake: None,
            }
        }
        fn stop(&mut self) -> Result<()> {
            if self.active {
                self.active = false;
                // SAFETY: the future has never moved; active is cleared first so
                // an unwinding destructor cannot cause a second drop.
                let result = export::catch(|| {
                    unsafe {
                        ManuallyDrop::drop(&mut self.future);
                    }
                    Ok(())
                });
                if result.is_err() {
                    export::resource_failed();
                }
                result?;
            }
            self.wake = None;
            Ok(())
        }
    }
    impl<F> Drop for FutureState<F> {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    pub(super) struct TaskTable<F, T>(PhantomData<(F, T)>);
    impl<F: Future<Output = Result<T>> + 'static, T: AbiValue> TaskTable<F, T> {
        pub(super) const TABLE: abi::TaskVTable = abi::TaskVTable {
            size: core::mem::size_of::<abi::TaskVTable>() as u32,
            abi_version: abi::EXTENSION_VERSION,
            output_size: core::mem::size_of::<T::Wire>(),
            output_align: core::mem::align_of::<T::Wire>(),
            output_schema: abi::Bytes::from_static(T::SCHEMA.as_bytes()),
            poll: Some(poll::<F, T>),
            cancel: Some(cancel::<F>),
            release: Some(export::destroy::<FutureState<F>>),
        };
    }

    unsafe extern "C" fn poll<F: Future<Output = Result<T>>, T: AbiValue>(
        context: *mut c_void,
        waker: *const abi::RawWaker,
        out: *mut c_void,
        state: *mut u32,
    ) -> i32 {
        export::guard(|| {
            export::check_pointer(state)?;
            export::check_pointer(waker)?;
            if core::mem::size_of::<T::Wire>() != 0 {
                export::check_pointer(out.cast::<T::Wire>())?;
            }
            export::validate_borrows(&[
                export::Borrow {
                    ptr: state.cast(),
                    len: core::mem::size_of::<u32>(),
                    mutable: true,
                },
                export::Borrow {
                    ptr: out.cast(),
                    len: core::mem::size_of::<T::Wire>(),
                    mutable: true,
                },
                export::Borrow {
                    ptr: waker.cast(),
                    len: core::mem::size_of::<abi::RawWaker>(),
                    mutable: false,
                },
            ])?;
            unsafe {
                state.write(abi::TASK_PENDING);
            }
            let raw = unsafe { *waker };
            unsafe {
                validate_waker(&raw)?;
            }
            let result = unsafe {
                export::with_object::<FutureState<F>, _>(context, |task| {
                    if !task.active {
                        return Err(if task.cancelled {
                            Error::Cancelled
                        } else {
                            Error::Completed
                        });
                    }
                    if let Some(bridge) = &task.wake {
                        status_result(bridge.inner.failure.load(Ordering::Acquire))?;
                    }
                    let replace = task.wake.as_ref().is_none_or(|old| {
                        old.inner.raw.context != raw.context || old.inner.raw.vtable != raw.vtable
                    });
                    if replace {
                        task.wake = Some(foreign_waker(raw)?);
                    }
                    let wake = &task.wake.as_ref().unwrap().waker;
                    let mut cx = Context::from_waker(wake);
                    // SAFETY: FutureState is in a Box<Object<_>> and its future field
                    // is only polled in place or destroyed by stop, never moved.
                    let value = Pin::new_unchecked(&mut *task.future).poll(&mut cx);
                    let failure = task
                        .wake
                        .as_ref()
                        .unwrap()
                        .inner
                        .failure
                        .load(Ordering::Acquire);
                    if value.is_ready() || failure != 0 {
                        task.stop()?;
                    }
                    status_result(failure)?;
                    Ok(value)
                })
            }?;
            if let Poll::Ready(result) = result {
                unsafe {
                    state.write(abi::TASK_READY);
                }
                let value = result?;
                if core::mem::size_of::<T::Wire>() != 0 {
                    unsafe {
                        out.cast::<T::Wire>().write(value.into_wire());
                    }
                }
            }
            Ok(())
        })
    }

    unsafe extern "C" fn cancel<F>(context: *mut c_void) -> i32 {
        export::guard(|| unsafe {
            export::with_object::<FutureState<F>, _>(context, |task| {
                task.cancelled = true;
                task.stop()
            })
        })
    }

    struct WakeBridge {
        inner: Arc<ForeignWake>,
        waker: Waker,
    }
    struct ForeignWake {
        raw: abi::RawWaker,
        failure: AtomicI32,
    }
    // SAFETY: validate_waker's caller guarantees the entire C waker contract,
    // including concurrent calls and origin-code lifetime until final release.
    unsafe impl Send for ForeignWake {}
    unsafe impl Sync for ForeignWake {}
    impl Wake for ForeignWake {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            if self.failure.load(Ordering::Acquire) != 0 {
                return;
            }
            let status = unsafe { ((*self.raw.vtable).wake.unwrap())(self.raw.context) };
            if status != 0 {
                // Rust Wake has no result channel; surface the first C callback
                // failure at the next poll and stop invoking this failed waker.
                let _ =
                    self.failure
                        .compare_exchange(0, status, Ordering::AcqRel, Ordering::Acquire);
            }
        }
    }
    impl Drop for ForeignWake {
        fn drop(&mut self) {
            let status = unsafe { ((*self.raw.vtable).release.unwrap())(self.raw.context) };
            if status == 0 {
                export::resource_released();
            }
            // Failed foreign release keeps this image resident conservatively.
        }
    }
    fn foreign_waker(raw: abi::RawWaker) -> Result<WakeBridge> {
        status_result(unsafe { ((*raw.vtable).retain.unwrap())(raw.context) })?;
        export::resource_acquired();
        let inner = Arc::new(ForeignWake {
            raw,
            failure: AtomicI32::new(0),
        });
        let waker = Waker::from(Arc::clone(&inner));
        Ok(WakeBridge { inner, waker })
    }

    struct HostWake {
        waker: ManuallyDrop<Waker>,
        failure: AtomicI32,
    }
    impl Drop for HostWake {
        fn drop(&mut self) {
            // A user executor may provide a panicking RawWaker destructor. Do
            // not unwind through a foreign release or drop it a second time.
            let result = export::catch(|| {
                unsafe {
                    ManuallyDrop::drop(&mut self.waker);
                }
                Ok(())
            });
            if result.is_ok() {
                export::resource_released();
            }
        }
    }
    pub(super) struct HostWaker(Arc<HostWake>);
    impl HostWaker {
        pub(super) fn new(waker: &Waker) -> Result<Self> {
            let waker = export::catch(|| Ok(waker.clone()))?;
            export::resource_acquired();
            Ok(Self(Arc::new(HostWake {
                waker: ManuallyDrop::new(waker),
                failure: AtomicI32::new(0),
            })))
        }
        pub(super) fn will_wake(&self, waker: &Waker) -> bool {
            self.0.waker.will_wake(waker)
        }
        pub(super) fn failure(&self) -> i32 {
            self.0.failure.load(Ordering::Acquire)
        }
        pub(super) fn raw(&self) -> abi::RawWaker {
            abi::RawWaker {
                context: Arc::as_ptr(&self.0).cast(),
                vtable: &HOST_WAKER,
            }
        }
    }
    const HOST_WAKER: abi::WakerVTable = abi::WakerVTable {
        size: core::mem::size_of::<abi::WakerVTable>() as u32,
        abi_version: abi::EXTENSION_VERSION,
        retain: Some(host_retain),
        wake: Some(host_wake),
        release: Some(host_release),
    };
    unsafe extern "C" fn host_retain(context: *const c_void) -> i32 {
        export::guard(|| {
            export::check_pointer(context.cast::<HostWake>())?;
            unsafe {
                Arc::increment_strong_count(context.cast::<HostWake>());
            }
            Ok(())
        })
    }
    unsafe extern "C" fn host_wake(context: *const c_void) -> i32 {
        if let Err(error) = export::check_pointer(context.cast::<HostWake>()) {
            return error.status();
        }
        let wake = unsafe { &*context.cast::<HostWake>() };
        if wake.failure.load(Ordering::Acquire) != 0 {
            return Error::Poisoned.status();
        }
        let status = export::guard(|| {
            wake.waker.wake_by_ref();
            Ok(())
        });
        if status != 0 {
            let _ = wake
                .failure
                .compare_exchange(0, status, Ordering::AcqRel, Ordering::Acquire);
        }
        status
    }
    unsafe extern "C" fn host_release(context: *const c_void) -> i32 {
        export::guard(|| {
            export::check_pointer(context.cast::<HostWake>())?;
            unsafe {
                drop(Arc::from_raw(context.cast::<HostWake>()));
            }
            Ok(())
        })
    }
}
