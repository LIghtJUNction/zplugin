use demo_interface::{DemoApi, SNAPSHOT_SCHEMA};
use std::{env, path::PathBuf};
use zplugin::{Error, Plugin, UnloadPolicy};

fn self_test(plugin: &Plugin) -> zplugin::Result<()> {
    let mut processor = DemoApi::create(plugin)?;
    assert_eq!(processor.add(u64::MAX, 1)?, 0);
    assert_eq!(processor.transform(&[], &mut [])?, 0);
    assert_eq!(processor.text("")?, 0);
    assert_eq!(processor.text("🙂é中")?, 3);
    let mut short = [0xa5_u8; 2];
    assert_eq!(
        processor.transform(b"long", &mut short),
        Err(Error::BufferTooSmall)
    );
    assert_eq!(short, [0xa5; 2]);
    assert_eq!(processor.fail(1701), Err(Error::PluginStatus(1701)));
    assert_eq!(processor.fail(0), Ok(()));
    assert_eq!(processor.restore(99, &[0; 8]), Err(Error::InvalidArgument));
    assert_eq!(
        processor.restore(SNAPSHOT_SCHEMA, &[]),
        Err(Error::InvalidArgument)
    );
    assert!(processor.owned(&[])?.is_empty());
    let owned = processor.owned(b"lifetime")?;
    drop(processor);
    assert_eq!(&*owned, b"lifetime");
    drop(owned);
    let mut panicking = DemoApi::create(plugin)?;
    assert_eq!(panicking.panic_now(), Err(Error::Panicked));
    assert_eq!(panicking.add(1, 2), Err(Error::Poisoned));
    assert_eq!(DemoApi::create(plugin)?.add(1, 2)?, 3);
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        let filename = format!(
            "{}demo_plugin{}",
            env::consts::DLL_PREFIX,
            env::consts::DLL_SUFFIX
        );
        PathBuf::from("target/debug").join(filename)
    });
    // SAFETY: This example loads our reviewed plugin. It implements the v1 ABI
    // and the cooperative unload contract, including all resource destructors.
    let plugin = unsafe { Plugin::load_path(&path, UnloadPolicy::Cooperative)? };
    if env::args().any(|argument| argument == "--self-test") {
        self_test(&plugin)?;
        println!("edge cases, panic containment, and per-instance poisoning passed");
    }
    let mut processor = DemoApi::create(&plugin)?;
    assert_eq!(processor.add(20, 22)?, 42);
    let mut output = [0_u8; 64];
    let used = processor.transform(b"Hello, Project Z!", &mut output)? as usize;
    println!("{}", std::str::from_utf8(&output[..used])?);
    println!("Unicode scalar values: {}", processor.text("Rust 插件")?);

    let mut snapshot = [0_u8; 8];
    processor.snapshot(&mut snapshot)?;
    let mut restored = DemoApi::create(&plugin)?;
    restored.restore(SNAPSHOT_SCHEMA, &snapshot)?;
    let mut roundtrip = [0_u8; 8];
    restored.snapshot(&mut roundtrip)?;
    assert_eq!(snapshot, roundtrip);

    let owned = processor.owned(b"plugin-owned bytes")?;
    drop(processor);
    assert_eq!(&*owned, b"plugin-owned bytes");
    drop(owned);
    drop(restored);
    println!("typed calls, explicit state migration, and owned release passed");
    Ok(())
}
