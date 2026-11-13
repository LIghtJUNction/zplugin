#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "macros", doc = include_str!("../README.md"))]
//! Native plugin primitives with a frozen C ABI and explicit library lifetimes.
//!
//! The default feature set needs neither `std` nor an allocator. Enable `macros`
//! for generated interfaces, `alloc` for owned library leases and plugin factories,
//! `std` for path conveniences and panic containment, and `hot-reload` for generations.
//! Native loading is unsafe: native code must obey ABI and ownership contracts.
//! The separate `wasm` feature provides an import-free, fuel-metered sandbox for
//! untrusted WebAssembly, with an optional Wasmi runtime dependency (Rust 1.86+).
//! `wasm_guest` and `export_wasm!` are allocator-free and need no engine feature.

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
#[cfg(feature = "wasm")]
pub mod wasm;
pub mod wasm_guest;

pub use callback::{Callback, CallbackRef};
pub use resource::{OwnedResource, Resource, ResourceHandler};
pub use runtime::{Binding, Buffer, Instance, InterfaceSpec, OwnedBytes, Plugin, UnloadPolicy};
pub use status::{Error, Result, status_result};
pub use task::{OwnedTask, Task};
pub use value::AbiValue;
#[cfg(feature = "macros")]
pub use zplugin_macros::{abi_type, interface};
