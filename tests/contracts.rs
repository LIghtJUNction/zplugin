use core::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use zplugin::{Error, InterfaceSpec, Plugin, abi, export, status_result};

const SPEC: InterfaceSpec = InterfaceSpec {
    id: "test.counter",
    version: 1,
    signatures: &["v1:get:shared()->result<u64>"],
};
static DESTROYED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn create(out: *mut *mut c_void) -> i32 {
    unsafe {
        out.write(Box::into_raw(Box::new(42_u64)).cast());
    }
    0
}
unsafe extern "C" fn destroy(object: *mut c_void) -> i32 {
    unsafe {
        drop(Box::from_raw(object.cast::<u64>()));
    }
    DESTROYED.fetch_add(1, Ordering::SeqCst);
    0
}
unsafe extern "C" fn get(object: *mut c_void, _: *const c_void, out: *mut c_void) -> i32 {
    unsafe {
        out.cast::<u64>().write(*object.cast::<u64>());
    }
    0
}

fn interface() -> abi::InterfaceDescriptor {
    let methods = Box::leak(Box::new([abi::Method {
        signature: abi::Bytes::from_static(SPEC.signatures[0].as_bytes()),
        call: Some(get),
    }]));
    abi::InterfaceDescriptor {
        size: size_of::<abi::InterfaceDescriptor>() as u32,
        version: 1,
        id: abi::Bytes::from_static(b"test.counter"),
        method_count: 1,
        methods: methods.as_ptr(),
        create: Some(create),
        destroy: Some(destroy),
    }
}

fn descriptor(interface: abi::InterfaceDescriptor) -> &'static abi::PluginDescriptor {
    let interface = Box::leak(Box::new(interface));
    let interfaces = Box::leak(Box::new([interface as *const _]));
    Box::leak(Box::new(abi::PluginDescriptor {
        header: abi::Header {
            magic: abi::MAGIC,
            major: 1,
            minor: 0,
            size: size_of::<abi::PluginDescriptor>() as u32,
        },
        plugin_version: abi::Version {
            major: 9,
            minor: 3,
            patch: 2,
        },
        flags: 0,
        interface_count: 1,
        interfaces: interfaces.as_ptr(),
        quiesce: None,
    }))
}

#[test]
fn bind_create_call_destroy_and_exact_version() {
    let plugin = unsafe { Plugin::from_static(&descriptor(interface()).header) }.unwrap();
    assert_eq!(plugin.version().major, 9);
    let binding = plugin.bind(SPEC).unwrap();
    let before = DESTROYED.load(Ordering::SeqCst);
    let object = binding.create().unwrap();
    let mut answer = 0_u64;
    status_result(unsafe {
        binding.method(0)(
            object.handle(),
            core::ptr::null(),
            (&mut answer as *mut u64).cast(),
        )
    })
    .unwrap();
    assert_eq!(answer, 42);
    drop(object);
    assert_eq!(DESTROYED.load(Ordering::SeqCst), before + 1);
    assert!(matches!(
        plugin.bind(InterfaceSpec { version: 2, ..SPEC }),
        Err(Error::InterfaceVersion {
            expected: 2,
            actual: 1
        })
    ));
    assert!(matches!(
        plugin.bind(InterfaceSpec {
            id: "absent",
            ..SPEC
        }),
        Err(Error::InterfaceNotFound)
    ));
}

#[test]
fn protocol_prefix_checked_before_extended_reads() {
    for (magic, major, size, expected) in [
        (0, 1, 16, Error::BadMagic),
        (abi::MAGIC, 9, 16, Error::AbiVersion { found: 9 }),
        (
            abi::MAGIC,
            1,
            16,
            Error::DescriptorTooShort {
                expected: size_of::<abi::PluginDescriptor>() as u32,
                actual: 16,
            },
        ),
    ] {
        let header = Box::leak(Box::new(abi::Header {
            magic,
            major,
            minor: 0,
            size,
        }));
        assert!(matches!(unsafe { Plugin::from_static(header) }, Err(error) if error == expected));
    }
}

