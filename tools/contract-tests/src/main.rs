//! Real dynamic-library lifecycle and negative protocol tests.

use demo_interface::{DemoApi, DemoProxy, SNAPSHOT_SCHEMA};
use std::{
    env,
    mem::size_of,
    path::{Path, PathBuf},
    rc::Rc,
};
use zplugin::{Error, Plugin, Result, UnloadPolicy, abi, hot::Generations};

fn path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}{}", env::consts::DLL_SUFFIX))
}

fn load(directory: &Path, name: &str, policy: UnloadPolicy) -> Plugin {
    // SAFETY: these are locally built reviewed test fixtures. Even the malformed
    // descriptors have valid readable prefixes and no hidden activity.
    unsafe { Plugin::load_path(path(directory, name), policy) }
        .unwrap_or_else(|error| panic!("loading {name}: {error}"))
}

fn expect_error<T>(result: Result<T>, expected: Error) {
    match result {
        Err(error) => assert_eq!(error, expected),
        Ok(_) => panic!("expected {expected}, operation succeeded"),
    }
}

fn instance(plugin: &Rc<Plugin>) -> Result<DemoProxy<'static>> {
    DemoApi::wrap(Plugin::create_owned(plugin, DemoApi::SPEC)?)
}

fn close(plugin: Plugin) {
    if let Err((error, _)) = plugin.try_close() {
        panic!("expected a quiescent library, got {error}");
    }
}

fn malformed_descriptors(directory: &Path) {
    for (name, error) in [
        ("missing", Error::MissingEntry),
        ("null", Error::InvalidArgument),
        ("abi", Error::AbiVersion { found: 99 }),
        ("magic", Error::BadMagic),
        (
            "short",
            Error::DescriptorTooShort {
                expected: size_of::<abi::PluginDescriptor>() as u32,
                actual: size_of::<abi::Header>() as u32,
            },
        ),
    ] {
        // SAFETY: fixtures provide readable prefixes and cooperative code.
        expect_error(
            unsafe { Plugin::load_path(path(directory, name), UnloadPolicy::Cooperative) },
            error,
        );
    }
    for (name, error) in [
        (
            "version",
            Error::InterfaceVersion {
                expected: 1,
                actual: 999,
            },
        ),
        (
            "short_interface",
            Error::DescriptorTooShort {
                expected: size_of::<abi::InterfaceDescriptor>() as u32,
                actual: 4,
            },
        ),
        ("signature", Error::SignatureMismatch { method: 0 }),
        ("missing_method", Error::MissingMethod { method: 0 }),
    ] {
        let plugin = load(directory, name, UnloadPolicy::Cooperative);
        expect_error(plugin.bind(DemoApi::SPEC), error);
        close(plugin);
    }
    println!(
        "PASS malformed descriptors: missing/null entry, ABI/magic/size, version, interface size, signature, missing method"
    );
}

fn normal_calls_and_errors(directory: &Path) {
    let plugin = load(directory, "v1", UnloadPolicy::Cooperative);
    let mut object = DemoApi::create(&plugin).unwrap();
    assert_eq!(object.add(u64::MAX, 1), Ok(0));
    assert_eq!(object.transform(&[], &mut []), Ok(0));
    assert_eq!(object.text("🙂é中"), Ok(3));
    let mut output = [0_u8; 3];
    assert_eq!(object.transform(b"AbC", &mut output), Ok(3));
    assert_eq!(&output, b"ABC");
    let mut short = [0xaa_u8; 2];
    assert_eq!(
        object.transform(b"long", &mut short),
        Err(Error::BufferTooSmall)
    );
    assert_eq!(short, [0xaa; 2]);
    assert_eq!(object.fail(1007), Err(Error::PluginStatus(1007)));
    assert_eq!(object.fail(0), Ok(()));
    assert_eq!(object.restore(999, &[0; 8]), Err(Error::InvalidArgument));
    assert_eq!(
        object.restore(SNAPSHOT_SCHEMA, &[]),
        Err(Error::InvalidArgument)
    );
    drop(object);
    close(plugin);
    let plugin = load(directory, "fail_init", UnloadPolicy::Cooperative);
    expect_error(DemoApi::create(&plugin), Error::PluginStatus(1001));
    close(plugin);
    println!(
        "PASS typed calls: scalars, empty input, UTF-8, short outputs, application statuses, constructor failure"
    );
}

