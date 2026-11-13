//! A no_std, allocation-free guest. Operations 3–6 exercise sandbox failures.
#![no_std]

use zplugin::{Error, Result, wasm_guest::Guest};

struct Demo {
    calls: u64,
}

impl Demo {
    const fn new() -> Self {
        Self { calls: 0 }
    }
}

impl Guest for Demo {
    fn call(&mut self, operation: u32, input: &[u8], output: &mut [u8]) -> Result<usize> {
        match operation {
            0 | 1 => {
                let destination = output.get_mut(..input.len()).ok_or(Error::BufferTooSmall)?;
                destination.copy_from_slice(input);
                if operation == 1 {
                    destination.make_ascii_uppercase();
                }
                Ok(input.len())
            }
            2 => {
                let destination = output.get_mut(..8).ok_or(Error::BufferTooSmall)?;
                self.calls = self.calls.wrapping_add(1);
                destination.copy_from_slice(&self.calls.to_le_bytes());
                Ok(8)
            }
            3 => {
                output.fill(0xcc);
                Err(Error::PluginStatus(77))
            }
            4 => loop {
                core::hint::spin_loop();
            },
            5 => core::arch::wasm32::unreachable(),
            6 => {
                let destination = output.get_mut(..4).ok_or(Error::BufferTooSmall)?;
                // This intentionally exceeds the host's guest-memory ceiling.
                let previous_pages = core::arch::wasm32::memory_grow::<0>(65_536);
                destination.copy_from_slice(&(previous_pages as u32).to_le_bytes());
                Ok(4)
            }
            _ => Err(Error::Unsupported),
        }
    }
}

zplugin::export_wasm!(
    Demo,
    constructor = Demo::new(),
    interface = 0x7a70_6c75_6769_6e01_u64,
    version = 1,
    input = 1_048_576,
    output = 1_048_576
);

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}
