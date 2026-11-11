//! Independently compiled malformed plugins: no dependency on zplugin itself.
#![allow(dead_code)]

use core::{ffi::c_void, mem::size_of};

#[repr(C)]
pub struct Header {
    magic: u64,
    major: u16,
    minor: u16,
    size: u32,
}
#[repr(C)]
struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Bytes {
    ptr: *const u8,
    len: usize,
}
unsafe impl Sync for Bytes {}
#[repr(C)]
#[derive(Clone, Copy)]
struct Method {
    signature: Bytes,
    call: Option<unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> i32>,
}
#[repr(C)]
struct Interface {
    size: u32,
    version: u32,
    id: Bytes,
    method_count: u32,
    methods: *const Method,
    create: Option<unsafe extern "C" fn(*mut *mut c_void) -> i32>,
    destroy: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
}
unsafe impl Sync for Interface {}
#[repr(C)]
struct Descriptor {
    header: Header,
    version: Version,
    flags: u32,
    interface_count: u32,
    interfaces: *const *const Interface,
    quiesce: Option<unsafe extern "C" fn() -> i32>,
}
unsafe impl Sync for Descriptor {}

unsafe extern "C" fn unavailable(_: *mut c_void, _: *const c_void, _: *mut c_void) -> i32 {
    -7
}
unsafe extern "C" fn create(out: *mut *mut c_void) -> i32 {
    if !out.is_null() {
        unsafe {
            out.write(core::ptr::null_mut());
        }
    }
    -7
}
unsafe extern "C" fn destroy(_: *mut c_void) -> i32 {
    -7
}
unsafe extern "C" fn quiesce() -> i32 {
    if cfg!(fixture = "busy") { -4 } else { 0 }
}

static METHODS: [Method; 8] = [Method {
    signature: Bytes {
        ptr: b"intentionally-wrong".as_ptr(),
        len: 19,
    },
    call: if cfg!(fixture = "missing_method") {
        None
    } else {
        Some(unavailable)
    },
}; 8];

static INTERFACE: Interface = Interface {
    size: size_of::<Interface>() as u32,
    version: if cfg!(fixture = "version") { 999 } else { 1 },
    id: Bytes {
        ptr: b"dev.project-z.demo".as_ptr(),
        len: 18,
    },
    method_count: 8,
    methods: METHODS.as_ptr(),
    create: Some(create),
    destroy: Some(destroy),
};

#[repr(C, align(16))]
struct ShortInterface {
    size: u32,
}
static SHORT_INTERFACE: ShortInterface = ShortInterface { size: 4 };
struct InterfacePointers([*const Interface; 1]);
unsafe impl Sync for InterfacePointers {}
static INTERFACES: InterfacePointers = InterfacePointers([if cfg!(fixture = "short_interface") {
    (&SHORT_INTERFACE as *const ShortInterface).cast::<Interface>()
} else {
    &INTERFACE
}]);
static DESCRIPTOR: Descriptor = Descriptor {
    header: Header {
        magic: 0x315f_4e49_4755_4c50,
        major: 1,
        minor: 0,
        size: size_of::<Descriptor>() as u32,
    },
    version: Version {
        major: 1,
        minor: 0,
        patch: 0,
    },
    flags: 1,
    interface_count: 1,
    interfaces: INTERFACES.0.as_ptr(),
    quiesce: Some(quiesce),
};
// These really are header-only allocations: a loader must not read the tail.
static SHORT: Header = Header {
    magic: 0x315f_4e49_4755_4c50,
    major: 1,
    minor: 0,
    size: size_of::<Header>() as u32,
};
static WRONG_ABI: Header = Header {
    magic: 0x315f_4e49_4755_4c50,
    major: 99,
    minor: 0,
    size: size_of::<Header>() as u32,
};
static WRONG_MAGIC: Header = Header {
    magic: 0,
    major: 1,
    minor: 0,
    size: size_of::<Header>() as u32,
};

#[cfg(not(fixture = "missing"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zplugin_entry_v1() -> *const Header {
    if cfg!(fixture = "null") {
        return core::ptr::null();
    }
    if cfg!(fixture = "abi") {
        return &WRONG_ABI;
    }
    if cfg!(fixture = "magic") {
        return &WRONG_MAGIC;
    }
    if cfg!(fixture = "short") {
        return &SHORT;
    }
    &DESCRIPTOR.header
}

#[cfg(fixture = "missing")]
#[unsafe(no_mangle)]
pub extern "C" fn not_the_plugin_entry() -> u32 {
    42
}