fn migration_and_coexistence(directory: &Path) {
    let mut generations = Generations::new(load(directory, "v1", UnloadPolicy::Cooperative), 2);
    let lease = generations.current();
    let mut old = instance(&lease).unwrap();
    assert_eq!(old.add(1, 2), Ok(3));
    let mut snapshot = [0; 8];
    old.snapshot(&mut snapshot).unwrap();
    let count = Rc::strong_count(&lease);
    for i in 0..1000 {
        assert_eq!(old.add(i, 1), Ok(i + 1));
    }
    assert_eq!(
        Rc::strong_count(&lease),
        count,
        "ordinary calls must not retain extra generation references"
    );
    // SAFETY: both separately named images satisfy the same reviewed contract.
    let mut new = unsafe {
        generations.reload_path(path(directory, "v2"), UnloadPolicy::Cooperative, |plugin| {
            let mut value = instance(plugin)?;
            value.restore(SNAPSHOT_SCHEMA, &snapshot)?;
            Ok(value)
        })
    }
    .unwrap();
    let mut restored = [0; 8];
    new.snapshot(&mut restored).unwrap();
    assert_eq!(snapshot, restored);
    let mut upper = [0; 3];
    let mut lower = [0; 3];
    old.transform(b"AbC", &mut upper).unwrap();
    new.transform(b"AbC", &mut lower).unwrap();
    assert_eq!(&upper, b"ABC");
    assert_eq!(&lower, b"abc");
    assert_eq!(generations.retired_count(), 1);
    assert_eq!(
        generations.collect(),
        0,
        "old object and request lease keep its image alive"
    );
    expect_error(
        unsafe {
            generations.reload_path(path(directory, "v2"), UnloadPolicy::Cooperative, |_| Ok(()))
        },
        Error::SamePath,
    );
    assert_eq!(
        new.add(4, 5),
        Ok(9),
        "duplicate-handle rejection must preserve the current image"
    );
    drop(old);
    drop(lease);
    assert_eq!(generations.collect(), 1);
    assert_eq!(generations.retired_count(), 0);
    drop(new);
    println!(
        "PASS generations: explicit snapshot/restore, simultaneous old/new behavior, request reuse, same-image rejection, retirement"
    );
}

fn failed_replacement_preserves_current(directory: &Path) {
    let mut generations = Generations::new(load(directory, "v1", UnloadPolicy::Cooperative), 1);
    let original = generations.current();
    expect_error(
        unsafe {
            generations.reload_path(
                path(directory, "fail_init"),
                UnloadPolicy::Cooperative,
                instance,
            )
        },
        Error::PluginStatus(1001),
    );
    assert!(Rc::ptr_eq(&original, &generations.current()));
    assert_eq!(generations.resident_retired(), 1);
    // A failed preparation remains accounted for until collection. The next
    // reload must collect the quiescent candidate before checking this limit.
    expect_error(
        unsafe {
            generations.reload_path(path(directory, "v2"), UnloadPolicy::Cooperative, |plugin| {
                let mut value = instance(plugin)?;
                value.restore(999, &[0; 8])?;
                Ok(value)
            })
        },
        Error::InvalidArgument,
    );
    assert!(Rc::ptr_eq(&original, &generations.current()));
    assert_eq!(generations.resident_retired(), 1);
    assert_eq!(generations.collect(), 1);
    assert_eq!(generations.resident_retired(), 0);
    assert_eq!(instance(&original).unwrap().add(20, 22), Ok(42));
    println!("PASS rollback: failed constructor or restore leaves the current generation usable");
}

fn owned_buffer_retains_its_library(directory: &Path) {
    let mut generations = Generations::new(load(directory, "v1", UnloadPolicy::Cooperative), 1);
    let lease = generations.current();
    let mut object = instance(&lease).unwrap();
    let buffer = object.owned(b"resource survives the proxy").unwrap();
    drop(object);
    drop(lease);
    let mut new = unsafe {
        generations.reload_path(path(directory, "v2"), UnloadPolicy::Cooperative, instance)
    }
    .unwrap();
    assert_eq!(&*buffer, b"resource survives the proxy");
    assert_eq!(generations.collect(), 0);
    assert_eq!(generations.resident_retired(), 1);
    expect_error(
        unsafe {
            generations.reload_path(
                path(directory, "fail_init"),
                UnloadPolicy::Cooperative,
                instance,
            )
        },
        Error::ResidentLimit,
    );
    assert_eq!(new.add(10, 20), Ok(30));
    drop(buffer); // The originating plugin frees bytes before the last lease is released.
    assert_eq!(generations.collect(), 1);
    drop(new);
    println!(
        "PASS owned result: remains valid after object/owner drop and prevents old-generation reclamation until release"
    );
}

