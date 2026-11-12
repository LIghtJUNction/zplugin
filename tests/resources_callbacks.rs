#![cfg(feature = "alloc")]

use std::{cell::Cell, rc::Rc};
use zplugin::{Callback, CallbackRef, Error, OwnedResource, ResourceHandler, abi};

struct Drops(Rc<Cell<u32>>);
impl Drop for Drops {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

struct Store {
    value: Vec<u8>,
    _drops: Drops,
}

impl ResourceHandler for Store {
    const ID: &'static str = "tests.store";
    const VERSION: u32 = 3;

    fn call(&mut self, operation: u32, input: &[u8], output: &mut [u8]) -> zplugin::Result<usize> {
        match operation {
            0 => {
                self.value.clear();
                self.value.extend_from_slice(input);
                Ok(0)
            }
            1 => {
                if output.len() < self.value.len() {
                    return Err(Error::BufferTooSmall);
                }
                output[..self.value.len()].copy_from_slice(&self.value);
                Ok(self.value.len())
            }
            _ => Err(Error::PluginStatus(81)),
        }
    }
}

#[test]
fn resource_type_state_and_exactly_once_release() {
    let count = Rc::new(Cell::new(0));
    let mut resource = OwnedResource::new(Store {
        value: vec![],
        _drops: Drops(count.clone()),
    })
    .unwrap();
    assert_eq!(resource.type_id(), "tests.store");
    assert_eq!(resource.version(), 3);
    assert_eq!(resource.check("tests.store", 3), Ok(()));
    assert_eq!(
        resource.check("tests.other", 3),
        Err(Error::InterfaceNotFound)
    );
    assert_eq!(
        resource.check("tests.store", 4),
        Err(Error::InterfaceVersion {
            expected: 4,
            actual: 3
        })
    );
    assert_eq!(resource.call(0, b"value", &mut []), Ok(0));
    let mut short = [0xaa; 2];
    assert_eq!(
        resource.call(1, &[], &mut short),
        Err(Error::BufferTooSmall)
    );
    assert_eq!(short, [0xaa; 2]);
    let mut output = [0; 8];
    assert_eq!(resource.call(1, &[], &mut output), Ok(5));
    assert_eq!(&output[..5], b"value");
    assert_eq!(resource.call(9, &[], &mut []), Err(Error::PluginStatus(81)));
    let raw = resource.into_raw();
    assert_eq!(count.get(), 0);
    // SAFETY: this test transfers its one owner, keeping same-process code alive.
    let resource = unsafe { OwnedResource::from_raw(raw) }.unwrap();
    drop(resource);
    assert_eq!(count.get(), 1);
}

#[test]
fn callback_retention_survives_original_owner() {
    let count = Rc::new(Cell::new(0));
    let guard = Drops(count.clone());
    let callback = Callback::new(move |input, output| {
        let _ = &guard;
        if output.len() < input.len() {
            return Err(Error::BufferTooSmall);
        }
        output[..input.len()].copy_from_slice(input);
        Ok(input.len())
    });
    let mut retained = callback.as_ref().try_clone().unwrap();
    let third = retained.try_clone().unwrap();
    drop(callback);
    drop(third);
    assert_eq!(count.get(), 0);
    let mut output = [0; 4];
    assert_eq!(retained.call(b"data", &mut output), Ok(4));
    assert_eq!(&output, b"data");
    assert_eq!(retained.call(&[], &mut []), Ok(0));
    drop(retained);
    assert_eq!(count.get(), 1);
}

#[test]
fn callback_reentry_and_last_release_are_busy() {
    let location = Rc::new(Cell::new(None::<abi::RawCallback>));
    let lookup = location.clone();
    let mut callback = Callback::new(move |_, _| {
        let raw = lookup.get().unwrap();
        let mut written = 711;
        // SAFETY: this test invokes the same live callback on its origin thread;
        // reentry and release must be rejected before accessing its closure.
        unsafe {
            assert_eq!(
                ((*raw.vtable).invoke.unwrap())(
                    raw.context,
                    abi::Bytes {
                        ptr: core::ptr::null(),
                        len: 0
                    },
                    abi::BytesMut {
                        ptr: core::ptr::null_mut(),
                        len: 0
                    },
                    &mut written,
                ),
                Error::Busy.status()
            );
            assert_eq!(
                ((*raw.vtable).release.unwrap())(raw.context),
                Error::Busy.status()
            );
        }
        assert_eq!(written, 711);
        Ok(0)
    });
    location.set(Some(callback.as_ref().as_raw()));
    assert_eq!(callback.call(&[], &mut []), Ok(0));
    assert_eq!(callback.call(&[], &mut []), Ok(0));
}

#[test]
fn callback_rejects_aliases_and_preserves_error_count() {
    let mut callback = Callback::new(|_, _| Err(Error::PluginStatus(31)));
    assert_eq!(callback.call(&[], &mut []), Err(Error::PluginStatus(31)));
    let raw = callback.as_ref().as_raw();
    let mut buffer = [8_u8; 4];
    let mut written = 213;
    // SAFETY: the ranges are mapped, but deliberately alias to test rejection.
    let status = unsafe {
        ((*raw.vtable).invoke.unwrap())(
            raw.context,
            abi::Bytes {
                ptr: buffer.as_ptr(),
                len: buffer.len(),
            },
            abi::BytesMut {
                ptr: buffer.as_mut_ptr(),
                len: buffer.len(),
            },
            &mut written,
        )
    };
    assert_eq!(status, Error::InvalidArgument.status());
    assert_eq!(written, 213);
    assert_eq!(buffer, [8; 4]);
    // Valid empty buffers reach the user error, without initializing written.
    let status = unsafe {
        ((*raw.vtable).invoke.unwrap())(
            raw.context,
            abi::Bytes {
                ptr: core::ptr::null(),
                len: 0,
            },
            abi::BytesMut {
                ptr: core::ptr::null_mut(),
                len: 0,
            },
            &mut written,
        )
    };
    assert_eq!(status, 31);
    assert_eq!(written, 213);
}

#[test]
fn validates_size_before_optional_vtable_fields() {
    let prefix = 4_u32;
    let context = core::ptr::NonNull::<u8>::dangling().as_ptr().cast();
    // SAFETY: only the promised size prefix is readable; no extended field may
    // be read when that prefix reports a truncated descriptor.
    assert!(matches!(
        unsafe {
            CallbackRef::from_raw(abi::RawCallback {
                context,
                vtable: (&prefix as *const u32).cast(),
            })
        },
        Err(Error::DescriptorTooShort { actual: 4, .. })
    ));
    assert!(matches!(
        unsafe {
            OwnedResource::from_raw(abi::RawResource {
                context,
                vtable: (&prefix as *const u32).cast(),
            })
        },
        Err(Error::DescriptorTooShort { actual: 4, .. })
    ));
}

#[test]
fn malformed_tables_are_rejected_before_invocation() {
    let context = core::ptr::NonNull::<u8>::dangling().as_ptr().cast();
    let mut callbacks = abi::CallbackVTable {
        size: core::mem::size_of::<abi::CallbackVTable>() as u32,
        abi_version: 99,
        invoke: None,
        retain: None,
        release: None,
    };
    assert!(matches!(
        unsafe {
            CallbackRef::from_raw(abi::RawCallback {
                context,
                vtable: &callbacks,
            })
        },
        Err(Error::Unsupported)
    ));
    callbacks.abi_version = 1;
    assert!(matches!(
        unsafe {
            CallbackRef::from_raw(abi::RawCallback {
                context,
                vtable: &callbacks,
            })
        },
        Err(Error::InvalidDescriptor)
    ));
    let resources = abi::ResourceVTable {
        size: core::mem::size_of::<abi::ResourceVTable>() as u32,
        abi_version: 1,
        type_id: abi::Bytes::from_static(b"tests.store"),
        resource_version: 3,
        call: None,
        release: None,
    };
    assert!(matches!(
        unsafe {
            OwnedResource::from_raw(abi::RawResource {
                context,
                vtable: &resources,
            })
        },
        Err(Error::InvalidDescriptor)
    ));
}

#[cfg(feature = "std")]
#[test]
fn reverse_callback_panics_poison_and_release_the_context() {
    let count = Rc::new(Cell::new(0));
    let guard = Drops(count.clone());
    let mut callback = Callback::new(move |_, _| {
        let _ = &guard;
        panic!("callback panic");
    });
    assert_eq!(callback.call(&[], &mut []), Err(Error::Panicked));
    assert_eq!(callback.call(&[], &mut []), Err(Error::Poisoned));
    drop(callback);
    assert_eq!(count.get(), 1);
}

#[cfg(feature = "std")]
#[test]
fn resource_panics_poison_the_instance() {
    struct Panics;
    impl ResourceHandler for Panics {
        const ID: &'static str = "tests.panic";
        const VERSION: u32 = 1;
        fn call(&mut self, _: u32, _: &[u8], _: &mut [u8]) -> zplugin::Result<usize> {
            panic!("resource panic");
        }
    }
    let mut resource = OwnedResource::new(Panics).unwrap();
    assert_eq!(resource.call(0, &[], &mut []), Err(Error::Panicked));
    assert_eq!(resource.call(0, &[], &mut []), Err(Error::Poisoned));
}

#[cfg(feature = "std")]
#[test]
fn callback_capture_cleanup_panics_are_contained() {
    struct PanicsOnDrop(Rc<Cell<u32>>);
    impl Drop for PanicsOnDrop {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
            panic!("intentional callback cleanup panic");
        }
    }
    let count = Rc::new(Cell::new(0));
    let capture = PanicsOnDrop(count.clone());
    let callback = Callback::new(move |_, _| {
        let _ = &capture;
        Ok(0)
    });
    let raw = callback.into_raw();
    // SAFETY: consume exactly the reference just transferred by into_raw.
    let status = unsafe { ((*raw.vtable).release.unwrap())(raw.context) };
    assert_eq!(status, Error::Panicked.status());
    assert_eq!(count.get(), 1);
}

