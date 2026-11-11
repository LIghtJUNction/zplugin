#![cfg(all(feature = "macros", feature = "alloc"))]

use core::ffi::c_void;
use zplugin::{Error, abi, export::ExportInterface};

#[zplugin::interface(id = "tests.scalars", version = 1)]
trait Scalars {
    fn uint8(&mut self, value: u8) -> zplugin::Result<u8>;
    fn uint16(&mut self, value: u16) -> zplugin::Result<u16>;
    fn uint32(&mut self, value: u32) -> zplugin::Result<u32>;
    fn uint64(&mut self, value: u64) -> zplugin::Result<u64>;
    fn int8(&mut self, value: i8) -> zplugin::Result<i8>;
    fn int16(&mut self, value: i16) -> zplugin::Result<i16>;
    fn int32(&mut self, value: i32) -> zplugin::Result<i32>;
    fn int64(&mut self, value: i64) -> zplugin::Result<i64>;
    fn float32(&mut self, value: f32) -> zplugin::Result<f32>;
    fn float64(&mut self, value: f64) -> zplugin::Result<f64>;
}

struct Identity;
macro_rules! identity_methods {
    ($($name:ident: $ty:ty),* $(,)?) => {
        $(fn $name(&mut self, value: $ty) -> zplugin::Result<$ty> { Ok(value) })*
    };
}
impl Scalars for Identity {
    identity_methods!(
        uint8: u8, uint16: u16, uint32: u32, uint64: u64,
        int8: i8, int16: i16, int32: i32, int64: i64,
        float32: f32, float64: f64,
    );
}
struct IdentityFactory;
impl zplugin::export::Factory for IdentityFactory {
    type Object = Identity;
    fn create() -> zplugin::Result<Identity> {
        Ok(Identity)
    }
}
static SCALARS: abi::PluginDescriptor = zplugin::export::Registration::<
    ScalarsExport<Identity>,
    IdentityFactory,
>::pinned(abi::Version {
    major: 1,
    minor: 0,
    patch: 0,
});

#[test]
fn every_fixed_width_scalar_round_trips_through_proxy() {
    // SAFETY: immutable static metadata points to adapters defined in this executable.
    let plugin = unsafe { zplugin::Plugin::from_static(&SCALARS.header).unwrap() };
    let mut object = ScalarsApi::create(&plugin).unwrap();
    assert_eq!(object.uint8(u8::MAX), Ok(u8::MAX));
    assert_eq!(object.uint16(u16::MAX), Ok(u16::MAX));
    assert_eq!(object.uint32(u32::MAX), Ok(u32::MAX));
    assert_eq!(object.uint64(u64::MAX), Ok(u64::MAX));
    assert_eq!(object.int8(i8::MIN), Ok(i8::MIN));
    assert_eq!(object.int16(i16::MIN), Ok(i16::MIN));
    assert_eq!(object.int32(i32::MIN), Ok(i32::MIN));
    assert_eq!(object.int64(i64::MIN), Ok(i64::MIN));
    assert_eq!(
        object.float32(-0.0).unwrap().to_bits(),
        (-0.0_f32).to_bits()
    );
    let nan = f64::from_bits(0x7ff8_1234_5678_9abc);
    assert_eq!(object.float64(nan).unwrap().to_bits(), nan.to_bits());
}

#[zplugin::interface(id = "tests.raw-borrows", version = 1)]
trait Borrowed {
    fn copy(&mut self, input: &[u8], output: &mut [u8], label: &str) -> zplugin::Result<u64>;
    fn outputs(&mut self, first: &mut [u8], second: &mut [u8]) -> zplugin::Result<()>;
}

struct CopyBytes;
impl Borrowed for CopyBytes {
    fn copy(&mut self, input: &[u8], output: &mut [u8], label: &str) -> zplugin::Result<u64> {
        if output.len() < input.len() {
            return Err(Error::BufferTooSmall);
        }
        output[..input.len()].copy_from_slice(input);
        Ok((input.len() + label.len()) as u64)
    }

    fn outputs(&mut self, first: &mut [u8], second: &mut [u8]) -> zplugin::Result<()> {
        first.fill(1);
        second.fill(2);
        Ok(())
    }
}

// ABI records deliberately mirror the public v1 protocol rather than accessing
// generated private records: these tests exercise a foreign caller's obligations.
#[repr(C)]
struct CopyArgs {
    input: abi::Bytes,
    output: abi::BytesMut,
    label: abi::Bytes,
}

fn empty_args() -> CopyArgs {
    CopyArgs {
        input: abi::Bytes {
            ptr: core::ptr::null(),
            len: 0,
        },
        output: abi::BytesMut {
            ptr: core::ptr::null_mut(),
            len: 0,
        },
        label: abi::Bytes {
            ptr: core::ptr::null(),
            len: 0,
        },
    }
}

