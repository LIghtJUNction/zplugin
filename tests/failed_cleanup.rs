#![cfg(all(feature = "std", feature = "macros"))]
//! Separate test binary: the process-wide conservative latch intentionally never
//! resets, so it must not contaminate unrelated cooperative-unload checks.
use zplugin::{
    Error, OwnedTask, abi,
    export::{Factory, Registration, UnloadSafe},
};

#[zplugin::interface(id = "tests.cleanup", version = 1)]
trait Cleanup {
    fn value(&self) -> zplugin::Result<u32>;
}
struct Service;
impl Cleanup for Service {
    fn value(&self) -> zplugin::Result<u32> {
        Ok(1)
    }
}
// SAFETY: no external activity; this test explicitly checks a failed cleanup is
// retained even when the normal factory itself satisfies the contract.
unsafe impl UnloadSafe for Service {}
struct Create;
impl Factory for Create {
    type Object = Service;
    fn create() -> zplugin::Result<Service> {
        Ok(Service)
    }
}

#[test]
fn failed_future_destructor_permanently_prevents_cooperative_close() {
    let descriptor = Registration::<CleanupExport<Service>, Create>::cooperative(abi::Version {
        major: 1,
        minor: 0,
        patch: 0,
    });
    let quiesce = descriptor.quiesce.unwrap();
    assert_eq!(unsafe { quiesce() }, 0);
    struct Bad;
    impl Drop for Bad {
        fn drop(&mut self) {
            panic!("failed async resource cleanup");
        }
    }
    let bad = Bad;
    let mut task = OwnedTask::<u64>::new(async move {
        let _bad = bad;
        core::future::pending::<()>().await;
        Ok(0)
    });
    assert_eq!(unsafe { quiesce() }, Error::Busy.status());
    assert_eq!(task.cancel(), Err(Error::Panicked));
    drop(task);
    assert_eq!(unsafe { quiesce() }, Error::Busy.status());
}
