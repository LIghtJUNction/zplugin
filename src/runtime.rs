use crate::{Error, Result, abi, export, loader::Library, status_result};
#[cfg(feature = "alloc")]
use alloc::{boxed::Box, rc::Rc, vec::Vec};
use core::{
    cell::Cell,
    ffi::{CStr, c_void},
    marker::PhantomData,
    ops::Deref,
    ptr::NonNull,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnloadPolicy {
    /// Keep code mapped until process exit. This is the default.
    Pinned,
    /// Close only after host resources and the plugin's quiescence contract agree.
    Cooperative,
}

#[derive(Clone, Copy, Debug)]
pub struct InterfaceSpec {
    pub id: &'static str,
    pub version: u32,
    pub signatures: &'static [&'static str],
}

/// A validated descriptor and owning library. It is neither Send nor Sync.
pub struct Plugin {
    library: Option<Library>,
    descriptor: NonNull<abi::PluginDescriptor>,
    policy: UnloadPolicy,
    pinned: Cell<bool>,
    _local: PhantomData<*mut ()>,
}

impl Plugin {
    /// # Safety
    /// The file and dependencies must be trusted native code and obey ABI v1,
    /// including valid immutable descriptors and all method/ownership contracts.
    pub unsafe fn load(path: &CStr) -> Result<Self> {
        unsafe { Self::load_policy(path, UnloadPolicy::Pinned) }
    }

    /// # Safety
    /// In addition to `load`, every resource outside the host's tracking (threads,
    /// callbacks, tasks, TLS/atexit destructors) must obey `UnloadSafe`. Library
    /// initializers and entry must leave no activity requiring a resident image
    /// when descriptor validation fails. Failed factories must roll back activity.
    pub unsafe fn load_cooperative(path: &CStr) -> Result<Self> {
        unsafe { Self::load_policy(path, UnloadPolicy::Cooperative) }
    }

    unsafe fn load_policy(path: &CStr, policy: UnloadPolicy) -> Result<Self> {
        let library = unsafe { Library::open(path)? };
        unsafe { Self::from_library(library, policy) }
    }

    #[cfg(windows)]
    /// Load a NUL-terminated UTF-16 path without requiring an allocator or std.
    /// # Safety
    /// The same native-code trust and optional cooperative obligations as `load`
    /// and `load_cooperative` apply.
    pub unsafe fn load_wide(path: &[u16], policy: UnloadPolicy) -> Result<Self> {
        let library = unsafe { Library::open_wide(path)? };
        unsafe { Self::from_library(library, policy) }
    }

