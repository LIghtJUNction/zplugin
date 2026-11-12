#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "macros", doc = include_str!("../README.md"))]
//! Native plugin primitives with a frozen C ABI and explicit library lifetimes.
//!
//! The default feature set needs neither `std` nor an allocator. Enable `macros`
//! for generated interfaces, `alloc` for owned library leases and plugin factories,
//! `std` for path conveniences and panic containment, and `hot-reload` for generations.
//! Loading is unsafe: native code must obey the ABI and ownership contracts.

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

pub mod abi;
pub mod bindings;
pub mod callback;
pub mod export;
#[cfg(feature = "hot-reload")]
pub mod hot;
pub mod loader;
pub mod resource;
mod runtime;
mod status;
pub mod task;
pub mod value;

pub use callback::{Callback, CallbackRef};
pub use resource::{OwnedResource, Resource, ResourceHandler};
pub use runtime::{Binding, Buffer, Instance, InterfaceSpec, OwnedBytes, Plugin, UnloadPolicy};
pub use status::{Error, Result, status_result};
pub use task::{OwnedTask, Task};
pub use value::AbiValue;
#[cfg(feature = "macros")]
pub use zplugin_macros::{abi_type, interface};
