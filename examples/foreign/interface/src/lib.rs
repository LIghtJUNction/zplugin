#![no_std]
//! One interface crate generates matching C, C++, and Zig declarations.

#[zplugin::abi_type(id = "foreign.point", version = 1)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[zplugin::abi_type(id = "foreign.mode", version = 1)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Sum = 1,
    Product = 2,
}

#[zplugin::abi_type(id = "foreign.event", version = 1)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Empty,
    Number { value: u64 },
    Position { point: Point },
}

#[zplugin::interface(id = "dev.project-z.foreign", version = 1)]
pub trait Foreign {
    fn calculate(&mut self, point: Point, mode: Mode) -> zplugin::Result<i64>;
    fn echo(&mut self, value: Event) -> zplugin::Result<Event>;
    fn transform(&mut self, input: &[u8], output: &mut [u8]) -> zplugin::Result<u64>;
    fn owned(&mut self, input: &[u8]) -> zplugin::Result<zplugin::OwnedBytes>;
    fn start(&mut self, seed: u64) -> zplugin::Result<zplugin::OwnedTask<u64>>;
    fn resource(&mut self) -> zplugin::Result<zplugin::OwnedResource>;
    fn callback(
        &mut self,
        callback: zplugin::CallbackRef<'_>,
        input: &[u8],
        output: &mut [u8],
    ) -> zplugin::Result<u64>;
}