    #[cfg(feature = "std")]
    /// Load an OS-native path (including Unicode Windows paths).
    /// # Safety
    /// See `load` and, when selected, `load_cooperative`.
    pub unsafe fn load_path(
        path: impl AsRef<std::path::Path>,
        policy: UnloadPolicy,
    ) -> Result<Self> {
        let path = std::fs::canonicalize(path).map_err(|_| Error::LoadFailed)?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(path.as_os_str().as_bytes())
                .map_err(|_| Error::InvalidArgument)?;
            unsafe { Self::load_policy(&path, policy) }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let path: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(core::iter::once(0))
                .collect();
            let library = unsafe { Library::open_wide(&path)? };
            unsafe { Self::from_library(library, policy) }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (path, policy);
            Err(Error::Unsupported)
        }
    }

    unsafe fn from_library(mut library: Library, policy: UnloadPolicy) -> Result<Self> {
        let result = (|| {
            let symbol = library.symbol(abi::ENTRY)?;
            // SAFETY: fixed entry signature is part of the native loading contract.
            let entry: abi::Entry = unsafe { core::mem::transmute(symbol) };
            let descriptor = unsafe { validate_descriptor(entry())? };
            let value = unsafe { descriptor.as_ref() };
            if policy == UnloadPolicy::Cooperative
                && (value.flags & abi::COOPERATIVE == 0 || value.quiesce.is_none())
            {
                return Err(Error::Unsupported);
            }
            Ok(descriptor)
        })();
        match result {
            Ok(descriptor) => Ok(Self {
                library: Some(library),
                descriptor,
                policy,
                pinned: Cell::new(false),
                _local: PhantomData,
            }),
            Err(error) => {
                // SAFETY: cooperative loading explicitly requires inert initializers
                // on failure. No instance was created and no pointer escaped.
                if policy == UnloadPolicy::Cooperative {
                    let _ = unsafe { library.close() };
                }
                Err(error)
            }
        }
    }

    /// Register a static/manual descriptor without an OS loader or allocation.
    /// # Safety
    /// Descriptor and every referenced object/code address must remain valid for
    /// the program's lifetime and obey the same contracts as a loaded library.
    pub unsafe fn from_static(header: &'static abi::Header) -> Result<Self> {
        Ok(Self {
            library: None,
            descriptor: unsafe { validate_descriptor(header)? },
            policy: UnloadPolicy::Pinned,
            pinned: Cell::new(false),
            _local: PhantomData,
        })
    }

    fn descriptor(&self) -> &abi::PluginDescriptor {
        // SAFETY: validated once and kept alive by this library owner.
        unsafe { self.descriptor.as_ref() }
    }

    pub fn version(&self) -> abi::Version {
        self.descriptor().plugin_version
    }
    pub fn unload_policy(&self) -> UnloadPolicy {
        self.policy
    }
    pub fn pin(&self) {
        self.pinned.set(true);
    }
    #[cfg(feature = "hot-reload")]
    pub(crate) fn same_image(&self, other: &Self) -> bool {
        match (&self.library, &other.library) {
            (Some(a), Some(b)) => a.same_image(b),
            (None, None) => self.descriptor == other.descriptor,
            _ => false,
        }
    }
    #[cfg(feature = "hot-reload")]
    pub(crate) fn discard_duplicate(mut self, other: &Self) {
        assert!(self.same_image(other));
        // SAFETY: the other owner still holds the same OS image. This only balances
        // the extra loader reference and cannot run its final unload/destructors.
        if let Some(mut library) = self.library.take() {
            if unsafe { library.close() }.is_err() {
                // Retain accounting for a loader reference that could not close.
                other.pin();
            }
        }
    }

    pub fn bind(&self, spec: InterfaceSpec) -> Result<Binding<'_>> {
        Ok(Binding {
            descriptor: self.find(spec)?,
            spec,
            lease: Lease::Borrowed(self),
        })
    }

    #[cfg(feature = "alloc")]
    pub fn create_owned(plugin: &Rc<Self>, spec: InterfaceSpec) -> Result<Instance<'static>> {
        Binding {
            descriptor: plugin.find(spec)?,
            spec,
            lease: Lease::Shared(Rc::clone(plugin)),
        }
        .create()
    }

    fn find(&self, spec: InterfaceSpec) -> Result<NonNull<abi::InterfaceDescriptor>> {
        let descriptor = self.descriptor();
        if descriptor.interface_count > 4096 {
            return Err(Error::InvalidDescriptor);
        }
        if descriptor.interface_count != 0 {
            export::check_pointer(descriptor.interfaces)?;
        }
        for i in 0..descriptor.interface_count as usize {
            // SAFETY: load's contract guarantees readable pointer array of this size.
            let ptr = unsafe { descriptor.interfaces.add(i).read() };
            export::check_pointer(ptr)?;
            // Read only the size prefix until its full layout is known to be present.
            let size = unsafe { ptr.cast::<u32>().read() };
            require_size(size, core::mem::size_of::<abi::InterfaceDescriptor>())?;
            let interface = unsafe { &*ptr };
            let id = unsafe { interface.id.as_slice()? };
            if id != spec.id.as_bytes() {
                continue;
            }
            if interface.version != spec.version {
                return Err(Error::InterfaceVersion {
                    expected: spec.version,
                    actual: interface.version,
                });
            }
            if interface.method_count as usize != spec.signatures.len() {
                return Err(Error::MissingMethod {
                    method: interface.method_count.min(spec.signatures.len() as u32),
                });
            }
            if interface.create.is_none() || interface.destroy.is_none() {
                return Err(Error::InvalidDescriptor);
            }
            if interface.method_count != 0 {
                export::check_pointer(interface.methods)?;
            }
            for (index, signature) in spec.signatures.iter().enumerate() {
                let method = unsafe { &*interface.methods.add(index) };
                if method.call.is_none() {
                    return Err(Error::MissingMethod {
                        method: index as u32,
                    });
                }
                if unsafe { method.signature.as_slice()? } != signature.as_bytes() {
                    return Err(Error::SignatureMismatch {
                        method: index as u32,
                    });
                }
            }
            return NonNull::new(ptr.cast_mut()).ok_or(Error::InvalidDescriptor);
        }
        Err(Error::InterfaceNotFound)
    }

    /// Attempt a cooperative close; Busy returns ownership so it can be retried.
    pub fn try_close(mut self) -> core::result::Result<(), (Error, Self)> {
        if let Err(error) = self.close() {
            return Err((error, self));
        }
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        if self.library.is_none() {
            return Ok(());
        }
        if self.policy != UnloadPolicy::Cooperative || self.pinned.get() {
            return Err(Error::Busy);
        }
        let quiesce = self.descriptor().quiesce.ok_or(Error::Unsupported)?;
        // SAFETY: cooperative contract and no host resource can survive exclusive
        // ownership of Plugin. Rc owners are unwrapped before reaching this point.
        status_result(unsafe { quiesce() })?;
        unsafe {
            self.library.as_mut().unwrap().close()?;
        }
        self.library = None;
        Ok(())
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn require_size(actual: u32, expected: usize) -> Result<()> {
    if (actual as usize) < expected {
        Err(Error::DescriptorTooShort {
            expected: expected as u32,
            actual,
        })
    } else {
        Ok(())
    }
}

unsafe fn validate_descriptor(
    header: *const abi::Header,
) -> Result<NonNull<abi::PluginDescriptor>> {
    export::check_pointer(header)?;
    // SAFETY: entry guarantees at least the fixed Header is readable.
    let value = unsafe { header.read() };
    if value.magic != abi::MAGIC {
        return Err(Error::BadMagic);
    }
    if value.major != abi::ABI_MAJOR {
        return Err(Error::AbiVersion { found: value.major });
    }
    require_size(value.size, core::mem::size_of::<abi::PluginDescriptor>())?;
    let ptr = header.cast::<abi::PluginDescriptor>();
    export::check_pointer(ptr)?;
    NonNull::new(ptr.cast_mut()).ok_or(Error::InvalidDescriptor)
}

pub(crate) enum Lease<'a> {
    Borrowed(&'a Plugin),
    #[cfg(feature = "alloc")]
    Shared(Rc<Plugin>),
}

impl Clone for Lease<'_> {
    fn clone(&self) -> Self {
        match self {
            Self::Borrowed(plugin) => Self::Borrowed(plugin),
            #[cfg(feature = "alloc")]
            Self::Shared(plugin) => Self::Shared(Rc::clone(plugin)),
        }
    }
}

