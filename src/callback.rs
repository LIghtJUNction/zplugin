//! Same-thread reverse callbacks with explicit retain, invoke, and release.
use crate::{Error, Result, abi, export, status_result};
#[cfg(feature = "alloc")]
use core::{cell::Cell, ffi::c_void};
use core::{marker::PhantomData, mem::MaybeUninit};

/// An owned callback reference. Neither this type nor its borrowed views can
/// cross threads. Reentrant invocation returns Busy before borrowing the closure.
pub struct Callback {
    raw: Option<abi::RawCallback>,
    _local: PhantomData<*mut ()>,
}

impl Callback {
    /// Allocate one callback context. Cloning it retains that same context.
    /// Captures own their state and may outlive the plugin call that receives it.
    #[cfg(feature = "alloc")]
    pub fn new<F>(callback: F) -> Self
    where
        F: FnMut(&[u8], &mut [u8]) -> Result<usize> + 'static,
    {
        let context = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(CallbackState {
            references: Cell::new(1),
            active: Cell::new(false),
            object: export::Object::new(callback),
        }));
        export::resource_acquired();
        Self {
            raw: Some(abi::RawCallback {
                context: context.cast(),
                vtable: &CallbackRegistration::<F>::VTABLE,
            }),
            _local: PhantomData,
        }
    }

    /// Adopt one already-retained reference.
    ///
    /// # Safety
    /// The context, vtable, and origin code must stay valid until all clones have
    /// been released. Calls, retain, and release use the origin thread and cannot
    /// unwind across the ABI. The origin cooperatively retains its library while
    /// callbacks exist, or its code is permanently mapped. On validation failure
    /// the caller remains responsible for the unknown ownership.
    pub unsafe fn from_raw(raw: abi::RawCallback) -> Result<Self> {
        unsafe { validate(raw)? };
        Ok(Self {
            raw: Some(raw),
            _local: PhantomData,
        })
    }

    pub fn as_ref(&self) -> CallbackRef<'_> {
        CallbackRef {
            raw: self.raw.unwrap(),
            _owner: PhantomData,
        }
    }

    pub fn call(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize> {
        self.as_ref().call(input, output)
    }

    pub fn try_clone(&self) -> Result<Self> {
        self.as_ref().try_clone()
    }

    /// Transfer one retained reference, to be released exactly once.
    pub fn into_raw(mut self) -> abi::RawCallback {
        self.raw.take().unwrap()
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY: ownership guarantees an outstanding reference and resident code.
            // A failed release retains origin state and its active-resource count.
            let _ = unsafe { ((*raw.vtable).release.unwrap())(raw.context) };
        }
    }
}

/// A callback borrowed for the current plugin call. `try_clone` explicitly
/// retains it for a later same-thread invocation.
#[derive(Clone, Copy)]
pub struct CallbackRef<'a> {
    raw: abi::RawCallback,
    _owner: PhantomData<&'a Callback>,
}

impl CallbackRef<'_> {
    /// # Safety
    /// An owner keeps this reference alive for `'a`. Origin code and retained
    /// clones obey `Callback::from_raw`, including its thread and unload rules.
    pub unsafe fn from_raw(raw: abi::RawCallback) -> Result<Self> {
        unsafe { validate(raw)? };
        Ok(Self {
            raw,
            _owner: PhantomData,
        })
    }

    /// A borrowed ABI view; copying it does not retain ownership.
    pub fn as_raw(&self) -> abi::RawCallback {
        self.raw
    }

    pub fn try_clone(&self) -> Result<Callback> {
        // SAFETY: this borrow keeps an existing reference alive during retain.
        status_result(unsafe { ((*self.raw.vtable).retain.unwrap())(self.raw.context) })?;
        Ok(Callback {
            raw: Some(self.raw),
            _local: PhantomData,
        })
    }

    pub fn call(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize> {
        let mut written = MaybeUninit::<usize>::uninit();
        // SAFETY: the borrow retains the context; Rust supplies disjoint buffers.
        status_result(unsafe {
            ((*self.raw.vtable).invoke.unwrap())(
                self.raw.context,
                abi::Bytes {
                    ptr: input.as_ptr(),
                    len: input.len(),
                },
                abi::BytesMut {
                    ptr: output.as_mut_ptr(),
                    len: output.len(),
                },
                written.as_mut_ptr(),
            )
        })?;
        let written = unsafe { written.assume_init() };
        if written > output.len() {
            return Err(Error::InvalidDescriptor);
        }
        Ok(written)
    }
}

