use core::fmt;

/// Errors are Rust values locally; the wire contains only an `i32` status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    InvalidArgument,
    InvalidUtf8,
    BufferTooSmall,
    Busy,
    Panicked,
    Poisoned,
    Unsupported,
    Cancelled,
    Completed,
    LoadFailed,
    MissingEntry,
    BadMagic,
    AbiVersion { found: u16 },
    DescriptorTooShort { expected: u32, actual: u32 },
    InterfaceNotFound,
    InterfaceVersion { expected: u32, actual: u32 },
    SignatureMismatch { method: u32 },
    MissingMethod { method: u32 },
    InvalidDescriptor,
    NullObject,
    ResidentLimit,
    SamePath,
    PluginStatus(i32),
}

pub type Result<T> = core::result::Result<T, Error>;

impl Error {
    /// Stable v1 status. Application-specific errors should use positive codes.
    pub const fn status(self) -> i32 {
        match self {
            Self::InvalidArgument => -1,
            Self::InvalidUtf8 => -2,
            Self::BufferTooSmall => -3,
            Self::Busy => -4,
            Self::Panicked => -5,
            Self::Poisoned => -6,
            Self::Unsupported => -7,
            Self::Cancelled => -8,
            Self::Completed => -9,
            Self::PluginStatus(n) if n != 0 => n,
            _ => -1,
        }
    }
}

/// Convert any nonzero status without constructing an unchecked Rust enum.
pub const fn status_result(status: i32) -> Result<()> {
    match status {
        0 => Ok(()),
        -1 => Err(Error::InvalidArgument),
        -2 => Err(Error::InvalidUtf8),
        -3 => Err(Error::BufferTooSmall),
        -4 => Err(Error::Busy),
        -5 => Err(Error::Panicked),
        -6 => Err(Error::Poisoned),
        -7 => Err(Error::Unsupported),
        -8 => Err(Error::Cancelled),
        -9 => Err(Error::Completed),
        n => Err(Error::PluginStatus(n)),
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoadFailed => f.write_str("the platform loader could not open the library"),
            Self::MissingEntry => f.write_str("missing zplugin_entry_v1 symbol"),
            Self::AbiVersion { found } => write!(f, "unsupported ABI major {found}; expected 1"),
            Self::DescriptorTooShort { expected, actual } => {
                write!(f, "descriptor has {actual} bytes; requires {expected}")
            }
            Self::InterfaceVersion { expected, actual } => {
                write!(f, "interface version {actual}; expected {expected}")
            }
            Self::SignatureMismatch { method } => write!(f, "method {method} signature differs"),
            Self::MissingMethod { method } => write!(f, "method {method} is absent"),
            Self::PluginStatus(n) => write!(f, "plugin returned status {n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