impl Lease<'_> {
    pub(crate) fn plugin(&self) -> &Plugin {
        match self {
            Self::Borrowed(plugin) => plugin,
            #[cfg(feature = "alloc")]
            Self::Shared(plugin) => plugin,
        }
    }
}

/// A fully checked interface; symbol lookup and signature checks are complete.
pub struct Binding<'a> {
    descriptor: NonNull<abi::InterfaceDescriptor>,
    spec: InterfaceSpec,
    lease: Lease<'a>,
}

impl<'a> Binding<'a> {
    pub fn method(&self, index: usize) -> abi::Call {
        assert!(index < self.spec.signatures.len());
        unsafe { (*self.descriptor.as_ref().methods.add(index)).call.unwrap() }
    }

    pub fn create(&self) -> Result<Instance<'a>> {
        let descriptor = unsafe { self.descriptor.as_ref() };
        let mut object = core::ptr::null_mut();
        let status = unsafe { descriptor.create.unwrap()(&mut object) };
        if let Err(error) = status_result(status) {
            // A failed factory must leave null. A violating plugin is retained.
            if !object.is_null() || error == Error::Panicked {
                self.lease.plugin().pin();
            }
            return Err(error);
        }
        let handle = NonNull::new(object).ok_or(Error::NullObject)?;
        Ok(Instance {
            handle,
            descriptor: self.descriptor,
            spec: self.spec,
            lease: self.lease.clone(),
        })
    }
}

/// A plugin-owned object. Drop calls its original destroy before releasing its lease.
pub struct Instance<'a> {
    handle: NonNull<c_void>,
    descriptor: NonNull<abi::InterfaceDescriptor>,
    spec: InterfaceSpec,
    lease: Lease<'a>,
}

