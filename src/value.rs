//! Fixed-layout values with checked conversion at the native boundary.
use crate::Result;
use core::fmt;

/// A value with an explicit, recursively described C representation.
///
/// Prefer `#[zplugin::abi_type(id = "application.point", version = 1)]` to manual
/// implementations. The Rust value itself never crosses the library boundary.
///
/// # Safety
/// `Wire` must have a target-C-compatible layout without Rust references, owned
/// allocations, or native enum discriminants. Every initialized bit pattern in
/// readable wire fields must be valid (integer tags are checked after reading).
/// `from_wire` must validate every
/// discriminant before interpreting its payload. `SCHEMA` must describe the
/// complete representation, including nested types, field order, and enum tags.
/// Distinct incompatible representations must have distinct schemas. Neither
/// conversion may expose borrowed storage. `write_c` must describe exactly Wire.
pub unsafe trait AbiValue: Copy + 'static {
    type Wire: Copy;
    const SCHEMA: &'static str;
    const C_TYPE: &'static str;
    fn into_wire(self) -> Self::Wire;
    fn from_wire(wire: Self::Wire) -> Result<Self>;
    fn write_c(writer: &mut impl fmt::Write) -> fmt::Result;
}

macro_rules! scalar {
    ($($rust:ty => $c:literal),* $(,)?) => {$ (
        // SAFETY: fixed-width numbers have the corresponding target C layout;
        // every bit pattern is valid and conversion is the identity.
        unsafe impl AbiValue for $rust {
            type Wire = Self;
            const SCHEMA: &'static str = stringify!($rust);
            const C_TYPE: &'static str = $c;
            fn into_wire(self) -> Self { self }
            fn from_wire(wire: Self) -> Result<Self> { Ok(wire) }
            fn write_c(_: &mut impl fmt::Write) -> fmt::Result { Ok(()) }
        }
    )*};
}

scalar! {
    u8 => "uint8_t", u16 => "uint16_t", u32 => "uint32_t", u64 => "uint64_t",
    i8 => "int8_t", i16 => "int16_t", i32 => "int32_t", i64 => "int64_t",
    f32 => "float", f64 => "double",
}

// SAFETY: a unit result writes no output bytes; the C typedef is only a placeholder.
unsafe impl AbiValue for () {
    type Wire = ();
    const SCHEMA: &'static str = "unit";
    const C_TYPE: &'static str = "uint8_t";
    fn into_wire(self) {}
    fn from_wire(_: ()) -> Result<Self> {
        Ok(())
    }
    fn write_c(_: &mut impl fmt::Write) -> fmt::Result {
        Ok(())
    }
}

/// Exact-size compile-time concatenation used by generated schema strings.
/// This produces no runtime allocation or fixed-capacity metadata slack.
#[doc(hidden)]
pub const fn concat<const N: usize>(parts: &[&str]) -> [u8; N] {
    let mut result = [0; N];
    let mut offset = 0;
    let mut part = 0;
    while part < parts.len() {
        let bytes = parts[part].as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            assert!(offset < N, "zplugin schema length mismatch");
            result[offset] = bytes[index];
            offset += 1;
            index += 1;
        }
        part += 1;
    }
    assert!(offset == N, "zplugin schema length mismatch");
    result
}

/// Write a byte-exact C string literal without allocating.
#[doc(hidden)]
pub fn write_c_string(writer: &mut impl fmt::Write, value: &str) -> fmt::Result {
    writer.write_char('"')?;
    for byte in value.bytes() {
        match byte {
            b'"' => writer.write_str("\\\"")?,
            b'\\' => writer.write_str("\\\\")?,
            b' '..=b'~' => writer.write_char(byte as char)?,
            _ => write!(writer, "\\{byte:03o}")?,
        }
    }
    writer.write_char('"')
}
