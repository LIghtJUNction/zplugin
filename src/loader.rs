//! Thin system-loader bindings. Raw libraries are pinned until explicitly closed.
use crate::{Error, Result};
use core::{
    ffi::{CStr, c_void},
    marker::PhantomData,
    ptr::NonNull,
};

/// A thread-local library handle. Drop retains the mapping; Plugin coordinates close.
pub struct Library {
    handle: Option<NonNull<c_void>>,
    _local: PhantomData<*mut ()>,
}

impl Library {
    /// Load trusted native code. Unix paths use native bytes; Windows uses its ANSI
    /// code page here. Use `open_wide` or the std path API for Unicode on Windows.
    /// # Safety
    /// Loading runs native initializers with process privileges. The caller must
    /// trust this file and all dependencies and uphold their platform requirements.
    pub unsafe fn open(path: &CStr) -> Result<Self> {
        // SAFETY: delegated native-code trust boundary; CStr is NUL terminated.
        let handle = unsafe { platform::open(path.as_ptr()) };
        NonNull::new(handle)
            .map(|handle| Self {
                handle: Some(handle),
                _local: PhantomData,
            })
            .ok_or(Error::LoadFailed)
    }

    #[cfg(windows)]
    /// Load a NUL-terminated UTF-16 path.
    /// # Safety
    /// The same native-code trust requirements as `open` apply.
    pub unsafe fn open_wide(path: &[u16]) -> Result<Self> {
        if path.last() != Some(&0) || path[..path.len().saturating_sub(1)].contains(&0) {
            return Err(Error::InvalidArgument);
        }
        let handle = unsafe { platform::open_wide(path.as_ptr()) };
        NonNull::new(handle)
            .map(|handle| Self {
                handle: Some(handle),
                _local: PhantomData,
            })
            .ok_or(Error::LoadFailed)
    }

    /// Resolve once and cache. Calling or dereferencing the address remains unsafe.
    pub fn symbol(&self, name: &CStr) -> Result<*mut c_void> {
        let handle = self.handle.ok_or(Error::LoadFailed)?;
        // SAFETY: live handle and valid symbol string. No typed pointer is invented.
        let symbol = unsafe { platform::symbol(handle.as_ptr(), name.as_ptr()) };
        if symbol.is_null() {
            Err(Error::MissingEntry)
        } else {
            Ok(symbol)
        }
    }

    /// # Safety
    /// No object, buffer, thread, task, callback, destructor, reference, or function
    /// pointer may still need the library. Finalizers must be safe to execute now.
    pub unsafe fn close(&mut self) -> Result<()> {
        if let Some(handle) = self.handle {
            if !unsafe { platform::close(handle.as_ptr()) } {
                return Err(Error::Busy);
            }
            self.handle = None;
        }
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use core::ffi::{c_char, c_int, c_void};
    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    unsafe extern "C" {
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
    }
    pub unsafe fn open(path: *const c_char) -> *mut c_void {
        #[cfg(target_os = "macos")]
        const LOCAL: c_int = 4;
        #[cfg(target_os = "linux")]
        const LOCAL: c_int = 0;
        unsafe { dlopen(path, 2 | LOCAL) } // RTLD_NOW | RTLD_LOCAL
    }
    pub unsafe fn symbol(handle: *mut c_void, name: *const c_char) -> *mut c_void {
        unsafe { dlsym(handle, name) }
    }
    pub unsafe fn close(handle: *mut c_void) -> bool {
        unsafe { dlclose(handle) == 0 }
    }
}

#[cfg(windows)]
mod platform {
    use core::ffi::{c_char, c_void};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExA(path: *const c_char, file: *mut c_void, flags: u32) -> *mut c_void;
        fn LoadLibraryExW(path: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn FreeLibrary(handle: *mut c_void) -> i32;
    }
    pub unsafe fn open(path: *const c_char) -> *mut c_void {
        unsafe { LoadLibraryExA(path, core::ptr::null_mut(), 0x1000) }
    }
    pub unsafe fn open_wide(path: *const u16) -> *mut c_void {
        unsafe { LoadLibraryExW(path, core::ptr::null_mut(), 0x1000) }
    }
    pub unsafe fn symbol(handle: *mut c_void, name: *const c_char) -> *mut c_void {
        unsafe { GetProcAddress(handle, name) }
    }
    pub unsafe fn close(handle: *mut c_void) -> bool {
        unsafe { FreeLibrary(handle) != 0 }
    }
}

// Protocol/manual registration remain usable on other no_std targets.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use core::ffi::{c_char, c_void};
    pub unsafe fn open(_: *const c_char) -> *mut c_void {
        core::ptr::null_mut()
    }
    pub unsafe fn symbol(_: *mut c_void, _: *const c_char) -> *mut c_void {
        core::ptr::null_mut()
    }
    pub unsafe fn close(_: *mut c_void) -> bool {
        false
    }
}