#[test]
fn interface_length_signature_and_presence_checked() {
    let mut short = interface();
    short.size = 4;
    let plugin = unsafe { Plugin::from_static(&descriptor(short).header) }.unwrap();
    assert!(matches!(
        plugin.bind(SPEC),
        Err(Error::DescriptorTooShort { actual: 4, .. })
    ));
    let mut wrong = interface();
    wrong.methods = Box::leak(Box::new([abi::Method {
        signature: abi::Bytes::from_static(b"different"),
        call: Some(get),
    }]))
    .as_ptr();
    let plugin = unsafe { Plugin::from_static(&descriptor(wrong).header) }.unwrap();
    assert!(matches!(
        plugin.bind(SPEC),
        Err(Error::SignatureMismatch { method: 0 })
    ));
    let mut absent = interface();
    absent.methods = Box::leak(Box::new([abi::Method {
        signature: abi::Bytes::from_static(SPEC.signatures[0].as_bytes()),
        call: None,
    }]))
    .as_ptr();
    let plugin = unsafe { Plugin::from_static(&descriptor(absent).header) }.unwrap();
    assert!(matches!(
        plugin.bind(SPEC),
        Err(Error::MissingMethod { method: 0 })
    ));
    let mut count = interface();
    count.method_count = 0;
    let plugin = unsafe { Plugin::from_static(&descriptor(count).header) }.unwrap();
    assert!(matches!(
        plugin.bind(SPEC),
        Err(Error::MissingMethod { method: 0 })
    ));
}

#[test]
fn empty_ranges_and_overlaps() {
    assert!(
        unsafe {
            abi::Bytes {
                ptr: core::ptr::null(),
                len: 0,
            }
            .as_slice()
        }
        .unwrap()
        .is_empty()
    );
    assert!(
        unsafe {
            abi::BytesMut {
                ptr: core::ptr::null_mut(),
                len: 0,
            }
            .as_mut_slice()
        }
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        unsafe {
            abi::Bytes {
                ptr: core::ptr::null(),
                len: 1,
            }
            .as_slice()
        },
        Err(Error::InvalidArgument)
    );
    let data = [0_u8; 8];
    let read = export::Borrow {
        ptr: data.as_ptr(),
        len: 8,
        mutable: false,
    };
    let write = export::Borrow {
        mutable: true,
        ..read
    };
    assert!(export::validate_borrows(&[read, read]).is_ok());
    assert_eq!(
        export::validate_borrows(&[read, write]),
        Err(Error::InvalidArgument)
    );
    assert_eq!(
        export::validate_borrows(&[export::Borrow {
            ptr: usize::MAX as *const u8,
            len: 2,
            mutable: false
        }]),
        Err(Error::InvalidArgument)
    );
}

#[test]
fn reentry_rejected_before_second_mutable_borrow() {
    let mut object = export::Object::new(7_u64);
    let raw = (&mut object as *mut export::Object<u64>).cast();
    unsafe {
        export::with_object::<u64, _>(raw, |value| {
            assert_eq!(
                export::with_object::<u64, _>(raw, |_| Ok(())),
                Err(Error::Busy)
            );
            *value += 1;
            Ok(())
        })
    }
    .unwrap();
    assert_eq!(
        unsafe { export::with_object::<u64, _>(raw, |value| Ok(*value)) }.unwrap(),
        8
    );
}

#[test]
fn status_codes_preserve_unknown_values_without_enum_transmute() {
    for code in [-123456, -7, -6, -5, -4, -3, -2, -1, 1, 200, i32::MAX] {
        assert_eq!(status_result(code).unwrap_err().status(), code);
    }
    assert_eq!(status_result(0), Ok(()));
    assert_ne!(Error::PluginStatus(0).status(), 0);
}

#[cfg(feature = "std")]
#[test]
fn unwind_poisoning_is_per_object() {
    let mut object = export::Object::new(0_u64);
    let raw = (&mut object as *mut export::Object<u64>).cast();
    assert_eq!(
        unsafe { export::with_object::<u64, ()>(raw, |_| panic!("controlled panic")) },
        Err(Error::Panicked)
    );
    assert_eq!(
        unsafe { export::with_object::<u64, _>(raw, |_| Ok(())) },
        Err(Error::Poisoned)
    );
    assert_eq!(
        export::guard(|| panic!("entry boundary")),
        Error::Panicked.status()
    );
}
