#![cfg(all(feature = "macros", feature = "alloc"))]

use std::sync::atomic::{AtomicUsize, Ordering};
use zplugin::{Error, Plugin, abi};

#[zplugin::interface(id = "tests.counter", version = 1)]
trait Counter {
    fn add(&mut self, increment: u64) -> zplugin::Result<u64>;
}

#[zplugin::interface(id = "tests.info", version = 7)]
trait Info {
    fn label(&self, output: &mut [u8]) -> zplugin::Result<u32>;
}

static DROPPED: AtomicUsize = AtomicUsize::new(0);
struct CounterObject(u64);
struct InfoObject;

impl Counter for CounterObject {
    fn add(&mut self, increment: u64) -> zplugin::Result<u64> {
        self.0 += increment;
        Ok(self.0)
    }
}

impl Info for InfoObject {
    fn label(&self, output: &mut [u8]) -> zplugin::Result<u32> {
        if output.len() < 4 {
            return Err(Error::BufferTooSmall);
        }
        output[..4].copy_from_slice(b"info");
        Ok(4)
    }
}

impl Drop for CounterObject {
    fn drop(&mut self) {
        DROPPED.fetch_or(1, Ordering::SeqCst);
    }
}

impl Drop for InfoObject {
    fn drop(&mut self) {
        DROPPED.fetch_or(2, Ordering::SeqCst);
    }
}

// SAFETY: neither object starts background activity or registers callbacks/TLS.
unsafe impl zplugin::export::UnloadSafe for CounterObject {}
unsafe impl zplugin::export::UnloadSafe for InfoObject {}

fn create_counter() -> zplugin::Result<CounterObject> {
    Ok(CounterObject(40))
}

fn create_info() -> zplugin::Result<InfoObject> {
    Ok(InfoObject)
}

zplugin::export_interfaces!(version = (9, 8, 7), unload = cooperative;
    CounterExport<CounterObject> => create_counter,
    InfoExport<InfoObject> => create_info,
);

unsafe extern "C" {
    fn zplugin_entry_v1() -> *const abi::Header;
}

#[test]
fn independent_interfaces_share_entry_and_quiescence() {
    // SAFETY: the symbol above exports immutable metadata backed by this test binary.
    let header = unsafe { &*zplugin_entry_v1() };
    let descriptor = unsafe { &*(header as *const abi::Header).cast::<abi::PluginDescriptor>() };
    assert_eq!(descriptor.interface_count, 2);
    assert_eq!(descriptor.flags, abi::COOPERATIVE);
    let quiesce = descriptor.quiesce.unwrap();
    assert_eq!(unsafe { quiesce() }, 0);

    let plugin = unsafe { Plugin::from_static(header) }.unwrap();
    assert_eq!(
        plugin.version(),
        abi::Version {
            major: 9,
            minor: 8,
            patch: 7
        }
    );
    let mut counter = CounterApi::create(&plugin).unwrap();
    let mut info = InfoApi::create(&plugin).unwrap();
    assert_eq!(counter.add(2), Ok(42));
    let mut bytes = [0_u8; 8];
    assert_eq!(info.label(&mut bytes), Ok(4));
    assert_eq!(&bytes[..4], b"info");
    assert!(matches!(
        plugin.bind(zplugin::InterfaceSpec {
            version: 1,
            ..InfoApi::SPEC
        }),
        Err(Error::InterfaceVersion {
            expected: 1,
            actual: 7
        })
    ));
    assert_eq!(unsafe { quiesce() }, Error::Busy.status());
    drop(counter);
    assert_eq!(DROPPED.load(Ordering::SeqCst), 1);
    assert_eq!(unsafe { quiesce() }, Error::Busy.status());
    drop(info);
    assert_eq!(DROPPED.load(Ordering::SeqCst), 3);
    assert_eq!(unsafe { quiesce() }, 0);
}
