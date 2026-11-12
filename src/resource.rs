//! Plugin-owned opaque resources with explicit type, version, and release paths.
use crate::{Error, Result, abi, export, runtime::Lease, status_result};
#[cfg(feature = "alloc")]
use core::ffi::c_void;
use core::{marker::PhantomData, mem::MaybeUninit};

/// An application-defined resource; calls are serialized on its creating thread.
/// The handler owns its state and must not borrow its creating plugin instance.
pub trait ResourceHandler: 'static {
    const ID: &'static str;
    const VERSION: u32;

    fn call(&mut self, operation: u32, input: &[u8], output: &mut [u8]) -> Result<usize>;
}

/// Plugin-side unique owner of an opaque resource.
///
/// The raw protocol contains no Rust allocation or object layout. Drop invokes
/// the allocator-matched release function on the creating thread.
pub struct OwnedResource {
    raw: Option<abi::RawResource>,
    _local: PhantomData<*mut ()>,
}

impl OwnedResource {
    /// Construct a resource whose storage and destructor stay in this library.
    #[cfg(feature = "alloc")]
    pub fn new<T: ResourceHandler>(value: T) -> Result<Self> {
        validate_id(T::ID.as_bytes())?;
        if T::VERSION == 0 {
            return Err(Error::InvalidArgument);
        }
        let context =
            alloc::boxed::Box::into_raw(alloc::boxed::Box::new(export::Object::new(value)));
        export::resource_acquired();
        Ok(Self {
            raw: Some(abi::RawResource {
                context: context.cast(),
                vtable: &ResourceRegistration::<T>::VTABLE,
            }),
            _local: PhantomData,
        })
    }

    /// Adopt a fresh, uniquely owned resource on its creating thread.
    ///
    /// # Safety
    /// All metadata, code, and context must remain valid until release. The
    /// resource must own its state independently of its creating instance. Its
    /// callbacks obey the resource v1 protocol and never unwind across the ABI.
    /// The caller retains the origin library, including on validation failure.
    pub unsafe fn from_raw(raw: abi::RawResource) -> Result<Self> {
        unsafe { validate(&raw)? };
        Ok(Self {
            raw: Some(raw),
            _local: PhantomData,
        })
    }

    /// Transfer exactly one release obligation to the raw ABI caller.
    pub fn into_raw(mut self) -> abi::RawResource {
        self.raw.take().unwrap()
    }

    fn table(&self) -> &abi::ResourceVTable {
        // SAFETY: construction validates the table and ownership retains it.
        unsafe { &*self.raw.as_ref().unwrap().vtable }
    }

    pub fn type_id(&self) -> &str {
        // SAFETY: validation checks the ID and the origin retains its metadata.
        unsafe { core::str::from_utf8_unchecked(self.table().type_id.as_slice().unwrap()) }
    }

    pub fn version(&self) -> u32 {
        self.table().resource_version
    }

    /// Require the application's exact resource type and protocol version.
    pub fn check(&self, id: &str, version: u32) -> Result<()> {
        if self.type_id() != id {
            return Err(Error::InterfaceNotFound);
        }
        if self.version() != version {
            return Err(Error::InterfaceVersion {
                expected: version,
                actual: self.version(),
            });
        }
        Ok(())
    }