unsafe fn validate(raw: abi::RawCallback) -> Result<()> {
    if raw.context.is_null() {
        return Err(Error::NullObject);
    }
    export::check_pointer(raw.vtable.cast::<u32>())?;
    // SAFETY: a producer supplies at least the immutable size prefix.
    let size = unsafe { raw.vtable.cast::<u32>().read() };
    if size < core::mem::size_of::<abi::CallbackVTable>() as u32 {
        return Err(Error::DescriptorTooShort {
            expected: core::mem::size_of::<abi::CallbackVTable>() as u32,
            actual: size,
        });
    }
    export::check_pointer(raw.vtable)?;
    let table = unsafe { &*raw.vtable };
    if table.abi_version != 1 {
        return Err(Error::Unsupported);
    }
    if table.invoke.is_none() || table.retain.is_none() || table.release.is_none() {
        return Err(Error::InvalidDescriptor);
    }
    Ok(())
}

#[cfg(feature = "alloc")]
struct CallbackState<F> {
    references: Cell<usize>,
    active: Cell<bool>,
    object: export::Object<F>,
}

#[cfg(feature = "alloc")]
struct CallbackRegistration<F>(PhantomData<F>);

#[cfg(feature = "alloc")]
impl<F: FnMut(&[u8], &mut [u8]) -> Result<usize> + 'static> CallbackRegistration<F> {
    const VTABLE: abi::CallbackVTable = abi::CallbackVTable {
        size: core::mem::size_of::<abi::CallbackVTable>() as u32,
        abi_version: 1,
        invoke: Some(invoke::<F>),
        retain: Some(retain::<F>),
        release: Some(release::<F>),
    };
}

#[cfg(feature = "alloc")]
unsafe extern "C" fn invoke<F: FnMut(&[u8], &mut [u8]) -> Result<usize> + 'static>(
    context: *mut c_void,
    input: abi::Bytes,
    output: abi::BytesMut,
    written: *mut usize,
) -> i32 {
    export::guard(|| {
        export::check_pointer(context.cast::<CallbackState<F>>())?;
        export::check_pointer(written)?;
        export::validate_borrows(&[
            export::Borrow {
                ptr: input.ptr,
                len: input.len,
                mutable: false,
            },
            export::Borrow {
                ptr: output.ptr,
                len: output.len,
                mutable: true,
            },
            export::Borrow {
                ptr: written.cast(),
                len: core::mem::size_of::<usize>(),
                mutable: true,
            },
        ])?;
        let state = unsafe { &*context.cast::<CallbackState<F>>() };
        if state.active.replace(true) {
            return Err(Error::Busy);
        }
        struct Reset<'a>(&'a Cell<bool>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _reset = Reset(&state.active);
        let object = (&state.object as *const export::Object<F>)
            .cast_mut()
            .cast();
        let count = unsafe {
            export::with_object::<F, _>(object, |callback| {
                let count = callback(input.as_slice()?, output.as_mut_slice()?)?;
                if count > output.len {
                    return Err(Error::InvalidArgument);
                }
                Ok(count)
            })
        }?;
        unsafe {
            written.write(count);
        }
        Ok(())
    })
}

#[cfg(feature = "alloc")]
unsafe extern "C" fn retain<F>(context: *mut c_void) -> i32 {
    export::guard(|| {
        export::check_pointer(context.cast::<CallbackState<F>>())?;
        let state = unsafe { &*context.cast::<CallbackState<F>>() };
        let count = state.references.get().checked_add(1).ok_or(Error::Busy)?;
        state.references.set(count);
        Ok(())
    })
}

#[cfg(feature = "alloc")]
unsafe extern "C" fn release<F>(context: *mut c_void) -> i32 {
    export::guard(|| {
        export::check_pointer(context.cast::<CallbackState<F>>())?;
        let state = unsafe { &*context.cast::<CallbackState<F>>() };
        let count = state.references.get();
        if count > 1 {
            state.references.set(count - 1);
            return Ok(());
        }
        if state.active.get() {
            return Err(Error::Busy);
        }
        let result = export::catch(|| {
            // SAFETY: the final reference is released on the origin thread,
            // with no invocation active. Box drop glue frees even if F panics.
            unsafe {
                drop(alloc::boxed::Box::from_raw(
                    context.cast::<CallbackState<F>>(),
                ));
            }
            Ok(())
        });
        if result.is_err() {
            // Capture cleanup may have left origin-side activity behind.
            export::resource_failed();
        }
        export::resource_released();
        result
    })
}