impl<'a> Instance<'a> {
    pub fn handle(&self) -> *mut c_void {
        self.handle.as_ptr()
    }
    pub fn check(&self, spec: InterfaceSpec) -> Result<()> {
        if self.spec.id != spec.id {
            return Err(Error::InterfaceNotFound);
        }
        if self.spec.version != spec.version {
            return Err(Error::InterfaceVersion {
                expected: spec.version,
                actual: self.spec.version,
            });
        }
        if self.spec.signatures != spec.signatures {
            return Err(Error::SignatureMismatch { method: 0 });
        }
        Ok(())
    }
    pub fn method(&self, index: usize) -> abi::Call {
        assert!(index < self.spec.signatures.len());
        unsafe { (*self.descriptor.as_ref().methods.add(index)).call.unwrap() }
    }
    /// Wrap an owned result produced by this instance's validated method.
    /// # Safety
    /// The raw buffer must be a fresh owned result from this same plugin and obey
    /// its release contract. It must not be duplicated or released elsewhere.
    pub unsafe fn buffer(&self, raw: abi::RawBuffer) -> Result<Buffer<'a>> {
        let owned = unsafe { OwnedBytes::from_raw(raw)? };
        Ok(Buffer {
            owned,
            lease: self.lease.clone(),
        })
    }

    /// Attach this library's lifetime to a newly returned task.
    /// # Safety
    /// The task must be uniquely owned, originate from this plugin, and satisfy
    /// `OwnedTask::from_raw`, including independence from this instance.
    pub unsafe fn task<T: crate::AbiValue>(&self, raw: abi::RawTask) -> Result<crate::Task<'a, T>> {
        unsafe { crate::task::Task::new_owned(raw, self.lease.clone()) }
    }

    /// Attach this library's lifetime to a newly returned opaque resource.
    /// # Safety
    /// The resource must be a fresh result from this plugin and obey
    /// `OwnedResource::from_raw`. Its context must outlive this instance.
    pub unsafe fn resource(&self, raw: abi::RawResource) -> Result<crate::Resource<'a>> {
        unsafe { crate::resource::Resource::new_owned(raw, self.lease.clone()) }
    }
}

impl Drop for Instance<'_> {
    fn drop(&mut self) {
        let destroy = unsafe { self.descriptor.as_ref().destroy.unwrap() };
        if unsafe { destroy(self.handle.as_ptr()) } != 0 {
            self.lease.plugin().pin();
        }
    }
}

/// Plugin-side owned bytes. Its v1 representation carries the origin allocator's release.
pub struct OwnedBytes {
    raw: Option<abi::RawBuffer>,
}

impl OwnedBytes {
    /// # Safety
    /// Own a fresh, initialized immutable buffer and a valid release callback.
    /// Its library must remain loaded until release, and the callback must not unwind.
    /// Both data and release context must survive destruction of the originating
    /// instance: they must not borrow instance fields or call a destroyed object.
    pub unsafe fn from_raw(raw: abi::RawBuffer) -> Result<Self> {
        abi::validate_range(raw.data, raw.len)?;
        if raw.release.is_none() {
            return Err(Error::InvalidArgument);
        }
        Ok(Self { raw: Some(raw) })
    }

    #[cfg(feature = "alloc")]
    pub fn from_vec(value: Vec<u8>) -> Self {
        let bytes = value.into_boxed_slice();
        let len = bytes.len();
        let data = Box::into_raw(bytes).cast::<u8>();
        export::resource_acquired();
        Self {
            raw: Some(abi::RawBuffer {
                data,
                len,
                context: core::ptr::null_mut(),
                release: Some(release_boxed),
            }),
        }
    }

    pub fn into_raw(mut self) -> abi::RawBuffer {
        self.raw.take().unwrap()
    }

    fn release(&mut self) -> i32 {
        self.raw.take().map_or(0, |raw| unsafe {
            raw.release.unwrap()(raw.context, raw.data, raw.len)
        })
    }
}

impl Deref for OwnedBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        let raw = self.raw.as_ref().unwrap();
        unsafe {
            abi::Bytes {
                ptr: raw.data,
                len: raw.len,
            }
            .as_slice()
            .unwrap()
        }
    }
}
impl Drop for OwnedBytes {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[cfg(feature = "alloc")]
unsafe extern "C" fn release_boxed(_: *mut c_void, data: *const u8, len: usize) -> i32 {
    export::guard(|| {
        let slice = core::ptr::slice_from_raw_parts_mut(data.cast_mut(), len);
        // SAFETY: exact pointer/length from Box::into_raw, returned once to its allocator.
        unsafe {
            drop(Box::from_raw(slice));
        }
        export::resource_released();
        Ok(())
    })
}

/// Host-side returned bytes. Release runs before its independent library lease drops.
pub struct Buffer<'a> {
    owned: OwnedBytes,
    lease: Lease<'a>,
}
impl Deref for Buffer<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.owned
    }
}
impl Drop for Buffer<'_> {
    fn drop(&mut self) {
        if self.owned.release() != 0 {
            self.lease.plugin().pin();
        }
    }
}