fn failed_prepare_leases_remain_bounded(directory: &Path) {
    let mut generations = Generations::new(load(directory, "v1", UnloadPolicy::Cooperative), 1);
    let original = generations.current();
    let mut escaped = None;
    expect_error(
        unsafe {
            generations.reload_path(path(directory, "v2"), UnloadPolicy::Cooperative, |plugin| {
                escaped = Some(Rc::clone(plugin));
                Err::<(), _>(Error::PluginStatus(2002))
            })
        },
        Error::PluginStatus(2002),
    );
    assert!(Rc::ptr_eq(&original, &generations.current()));
    assert_eq!(generations.resident_retired(), 1);
    expect_error(
        unsafe {
            generations.reload_path(
                path(directory, "fail_init"),
                UnloadPolicy::Cooperative,
                |_| Ok(()),
            )
        },
        Error::ResidentLimit,
    );
    drop(escaped);
    assert_eq!(generations.collect(), 1);
    expect_error(
        unsafe {
            generations.reload_path(
                path(directory, "v2"),
                UnloadPolicy::Cooperative,
                |_| -> Result<()> {
                    panic!("intentional failing preparation");
                },
            )
        },
        Error::Panicked,
    );
    assert!(Rc::ptr_eq(&original, &generations.current()));
    assert_eq!(generations.resident_retired(), 1);
    assert_eq!(generations.collect(), 1);
    assert_eq!(generations.resident_retired(), 0);
    println!(
        "PASS failed preparation: escaped candidate leases count toward the cap, panic rolls back"
    );
}

fn busy_quiescence_preserves_ownership(directory: &Path) {
    let plugin = load(directory, "busy", UnloadPolicy::Cooperative);
    let plugin = match plugin.try_close() {
        Err((Error::Busy, plugin)) => plugin,
        Err((error, _)) => panic!("expected Busy, got {error}"),
        Ok(()) => panic!("Busy plugin was closed"),
    };
    let mut generations = Generations::new(plugin, 1);
    unsafe {
        generations.reload_path(path(directory, "v1"), UnloadPolicy::Cooperative, |_| Ok(()))
    }
    .unwrap();
    assert_eq!(generations.collect(), 0);
    assert_eq!(generations.resident_retired(), 1);
    expect_error(
        unsafe {
            generations.reload_path(path(directory, "v2"), UnloadPolicy::Cooperative, |_| Ok(()))
        },
        Error::ResidentLimit,
    );
    println!(
        "PASS Busy quiescence: close returns the owner, retirement retains the library, limits remain enforced"
    );
}

fn pinned_generations_and_failures_are_bounded(directory: &Path) {
    let mut generations = Generations::new(load(directory, "v1", UnloadPolicy::Pinned), 1);
    unsafe { generations.reload_path(path(directory, "v2"), UnloadPolicy::Pinned, |_| Ok(())) }
        .unwrap();
    assert_eq!(generations.collect(), 0);
    assert_eq!(generations.resident_retired(), 1);
    expect_error(
        unsafe {
            generations.reload_path(path(directory, "fail_init"), UnloadPolicy::Pinned, |_| {
                Ok(())
            })
        },
        Error::ResidentLimit,
    );

    let mut attempts = Generations::new(load(directory, "v1", UnloadPolicy::Cooperative), 1);
    expect_error(
        unsafe { attempts.reload_path(path(directory, "abi"), UnloadPolicy::Pinned, |_| Ok(())) },
        Error::AbiVersion { found: 99 },
    );
    assert_eq!(attempts.retired_count(), 0);
    assert_eq!(attempts.resident_retired(), 1);
    expect_error(
        unsafe {
            attempts.reload_path(path(directory, "v2"), UnloadPolicy::Cooperative, |_| Ok(()))
        },
        Error::ResidentLimit,
    );
    println!(
        "PASS pinned residency: both successful old generations and failed pinned loads consume the configured budget"
    );
}

fn main() {
    let directory = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: zplugin-contract-tests <built-plugin-directory>");
    malformed_descriptors(&directory);
    normal_calls_and_errors(&directory);
    migration_and_coexistence(&directory);
    failed_replacement_preserves_current(&directory);
    owned_buffer_retains_its_library(&directory);
    failed_prepare_leases_remain_bounded(&directory);
    busy_quiescence_preserves_ownership(&directory);
    // Pinned images intentionally remain loaded, so run these after reclaim tests.
    pinned_generations_and_failures_are_bounded(&directory);
    println!("8 dynamic contract groups passed");
}