#[cfg(all(feature = "macros", feature = "alloc"))]
mod generated {
    use super::*;
    use zplugin::{Plugin, abi};

    #[zplugin::interface(id = "tests.resource.factory", version = 1)]
    trait ResourceFactory {
        fn resource(&mut self) -> zplugin::Result<zplugin::OwnedResource>;
        fn retain_callback(&mut self, callback: zplugin::CallbackRef<'_>) -> zplugin::Result<()>;
        fn invoke(&mut self, input: &[u8], output: &mut [u8]) -> zplugin::Result<u64>;
    }

    struct Factory {
        callback: Option<Callback>,
    }
    impl ResourceFactory for Factory {
        fn resource(&mut self) -> zplugin::Result<zplugin::OwnedResource> {
            OwnedResource::new(Store {
                value: b"independent".to_vec(),
                _drops: Drops(Rc::new(Cell::new(0))),
            })
        }
        fn retain_callback(&mut self, callback: zplugin::CallbackRef<'_>) -> zplugin::Result<()> {
            self.callback = Some(callback.try_clone()?);
            Ok(())
        }
        fn invoke(&mut self, input: &[u8], output: &mut [u8]) -> zplugin::Result<u64> {
            Ok(self
                .callback
                .as_mut()
                .ok_or(Error::Unsupported)?
                .call(input, output)? as u64)
        }
    }
    // SAFETY: the factory creates no threads. Owned resources and retained
    // callbacks contribute to the origin's active count until their final drop.
    unsafe impl zplugin::export::UnloadSafe for Factory {}
    fn create() -> zplugin::Result<Factory> {
        Ok(Factory { callback: None })
    }
    zplugin::export!(
        ResourceFactoryExport<Factory>,
        constructor = create,
        version = (1, 0, 0),
        unload = cooperative
    );
    unsafe extern "C" {
        fn zplugin_entry_v1() -> *const abi::Header;
    }

    #[test]
    fn generated_resource_outlives_proxy_and_retained_callback_survives_owner() {
        let header = unsafe { &*zplugin_entry_v1() };
        let plugin = unsafe { Plugin::from_static(header) }.unwrap();
        let mut factory = ResourceFactoryApi::create(&plugin).unwrap();
        let drops = Rc::new(Cell::new(0));
        let guard = Drops(drops.clone());
        let callback = Callback::new(move |input, output| {
            let _ = &guard;
            if output.len() < input.len() {
                return Err(Error::BufferTooSmall);
            }
            output[..input.len()].copy_from_slice(input);
            Ok(input.len())
        });
        factory.retain_callback(callback.as_ref()).unwrap();
        drop(callback);
        assert_eq!(drops.get(), 0);
        let mut output = [0; 32];
        assert_eq!(factory.invoke(b"retained", &mut output), Ok(8));
        assert_eq!(&output[..8], b"retained");
        let mut resource = factory.resource().unwrap();
        drop(factory);
        assert_eq!(drops.get(), 1);
        assert_eq!(resource.check("tests.store", 3), Ok(()));
        assert_eq!(resource.call(1, &[], &mut output), Ok(11));
        assert_eq!(&output[..11], b"independent");
        drop(resource);
    }
}
