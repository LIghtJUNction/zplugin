#![no_std]

/// Immediate, worker-driven, business-error, and panic paths in the example.
pub const READY: u32 = 0;
pub const WORKER: u32 = 1;
pub const APPLICATION_ERROR: u32 = 2;
pub const PANIC: u32 = 3;

/// Starting a task is synchronous; polling, cancellation, and completion cross
/// the task ABI. Inputs retained by a task must be owned before start returns.
#[zplugin::interface(id = "dev.project-z.async.example", version = 1)]
pub trait AsyncService {
    fn start(&mut self, mode: u32, value: u64) -> zplugin::Result<zplugin::OwnedTask<u64>>;
    fn signal(&mut self) -> zplugin::Result<()>;
    fn workers(&self) -> zplugin::Result<u32>;
}
