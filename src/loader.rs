//! Project Z native loader, backed by zsys. Images remain pinned until closed.
use crate::{Error, Result};
use core::ffi::{CStr, c_void};

/// A thread-local library handle. Drop retains the mapping; Plugin coordinates close.
pub struct Library {
    inner: zsys::dylib::Library,
}

impl Library {
    #[cfg(feature = "hot-reload")]
    pub(crate) fn same_image(&self, other: &Self) -> bool {
        self.inner.same_image(&other.inner)
    }

    /// Load trusted native code. Unix paths use native bytes; Windows uses its ANSI
    /// code page here. Use `open_wide` or the std path API for Unicode on Windows.
    /// # Safety
    /// Loading runs native initializers with process privileges. The caller must
    /// trust this file and all dependencies and uphold their platform requirements.
    pub unsafe fn open(path: &CStr) -> Result<Self> {
        // SAFETY: the caller supplies the same trust contract required by zsys.
        unsafe { zsys::dylib::Library::open(path) }
            .map(|inner| Self { inner })
            .map_err(loader_error)
    }

    #[cfg(windows)]
    /// Load a NUL-terminated UTF-16 path.
    /// # Safety
    /// The same native-code trust requirements as `open` apply.
    pub unsafe fn open_wide(path: &[u16]) -> Result<Self> {
        // SAFETY: zsys validates the path; the caller supplies native-code trust.
        unsafe { zsys::dylib::Library::open_wide(path) }
            .map(|inner| Self { inner })
            .map_err(loader_error)
    }

    /// Resolve once and cache. Calling or dereferencing the address remains unsafe.
    pub fn symbol(&self, name: &CStr) -> Result<*mut c_void> {
        self.inner.symbol(name).map_err(loader_error)
    }

    /// # Safety
    /// No object, buffer, thread, task, callback, destructor, reference, or function
    /// pointer may still need the library. Finalizers must be safe to execute now.
    pub unsafe fn close(&mut self) -> Result<()> {
        // SAFETY: Plugin's caller has satisfied the shared explicit-close contract.
        unsafe { self.inner.close() }.map_err(loader_error)
    }
}

fn loader_error(error: zsys::dylib::Error) -> Error {
    match error {
        zsys::dylib::Error::Unsupported
        | zsys::dylib::Error::LoadFailed
        | zsys::dylib::Error::Closed => Error::LoadFailed,
        zsys::dylib::Error::MissingSymbol => Error::MissingEntry,
        zsys::dylib::Error::CloseFailed => Error::Busy,
        zsys::dylib::Error::InvalidPath => Error::InvalidArgument,
    }
}