    /// Borrow input and output only for this call. The written length is valid
    /// only on success; application errors may have modified the output buffer.
    pub fn call(&mut self, operation: u32, input: &[u8], output: &mut [u8]) -> Result<usize> {
        let raw = self.raw.as_ref().unwrap();
        let call = self.table().call.unwrap();
        let mut written = MaybeUninit::<usize>::uninit();
        // SAFETY: Rust borrows are disjoint and this owner retains the context,
        // immutable table, and code for the complete serialized call.
        status_result(unsafe {
            call(
                raw.context,
                operation,
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

    fn release(&mut self) -> i32 {
        self.raw.take().map_or(0, |raw| {
            // SAFETY: this unique owner invokes its release obligation once.
            unsafe { ((*raw.vtable).release.unwrap())(raw.context) }
        })
    }
}

impl Drop for OwnedResource {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// Host-side resource with an independent library lease.
/// Destroying the original proxy does not invalidate this resource.
pub struct Resource<'a> {
    owned: OwnedResource,
    lease: Lease<'a>,
}

impl<'a> Resource<'a> {
    pub(crate) unsafe fn new_owned(raw: abi::RawResource, lease: Lease<'a>) -> Result<Self> {
        match unsafe { OwnedResource::from_raw(raw) } {
            Ok(owned) => Ok(Self { owned, lease }),
            Err(error) => {
                // Unknown malformed ownership cannot safely be released.
                lease.plugin().pin();
                Err(error)
            }
        }
    }

    pub fn type_id(&self) -> &str {
        self.owned.type_id()
    }
    pub fn version(&self) -> u32 {
        self.owned.version()
    }
    pub fn check(&self, id: &str, version: u32) -> Result<()> {
        self.owned.check(id, version)
    }
    pub fn call(&mut self, operation: u32, input: &[u8], output: &mut [u8]) -> Result<usize> {
        self.owned.call(operation, input, output)
    }
}

impl Drop for Resource<'_> {
    fn drop(&mut self) {
        if self.owned.release() != 0 {
            // A failed release leaves possible activity alive; never unload it.
            self.lease.plugin().pin();
        }
    }
}

fn validate_id(id: &[u8]) -> Result<()> {
    if id.is_empty()
        || id.len() > 255
        || !id
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(byte))
    {
        return Err(Error::InvalidArgument);
    }
    Ok(())
}

unsafe fn validate(raw: &abi::RawResource) -> Result<()> {
    if raw.context.is_null() {
        return Err(Error::NullObject);
    }
    export::check_pointer(raw.vtable.cast::<u32>())?;
    // SAFETY: raw producers provide the readable size prefix, even for old tables.
    let size = unsafe { raw.vtable.cast::<u32>().read() };
    if size < core::mem::size_of::<abi::ResourceVTable>() as u32 {
        return Err(Error::DescriptorTooShort {
            expected: core::mem::size_of::<abi::ResourceVTable>() as u32,
            actual: size,
        });
    }
    export::check_pointer(raw.vtable)?;
    let table = unsafe { &*raw.vtable };
    if table.abi_version != 1 {
        return Err(Error::Unsupported);
    }
    validate_id(unsafe { table.type_id.as_slice()? })?;
    if table.resource_version == 0 || table.call.is_none() || table.release.is_none() {
        return Err(Error::InvalidDescriptor);
    }
    Ok(())
}

#[cfg(feature = "alloc")]
struct ResourceRegistration<T>(PhantomData<T>);

#[cfg(feature = "alloc")]
impl<T: ResourceHandler> ResourceRegistration<T> {
    const VTABLE: abi::ResourceVTable = abi::ResourceVTable {
        size: core::mem::size_of::<abi::ResourceVTable>() as u32,
        abi_version: 1,
        type_id: abi::Bytes::from_static(T::ID.as_bytes()),
        resource_version: T::VERSION,
        call: Some(resource_call::<T>),
        release: Some(export::destroy::<T>),
    };
}

#[cfg(feature = "alloc")]
unsafe extern "C" fn resource_call<T: ResourceHandler>(
    context: *mut c_void,
    operation: u32,
    input: abi::Bytes,
    output: abi::BytesMut,
    written: *mut usize,
) -> i32 {
    export::guard(|| {
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
        let count = unsafe {
            export::with_object::<T, _>(context, |handler| {
                let count = handler.call(operation, input.as_slice()?, output.as_mut_slice()?)?;
                if count > output.len {
                    return Err(Error::InvalidArgument);
                }
                Ok(count)
            })
        }?;
        // SAFETY: success initializes the count; errors leave it untouched.
        unsafe {
            written.write(count);
        }
        Ok(())
    })
}
