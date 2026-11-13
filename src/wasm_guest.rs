//! Allocator-free guest SDK for the separate zplugin Wasm byte-buffer ABI v1.
//!
//! Build for `wasm32-unknown-unknown`; no host engine dependency is needed.
//! This protocol does not expose the native C ABI, pointers, tasks or callbacks.

/// The frozen Wasm protocol version, independent of application interface versions.
pub const ABI_VERSION: u64 = 1;
/// Export queried during binding: `(field: i32) -> i64`.
pub const INFO_EXPORT: &str = "zplugin_wasm_info_v1";
/// Cached call export: `(operation: i32, input_len: i32, output_capacity: i32) -> i64`.
pub const CALL_EXPORT: &str = "zplugin_wasm_call_v1";

/// A synchronous operation on instance-local buffers.
///
/// Arguments are borrowed only for this call. Return the initialized output length
/// on success. The export adapter checks it before reporting success to the host.
pub trait Guest {
    fn call(&mut self, operation: u32, input: &[u8], output: &mut [u8]) -> crate::Result<usize>;
}

/// Encode the v1 result: signed status in the low word, output length in the high word.
pub const fn result(status: i32, written: u32) -> u64 {
    ((written as u64) << 32) | (status as u32 as u64)
}

/// Export a no-allocator Wasm plugin with fixed, disjoint scratch buffers.
///
/// `constructor` must be a const expression. The instance owns its state and
/// buffers. The host serializes calls; the adapter also rejects same-instance
/// reentrancy. Panics must abort/trap inside the guest; they never unwind into
/// the host. There are no automatic imports, start function, or guest destructor.
///
/// ```ignore
/// zplugin::export_wasm!(Counter, constructor = Counter::new(),
///     interface = 0x1234_u64, version = 1, input = 4096, output = 4096);
/// ```
#[macro_export]
macro_rules! export_wasm {
    ($implementation:ty, constructor = $constructor:expr,
        interface = $id:expr, version = $version:expr,
        input = $input:expr, output = $output:expr $(,)?) => {
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        compile_error!("export_wasm! requires the wasm32-unknown-unknown target");

        const _: () = {
            assert!($input <= u32::MAX as usize);
            assert!($output <= u32::MAX as usize);
        };

        static mut __ZPLUGIN_WASM_STATE: $implementation = $constructor;
        static mut __ZPLUGIN_WASM_INPUT: [u8; $input] = [0; $input];
        static mut __ZPLUGIN_WASM_OUTPUT: [u8; $output] = [0; $output];
        static mut __ZPLUGIN_WASM_BUSY: bool = false;

        #[unsafe(no_mangle)]
        pub extern "C" fn zplugin_wasm_info_v1(field: u32) -> u64 {
            match field {
                0 => $crate::wasm_guest::ABI_VERSION,
                1 => {
                    const ID: u64 = $id;
                    ID
                }
                2 => {
                    const VERSION: u32 = $version;
                    VERSION as u64
                }
                3 => core::ptr::addr_of_mut!(__ZPLUGIN_WASM_INPUT) as usize as u64,
                4 => $input as u64,
                5 => core::ptr::addr_of_mut!(__ZPLUGIN_WASM_OUTPUT) as usize as u64,
                6 => $output as u64,
                _ => u64::MAX,
            }
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn zplugin_wasm_call_v1(
            operation: u32,
            input_len: u32,
            output_capacity: u32,
        ) -> u64 {
            if input_len as usize > $input || output_capacity as usize > $output {
                return $crate::wasm_guest::result($crate::Error::InvalidArgument.status(), 0);
            }
            // SAFETY: the supported guest is single-threaded, with no host imports.
            // BUSY rejects reentry before forming references. State and scratch
            // arrays are separate statics; lengths were checked above. A guest
            // trap poisons the host instance, so an interrupted call is not reused.
            unsafe {
                if __ZPLUGIN_WASM_BUSY {
                    return $crate::wasm_guest::result($crate::Error::Busy.status(), 0);
                }
                __ZPLUGIN_WASM_BUSY = true;
                let state = &mut *core::ptr::addr_of_mut!(__ZPLUGIN_WASM_STATE);
                let input = core::slice::from_raw_parts(
                    core::ptr::addr_of!(__ZPLUGIN_WASM_INPUT).cast::<u8>(),
                    input_len as usize,
                );
                let output = core::slice::from_raw_parts_mut(
                    core::ptr::addr_of_mut!(__ZPLUGIN_WASM_OUTPUT).cast::<u8>(),
                    output_capacity as usize,
                );
                let result = <$implementation as $crate::wasm_guest::Guest>::call(
                    state, operation, input, output,
                );
                __ZPLUGIN_WASM_BUSY = false;
                match result {
                    Ok(n) if n <= output_capacity as usize => {
                        $crate::wasm_guest::result(0, n as u32)
                    }
                    Ok(_) => $crate::wasm_guest::result($crate::Error::InvalidArgument.status(), 0),
                    Err(error) => $crate::wasm_guest::result(error.status(), 0),
                }
            }
        }
    };
}
