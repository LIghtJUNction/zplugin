//! Load a binary Wasm module without granting filesystem or network imports.
use std::{env, error::Error as StdError, fs::File, io::Read};
use zplugin::wasm::{Error, Interface, Limits, Plugin};

const INTERFACE: Interface = Interface {
    id: 0x7a70_6c75_6769_6e01,
    version: 1,
};

fn self_test(bytes: &[u8]) -> Result<(), Box<dyn StdError>> {
    let mut plugin = Plugin::new(bytes, INTERFACE, Limits::default())?;
    let mut checks = 0;
    for size in [0, 64, 4096, 1_048_576] {
        let input: Vec<_> = (0..size).map(|i| (i % 251) as u8).collect();
        let mut output = vec![0xa5; size];
        assert_eq!(plugin.call(0, &input, &mut output)?, size);
        assert_eq!(output, input);
    }
    checks += 1;
    println!("PASS empty, 64 B, 4 KiB, and 1 MiB guest round trips");

    let mut output = [0xa5; 32];
    let count = plugin.call(1, b"Hello, wasm!", &mut output)?;
    assert_eq!(&output[..count], b"HELLO, WASM!");
    checks += 1;
    println!("PASS typed Rust guest implementation and generated exports");

    for expected in [1_u64, 2] {
        assert_eq!(plugin.call(2, &[], &mut output)?, 8);
        assert_eq!(u64::from_le_bytes(output[..8].try_into()?), expected);
    }
    let mut other = Plugin::new(bytes, INTERFACE, Limits::default())?;
    other.call(2, &[], &mut output)?;
    assert_eq!(u64::from_le_bytes(output[..8].try_into()?), 1);
    checks += 1;
    println!("PASS persistent state and isolation between two instances");

    output.fill(0xa5);
    assert!(matches!(
        plugin.call(3, b"not published on error", &mut output),
        Err(Error::GuestStatus(77))
    ));
    assert_eq!(output, [0xa5; 32]);
    assert!(matches!(
        plugin.call(0, b"short output", &mut output[..2]),
        Err(Error::GuestStatus(-3))
    ));
    assert!(matches!(
        plugin.call(u32::MAX, &[], &mut output),
        Err(Error::GuestStatus(-7))
    ));
    assert!(!plugin.is_poisoned());
    checks += 1;
    println!("PASS guest statuses, short output, and transactional host output");

    let oversized = vec![0; 1_048_577];
    assert!(matches!(
        plugin.call(0, &oversized, &mut output),
        Err(Error::InputTooLarge { .. })
    ));
    assert!(!plugin.is_poisoned());
    checks += 1;
    println!("PASS input capacity rejected before invocation");

    let memory_before = plugin.memory_size();
    plugin.call(6, &[], &mut output)?;
    assert_eq!(u32::from_le_bytes(output[..4].try_into()?), u32::MAX);
    assert_eq!(plugin.memory_size(), memory_before);
    checks += 1;
    println!("PASS memory.grow denial and unchanged guest memory size");

    output.fill(0xa5);
    assert!(matches!(
        plugin.call(4, &[], &mut output),
        Err(Error::FuelExhausted)
    ));
    assert!(plugin.is_poisoned());
    assert!(matches!(
        plugin.call(0, &[], &mut output),
        Err(Error::Poisoned)
    ));
    assert_eq!(output, [0xa5; 32]);
    checks += 1;
    println!("PASS infinite loop bounded by fuel and trapped instance poisoned");

    assert!(other.call(5, &[], &mut output).is_err());
    assert!(other.is_poisoned());
    let mut fresh = Plugin::new(bytes, INTERFACE, Limits::default())?;
    let count = fresh.call(0, b"still alive", &mut output)?;
    assert_eq!(&output[..count], b"still alive");
    checks += 1;
    println!("PASS native host survives guest traps and can create a fresh instance");
    println!(
        "{{\"suite\":\"wasm-rust-guest\",\"checks\":{checks},\"status\":\"passed\",\"guest_memory_bytes\":{memory_before}}}"
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn StdError>> {
    let mut arguments = env::args_os().skip(1);
    let path = arguments
        .next()
        .ok_or("usage: wasm_host PATH.wasm [--self-test]")?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(Limits::default().max_module_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if arguments.next().is_some_and(|arg| arg == "--self-test") {
        return self_test(&bytes);
    }
    let mut plugin = Plugin::new(&bytes, INTERFACE, Limits::default())?;
    let mut output = [0; 64];
    let count = plugin.call(1, b"Hello from an untrusted Wasm plugin", &mut output)?;
    println!("{}", std::str::from_utf8(&output[..count])?);
    Ok(())
}