unsafe fn copy(
    object: &mut zplugin::export::Object<CopyBytes>,
    args: *const CopyArgs,
    out: *mut u64,
) -> i32 {
    // SAFETY: callers supply initialized readable argument records unless checking
    // a structural rejection that happens before any dereference.
    unsafe {
        BorrowedExport::<CopyBytes>::METHODS[0].call.unwrap()(
            (object as *mut zplugin::export::Object<CopyBytes>).cast(),
            args.cast(),
            out.cast(),
        )
    }
}

#[test]
fn empty_null_ranges_are_accepted() {
    let mut object = zplugin::export::Object::new(CopyBytes);
    let args = empty_args();
    let mut output = 123;
    assert_eq!(unsafe { copy(&mut object, &args, &mut output) }, 0);
    assert_eq!(output, 0);
}

#[test]
fn malformed_raw_inputs_preserve_scalar_output() {
    let mut object = zplugin::export::Object::new(CopyBytes);
    let mut args = empty_args();
    let mut output = 123;
    args.input.len = 1;
    assert_eq!(
        unsafe { copy(&mut object, &args, &mut output) },
        Error::InvalidArgument.status()
    );
    assert_eq!(output, 123);
    args.input.len = 0;
    args.label = abi::Bytes::from_static(&[0xff]);
    assert_eq!(
        unsafe { copy(&mut object, &args, &mut output) },
        Error::InvalidUtf8.status()
    );
    assert_eq!(output, 123);
    args.label = abi::Bytes::from_static(b"");
    args.input = abi::Bytes::from_static(b"more than zero");
    assert_eq!(
        unsafe { copy(&mut object, &args, &mut output) },
        Error::BufferTooSmall.status()
    );
    assert_eq!(output, 123);
}

#[test]
fn aliasing_is_rejected_before_constructing_borrows() {
    let mut object = zplugin::export::Object::new(CopyBytes);
    let mut bytes = [7_u8; 16];
    let mut output = 123;
    let mut args = empty_args();
    args.input = abi::Bytes {
        ptr: bytes.as_ptr(),
        len: 8,
    };
    args.output = abi::BytesMut {
        ptr: bytes.as_mut_ptr().wrapping_add(4),
        len: 8,
    };
    assert_eq!(
        unsafe { copy(&mut object, &args, &mut output) },
        Error::InvalidArgument.status()
    );
    assert_eq!(bytes, [7; 16]);
    assert_eq!(output, 123);

    args = empty_args();
    let args_ptr = &mut args as *mut CopyArgs;
    // An out value may not overwrite the argument record during a call.
    assert_eq!(
        unsafe { copy(&mut object, args_ptr, args_ptr.cast()) },
        Error::InvalidArgument.status()
    );
    // A caller-provided byte output may not alias the argument record either.
    args.output = abi::BytesMut {
        ptr: args_ptr.cast(),
        len: 8,
    };
    assert_eq!(
        unsafe { copy(&mut object, &args, &mut output) },
        Error::InvalidArgument.status()
    );
}

#[test]
fn null_and_misaligned_record_pointers_are_rejected() {
    let mut object = zplugin::export::Object::new(CopyBytes);
    let args = empty_args();
    let mut output = 123;
    assert_eq!(
        unsafe { copy(&mut object, core::ptr::null(), &mut output) },
        Error::InvalidArgument.status()
    );
    assert_eq!(
        unsafe { copy(&mut object, &args, core::ptr::null_mut()) },
        Error::InvalidArgument.status()
    );
    let storage = [0_u64; 16];
    let misaligned = storage
        .as_ptr()
        .cast::<u8>()
        .wrapping_add(1)
        .cast::<CopyArgs>();
    assert_eq!(
        unsafe { copy(&mut object, misaligned, &mut output) },
        Error::InvalidArgument.status()
    );
}

#[test]
fn distinct_mutable_ranges_and_null_unit_output() {
    #[repr(C)]
    struct OutputsArgs {
        first: abi::BytesMut,
        second: abi::BytesMut,
    }
    let mut object = zplugin::export::Object::new(CopyBytes);
    let mut bytes = [0_u8; 8];
    let mut args = OutputsArgs {
        first: abi::BytesMut {
            ptr: bytes.as_mut_ptr(),
            len: 4,
        },
        second: abi::BytesMut {
            ptr: bytes.as_mut_ptr().wrapping_add(4),
            len: 4,
        },
    };
    let call = BorrowedExport::<CopyBytes>::METHODS[1].call.unwrap();
    let object = (&mut object as *mut zplugin::export::Object<CopyBytes>).cast::<c_void>();
    assert_eq!(
        unsafe {
            call(
                object,
                (&args as *const OutputsArgs).cast(),
                core::ptr::null_mut(),
            )
        },
        0
    );
    assert_eq!(bytes, [1, 1, 1, 1, 2, 2, 2, 2]);
    args.second.ptr = args.first.ptr;
    assert_eq!(
        unsafe {
            call(
                object,
                (&args as *const OutputsArgs).cast(),
                core::ptr::null_mut(),
            )
        },
        Error::InvalidArgument.status()
    );
}
