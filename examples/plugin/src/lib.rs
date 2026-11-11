//! Compile this crate independently; the host links only demo-interface.

use demo_interface::{Demo, DemoExport, SNAPSHOT_SCHEMA};
use zplugin::{Error, OwnedBytes, Result};

#[cfg(feature = "instrument")]
mod measurements;

struct Processor {
    calls: u64,
}

impl Demo for Processor {
    fn add(&mut self, a: u64, b: u64) -> Result<u64> {
        self.calls = self.calls.wrapping_add(1);
        Ok(a.wrapping_add(b))
    }

    fn transform(&mut self, input: &[u8], output: &mut [u8]) -> Result<u64> {
        if output.len() < input.len() {
            return Err(Error::BufferTooSmall);
        }
        for (source, target) in input.iter().zip(output.iter_mut()) {
            *target = if cfg!(feature = "v2") {
                source.to_ascii_lowercase()
            } else {
                source.to_ascii_uppercase()
            };
        }
        self.calls = self.calls.wrapping_add(1);
        Ok(input.len() as u64)
    }

    fn text(&self, input: &str) -> Result<u64> {
        Ok(input.chars().count() as u64)
    }

    fn snapshot(&self, output: &mut [u8]) -> Result<u64> {
        if output.len() < 8 {
            return Err(Error::BufferTooSmall);
        }
        output[..8].copy_from_slice(&self.calls.to_le_bytes());
        Ok(8)
    }

    fn restore(&mut self, schema: u32, input: &[u8]) -> Result<()> {
        if schema != SNAPSHOT_SCHEMA || input.len() != 8 {
            return Err(Error::InvalidArgument);
        }
        self.calls = u64::from_le_bytes(input.try_into().map_err(|_| Error::InvalidArgument)?);
        Ok(())
    }

    fn owned(&self, input: &[u8]) -> Result<OwnedBytes> {
        Ok(OwnedBytes::from_vec(input.to_vec()))
    }

    fn fail(&self, code: i32) -> Result<()> {
        if code == 0 {
            Ok(())
        } else {
            Err(Error::PluginStatus(code))
        }
    }

    fn panic_now(&mut self) -> Result<()> {
        panic!("intentional plugin panic for the lifecycle test")
    }
}

fn create() -> Result<Processor> {
    if cfg!(feature = "fail-init") {
        Err(Error::PluginStatus(1001))
    } else {
        Ok(Processor { calls: 0 })
    }
}

// SAFETY: Processor owns no background threads, tasks, callbacks, TLS resources
// or external registrations. Every owned byte buffer uses the plugin releaser.
unsafe impl zplugin::export::UnloadSafe for Processor {}

zplugin::export!(
    DemoExport<Processor>,
    constructor = create,
    version = (1, cfg!(feature = "v2") as u32, 0),
    unload = cooperative
);

/// Benchmark symbol containing the exact generated adapter function pointer.
/// Loading it once allows libloading to exercise the same call and lifetime
/// contract as the generated proxy, without adding a forwarding function.
#[unsafe(no_mangle)]
pub static demo_add_function: zplugin::abi::Call =
    match <DemoExport<Processor> as zplugin::export::ExportInterface>::METHODS[0].call {
        Some(call) => call,
        None => panic!("generated method exists"),
    };

#[unsafe(no_mangle)]
pub static demo_transform_function: zplugin::abi::Call =
    match <DemoExport<Processor> as zplugin::export::ExportInterface>::METHODS[1].call {
        Some(call) => call,
        None => panic!("generated method exists"),
    };
