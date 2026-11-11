#![no_std]

//! The only crate shared by the host and independently compiled plugin.

/// A versioned byte processor. All inputs live for the duration of a call.
#[zplugin::interface(id = "dev.project-z.demo", version = 1)]
pub trait Demo {
    fn add(&mut self, a: u64, b: u64) -> Result<u64, zplugin::Error>;
    fn transform(&mut self, input: &[u8], output: &mut [u8]) -> Result<u64, zplugin::Error>;
    fn text(&self, input: &str) -> Result<u64, zplugin::Error>;
    fn snapshot(&self, output: &mut [u8]) -> Result<u64, zplugin::Error>;
    fn restore(&mut self, schema: u32, input: &[u8]) -> Result<(), zplugin::Error>;
    fn owned(&self, input: &[u8]) -> Result<zplugin::OwnedBytes, zplugin::Error>;
    fn fail(&self, code: i32) -> Result<(), zplugin::Error>;
    fn panic_now(&mut self) -> Result<(), zplugin::Error>;
}

/// Snapshot schema is independent of interface, crate and plugin versions.
pub const SNAPSHOT_SCHEMA: u32 = 1;
