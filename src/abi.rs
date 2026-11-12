//! Protocol v1. All pointers refer to the current process and target C ABI.
use crate::{Error, Result};
use core::{ffi::c_void, ptr::NonNull};

pub const MAGIC: u64 = 0x315f4e4947554c50;
pub const ABI_MAJOR: u16 = 1;
pub const ABI_MINOR: u16 = 0;
pub const COOPERATIVE: u32 = 1;
pub const ENTRY: &core::ffi::CStr = c"zplugin_entry_v1";

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub magic: u64,
    pub major: u16,
    pub minor: u16,
    pub size: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// Immutable bytes. Null is permitted only at length zero.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Bytes {
    pub ptr: *const u8,
    pub len: usize,
}

// Sharing raw addresses alone cannot dereference them; all reads remain unsafe.
unsafe impl Sync for Bytes {}

impl Bytes {
    pub const fn from_static(bytes: &'static [u8]) -> Self {
        Self {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        }
    }

    /// # Safety
    /// The range must be readable, initialized, and immutable for `'a`.
    pub unsafe fn as_slice<'a>(self) -> Result<&'a [u8]> {
        validate_range(self.ptr, self.len)?;
        let ptr = if self.len == 0 {
            NonNull::<u8>::dangling().as_ptr()
        } else {
            self.ptr
        };
        // SAFETY: the caller guarantees validity; the zero-length pointer is nonnull.
        Ok(unsafe { core::slice::from_raw_parts(ptr, self.len) })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BytesMut {
    pub ptr: *mut u8,
    pub len: usize,
}

impl BytesMut {
    /// # Safety
    /// The range must be initialized, writable, and exclusively borrowed for `'a`.
    pub unsafe fn as_mut_slice<'a>(self) -> Result<&'a mut [u8]> {
        validate_range(self.ptr, self.len)?;
        let ptr = if self.len == 0 {
            NonNull::<u8>::dangling().as_ptr()
        } else {
            self.ptr
        };
        // SAFETY: validity and exclusivity are the caller's contract.
        Ok(unsafe { core::slice::from_raw_parts_mut(ptr, self.len) })
    }
}

pub(crate) fn validate_range(ptr: *const u8, len: usize) -> Result<()> {
    if len > isize::MAX as usize
        || (len != 0 && ptr.is_null())
        || (ptr as usize).checked_add(len).is_none()
    {
        return Err(Error::InvalidArgument);
    }
    Ok(())
}

/// Every generated method has its own frozen repr(C) argument/result records.
pub type Call = unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> i32;
pub type Create = unsafe extern "C" fn(*mut *mut c_void) -> i32;
pub type Destroy = unsafe extern "C" fn(*mut c_void) -> i32;
pub type Quiesce = unsafe extern "C" fn() -> i32;
pub type Entry = unsafe extern "C" fn() -> *const Header;
pub type Release = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Method {
    pub signature: Bytes,
    pub call: Option<Call>,
}

#[repr(C)]
pub struct InterfaceDescriptor {
    pub size: u32,
    pub version: u32,
    pub id: Bytes,
    pub method_count: u32,
    pub methods: *const Method,
    pub create: Option<Create>,
    pub destroy: Option<Destroy>,
}

// Descriptor producers promise immutable metadata and stable code addresses.
unsafe impl Sync for InterfaceDescriptor {}

#[repr(C)]
pub struct PluginDescriptor {
    pub header: Header,
    pub plugin_version: Version,
    pub flags: u32,
    pub interface_count: u32,
    pub interfaces: *const *const InterfaceDescriptor,
    pub quiesce: Option<Quiesce>,
}

// As above: field access is safe, following foreign pointers remains unsafe.
unsafe impl Sync for PluginDescriptor {}

/// Ownership crosses the boundary only with its originating release function.
#[repr(C)]
pub struct RawBuffer {
    pub data: *const u8,
    pub len: usize,
    pub context: *mut c_void,
    pub release: Option<Release>,
}

/// Additive extension records. These do not change any protocol-v1 descriptor.
pub const EXTENSION_VERSION: u32 = 1;
pub const TASK_PENDING: u32 = 0;
pub const TASK_READY: u32 = 1;

pub type TaskPoll =
    unsafe extern "C" fn(*mut c_void, *const RawWaker, *mut c_void, *mut u32) -> i32;
pub type WakeFn = unsafe extern "C" fn(*const c_void) -> i32;

/// A borrowed view. Retain before storing; release each retained reference once.
/// All three operations, including final release, must be thread-safe.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawWaker {
    pub context: *const c_void,
    pub vtable: *const WakerVTable,
}

#[repr(C)]
pub struct WakerVTable {
    pub size: u32,
    pub abi_version: u32,
    pub retain: Option<WakeFn>,
    /// Wake by reference; does not consume a reference.
    pub wake: Option<WakeFn>,
    pub release: Option<WakeFn>,
}

/// A uniquely owned, single-threaded task. Only its waker may cross threads.
#[repr(C)]
pub struct RawTask {
    pub context: *mut c_void,
    pub vtable: *const TaskVTable,
}

#[repr(C)]
pub struct TaskVTable {
    pub size: u32,
    pub abi_version: u32,
    pub output_size: usize,
    pub output_align: usize,
    pub output_schema: Bytes,
    /// Sets state on every call. Output is initialized only on READY + status 0.
    pub poll: Option<TaskPoll>,
    pub cancel: Option<Destroy>,
    pub release: Option<Destroy>,
}
// Immutable foreign metadata; dereferencing its pointers remains unsafe.
unsafe impl Sync for TaskVTable {}

pub type ResourceCall = unsafe extern "C" fn(*mut c_void, u32, Bytes, BytesMut, *mut usize) -> i32;
pub type CallbackCall = unsafe extern "C" fn(*mut c_void, Bytes, BytesMut, *mut usize) -> i32;

#[repr(C)]
pub struct RawResource {
    pub context: *mut c_void,
    pub vtable: *const ResourceVTable,
}

#[repr(C)]
pub struct ResourceVTable {
    pub size: u32,
    pub abi_version: u32,
    pub type_id: Bytes,
    pub resource_version: u32,
    pub call: Option<ResourceCall>,
    pub release: Option<Destroy>,
}
unsafe impl Sync for ResourceVTable {}

/// Copying this borrowed view does not retain its context.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawCallback {
    pub context: *mut c_void,
    pub vtable: *const CallbackVTable,
}

#[repr(C)]
pub struct CallbackVTable {
    pub size: u32,
    pub abi_version: u32,
    pub invoke: Option<CallbackCall>,
    pub retain: Option<Destroy>,
    pub release: Option<Destroy>,
}
