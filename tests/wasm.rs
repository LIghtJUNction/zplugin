#![cfg(feature = "wasm")]

mod wasm_support;

use wasm_support::{
    CAPACITY, Fixture, Function, ID, Import, OUTPUT, Table, constant, i32_const, i64_const,
    infinite_loop, return_i32,
};
use zplugin::wasm::{Error, Interface, Limits, Plugin};

fn interface() -> Interface {
    Interface { id: ID, version: 1 }
}

fn load(fixture: &Fixture) -> Plugin {
    Plugin::new(&fixture.bytes(), interface(), Limits::default()).unwrap()
}

fn rejects(fixture: &Fixture) {
    assert!(Plugin::new(&fixture.bytes(), interface(), Limits::default()).is_err());
}

#[test]
fn plugin_is_neither_send_nor_sync() {
    // This expression has one applicable implementation only while both traits
    // are absent. Adding either auto trait makes the marker type ambiguous and
    // the test crate fails to compile, without an external assertion dependency.
    trait LocalOnly<Marker> {
        fn check() {}
    }
    impl<T: ?Sized> LocalOnly<()> for T {}
    struct IsSend;
    impl<T: ?Sized + Send> LocalOnly<IsSend> for T {}
    struct IsSync;
    impl<T: ?Sized + Sync> LocalOnly<IsSync> for T {}
    let _ = <Plugin as LocalOnly<_>>::check;
}

#[test]
fn empty_exact_and_short_output_round_trips() {
    let mut plugin = load(&Fixture::default());
    assert_eq!(plugin.call(0, &[], &mut []).unwrap(), 0);
    let mut output = [0xa5; CAPACITY as usize];
    let input = [0x5a; CAPACITY as usize];
    assert_eq!(plugin.call(0, &input, &mut output).unwrap(), input.len());
    assert_eq!(output, input);
    let mut short = [0xa5; 4];
    assert_eq!(
        plugin.call(0, b"too long", &mut short),
        Err(Error::GuestStatus(-1))
    );
    assert_eq!(short, [0xa5; 4]);
    assert!(!plugin.is_poisoned());
    assert_eq!(plugin.call(0, b"ok", &mut output).unwrap(), 2);
    assert_eq!(&output[..2], b"ok");
}

#[test]
fn host_input_limit_is_non_destructive() {
    let mut plugin = load(&Fixture::default());
    let mut output = [0xa5; 8];
    assert_eq!(
        plugin.call(0, &[0; CAPACITY as usize + 1], &mut output),
        Err(Error::InputTooLarge {
            size: CAPACITY as usize + 1,
            limit: CAPACITY as usize
        }),
    );
    assert_eq!(output, [0xa5; 8]);
    assert!(!plugin.is_poisoned());
    assert_eq!(plugin.call(0, b"after", &mut output).unwrap(), 5);
}

#[test]
fn application_status_does_not_poison_or_copy_output() {
    let mut code = Vec::new();
    i32_const(OUTPUT as i32, &mut code);
    i32_const(0xff, &mut code);
    code.extend_from_slice(&[0x3a, 0, 0]);
    // Even an enormous upper-half length must not be read on a failed status.
    i64_const((u32::MAX as u64) << 32 | 4097, &mut code);
    let mut plugin = load(&Fixture::call(code));
    let mut output = [0xa5; 4];
    for _ in 0..3 {
        assert_eq!(
            plugin.call(0, b"", &mut output),
            Err(Error::GuestStatus(4097))
        );
        assert_eq!(output, [0xa5; 4]);
        assert!(!plugin.is_poisoned());
    }
}

#[test]
fn instance_state_is_persistent_but_separate() {
    let mut code = vec![0x23, 0]; // global.get counter
    i32_const(1, &mut code);
    code.extend_from_slice(&[0x6a, 0x24, 0]); // increment and global.set
    i32_const(OUTPUT as i32, &mut code);
    code.extend_from_slice(&[0x23, 0]);
    return_i32(&mut code);
    let fixture = Fixture::call(code);
    let mut first = load(&fixture);
    let mut second = load(&fixture);
    let mut output = [0; 4];
    for expected in [1_u32, 2, 3] {
        assert_eq!(first.call(0, b"", &mut output).unwrap(), 4);
        assert_eq!(u32::from_le_bytes(output), expected);
    }
    second.call(0, b"", &mut output).unwrap();
    assert_eq!(u32::from_le_bytes(output), 1);
}

#[test]
fn truncated_and_invalid_binary_modules_are_rejected() {
    let bytes = Fixture::default().bytes();
    for length in 0..bytes.len() {
        assert!(
            Plugin::new(&bytes[..length], interface(), Limits::default()).is_err(),
            "accepted truncated length {length}"
        );
    }
    for corrupt in [
        b"not wasm".as_slice(),
        b"\0asm\x02\0\0\0",
        b"\0asm\x01\0\0\0\x01\xff\xff\xff\xff\xff\x01",
    ] {
        assert!(Plugin::new(corrupt, interface(), Limits::default()).is_err());
    }
    rejects(&Fixture::call(vec![0xff])); // Invalid instruction.
    rejects(&Fixture::call(vec![0x41, 0])); // i32 where i64 is required.
}

#[test]
fn oversized_module_is_rejected_before_validation() {
    let bytes = Fixture::default().bytes();
    let limits = Limits {
        max_module_bytes: bytes.len() - 1,
        ..Limits::default()
    };
    assert!(matches!(
        Plugin::new(&bytes, interface(), limits),
        Err(Error::ModuleTooLarge { .. })
    ));
}

#[test]
fn missing_and_wrongly_typed_exports_are_rejected() {
    for name in ["memory", "zplugin_wasm_info_v1", "zplugin_wasm_call_v1"] {
        let mut fixture = Fixture::default();
        fixture.exports.retain(|export| export.name != name);
        assert!(
            matches!(Plugin::new(&fixture.bytes(), interface(), Limits::default()), Err(Error::InvalidExport(found)) if found == name)
        );
    }
    let mut fixture = Fixture::default();
    fixture.exports[1].kind = 3; // A global exported under the function's name.
    rejects(&fixture);
    let mut fixture = Fixture::default();
    fixture.functions[0].kind = 3;
    fixture.info_code = Some(vec![0x41, 0]);
    rejects(&fixture);
    let mut fixture = Fixture::default();
    fixture.functions[1] = Function::new(0, constant(0));
    rejects(&fixture);
}

#[test]
fn abi_interface_and_application_version_are_checked() {
    for field in [0, 1, 2] {
        let mut fixture = Fixture::default();
        fixture.info[field] = fixture.info[field].wrapping_add(1);
        let error = Plugin::new(&fixture.bytes(), interface(), Limits::default()).unwrap_err();
        match field {
            0 => assert_eq!(error, Error::AbiVersion { found: 2 }),
            1 => assert_eq!(
                error,
                Error::InterfaceId {
                    expected: ID,
                    found: ID + 1
                }
            ),
            2 => assert_eq!(
                error,
                Error::InterfaceVersion {
                    expected: 1,
                    found: 2
                }
            ),
            _ => unreachable!(),
        }
    }
    let mut fixture = Fixture::default();
    fixture.info[2] = 1_u64 << 32 | 1; // Must not silently truncate.
    rejects(&fixture);
}

#[test]
fn every_import_kind_is_denied_including_wasi() {
    for import in [
        Import::Function("wasi_snapshot_preview1", "proc_exit"),
        Import::Function("env", "arbitrary_host_call"),
        Import::Memory,
        Import::Table,
        Import::Global,
    ] {
        let mut fixture = Fixture::default();
        if matches!(import, Import::Memory) {
            fixture.memory = None;
        }
        fixture.imports.push(import);
        assert!(matches!(
            Plugin::new(&fixture.bytes(), interface(), Limits::default()),
            Err(Error::ImportsDenied)
        ));
    }
}

#[test]
fn start_functions_are_denied_without_execution() {
    for code in [vec![], vec![0x03, 0x40, 0x0c, 0, 0x0b]] {
        let mut fixture = Fixture::default();
        fixture.functions.push(Function::new(2, code));
        fixture.start = Some(2);
        rejects(&fixture);
    }
}

#[test]
fn negotiation_executes_under_a_finite_budget() {
    let fixture = Fixture {
        info_code: Some(infinite_loop()),
        ..Fixture::default()
    };
    let limits = Limits {
        fuel_per_call: 1000,
        ..Limits::default()
    };
    assert!(matches!(
        Plugin::new(&fixture.bytes(), interface(), limits),
        Err(Error::FuelExhausted)
    ));
}

#[test]
fn metadata_queries_share_one_budget() {
    let mut fixture = Fixture::default();
    let mut code = Vec::new();
    for _ in 0..100 {
        // Observable state updates cannot be optimized into a constant response.
        code.extend_from_slice(&[0x23, 0, 0x41, 1, 0x6a, 0x24, 0]);
    }
    code.extend_from_slice(&fixture.info_body());
    fixture.info_code = Some(code);
    let limits = Limits {
        fuel_per_call: 1000,
        ..Limits::default()
    };
    assert!(matches!(
        Plugin::new(&fixture.bytes(), interface(), limits),
        Err(Error::FuelExhausted)
    ));
    assert!(
        Plugin::new(
            &fixture.bytes(),
            interface(),
            Limits {
                fuel_per_call: 10_000,
                ..limits
            }
        )
        .is_ok()
    );
}

#[test]
fn out_of_bounds_overlapping_and_wide_scratch_addresses_are_rejected() {
    for (field, value) in [
        (3, u32::MAX as u64),
        (4, 65537),
        (5, 65536),
        (6, 65537),
        (3, 1_u64 << 32),
        (4, 1_u64 << 32),
        (5, 1_u64 << 32),
        (6, 1_u64 << 32),
        (5, 128),
    ] {
        let mut fixture = Fixture::default();
        fixture.info[field] = value;
        rejects(&fixture);
    }
    let mut fixture = Fixture::default();
    fixture.info[3] = u32::MAX as u64 - 128;
    fixture.info[4] = 256;
    rejects(&fixture);
}

#[test]
fn advertised_buffer_capacity_must_obey_host_limits() {
    for limits in [
        Limits {
            max_input_bytes: 32,
            ..Limits::default()
        },
        Limits {
            max_output_bytes: 32,
            ..Limits::default()
        },
    ] {
        assert!(Plugin::new(&Fixture::default().bytes(), interface(), limits).is_err());
    }
}

#[test]
fn output_length_is_checked_before_copying_and_poisons_invalid_instance() {
    for claimed in [5_u32, CAPACITY + 1, u32::MAX] {
        let mut plugin = load(&Fixture::call(constant(u64::from(claimed) << 32)));
        let mut output = [0xa5; 4];
        assert_eq!(
            plugin.call(0, &[], &mut output),
            Err(Error::InvalidOutput {
                size: claimed as usize,
                limit: 4
            })
        );
        assert_eq!(output, [0xa5; 4]);
        assert!(plugin.is_poisoned());
        assert_eq!(plugin.call(0, &[], &mut output), Err(Error::Poisoned));
    }
}

#[test]
fn a_trap_does_not_expose_partial_output_or_poison_another_instance() {
    let mut code = Vec::new();
    i32_const(OUTPUT as i32, &mut code);
    i32_const(0xff, &mut code);
    code.extend_from_slice(&[0x3a, 0, 0, 0]); // store8 followed by unreachable.
    let mut malicious = load(&Fixture::call(code));
    let mut healthy = load(&Fixture::default());
    let mut output = [0xa5; 4];
    assert!(malicious.call(0, &[], &mut output).is_err());
    assert_eq!(output, [0xa5; 4]);
    assert!(malicious.is_poisoned());
    assert_eq!(healthy.call(0, b"safe", &mut output).unwrap(), 4);
    assert_eq!(&output, b"safe");
}

#[test]
fn infinite_execution_exhausts_fuel_and_cannot_be_resumed() {
    let limits = Limits {
        fuel_per_call: 1000,
        ..Limits::default()
    };
    let mut plugin =
        Plugin::new(&Fixture::call(infinite_loop()).bytes(), interface(), limits).unwrap();
    let mut output = [0xa5; 4];
    assert_eq!(plugin.call(0, &[], &mut output), Err(Error::FuelExhausted));
    assert!(plugin.is_poisoned());
    assert_eq!(output, [0xa5; 4]);
    assert_eq!(plugin.call(0, &[], &mut output), Err(Error::Poisoned));
}

#[test]
fn successful_calls_receive_fresh_fuel_without_accumulating_it() {
    let limits = Limits {
        fuel_per_call: 1000,
        ..Limits::default()
    };
    let mut plugin = Plugin::new(&Fixture::default().bytes(), interface(), limits).unwrap();
    let mut output = [0; 8];
    for _ in 0..2000 {
        assert_eq!(plugin.call(0, b"metered", &mut output).unwrap(), 7);
        assert!(plugin.fuel_remaining() < 1000);
    }
}

#[test]
fn recursion_is_bounded_without_exhausting_the_host_stack() {
    let code = vec![0x20, 0, 0x20, 1, 0x20, 2, 0x10, 1];
    let limits = Limits {
        max_call_depth: 8,
        ..Limits::default()
    };
    let mut plugin = Plugin::new(&Fixture::call(code).bytes(), interface(), limits).unwrap();
    assert_eq!(plugin.call(0, &[], &mut []), Err(Error::StackExhausted));
    assert!(plugin.is_poisoned());
}

#[test]
fn out_of_bounds_memory_read_and_write_trap() {
    for write in [false, true] {
        let mut code = Vec::new();
        i32_const(65536, &mut code);
        if write {
            i32_const(1, &mut code);
            code.extend_from_slice(&[0x3a, 0, 0]);
        } else {
            code.extend_from_slice(&[0x2d, 0, 0, 0x1a]);
        }
        i64_const(0, &mut code);
        let mut plugin = load(&Fixture::call(code));
        assert!(plugin.call(0, &[], &mut []).is_err());
        assert!(plugin.is_poisoned());
        assert_eq!(plugin.memory_size(), 65536);
    }
}

#[test]
fn memory_initialization_and_growth_respect_the_limit() {
    let limits = Limits {
        max_memory_bytes: 65536,
        max_input_bytes: CAPACITY as usize,
        max_output_bytes: CAPACITY as usize,
        ..Limits::default()
    };
    let fixture = Fixture {
        memory: Some((2, None)),
        ..Fixture::default()
    };
    assert!(Plugin::new(&fixture.bytes(), interface(), limits).is_err());
    let mut code = Vec::new();
    i32_const(OUTPUT as i32, &mut code);
    i32_const(1, &mut code);
    code.extend_from_slice(&[0x40, 0]);
    return_i32(&mut code);
    let mut plugin = Plugin::new(&Fixture::call(code).bytes(), interface(), limits).unwrap();
    let mut output = [0; 4];
    assert_eq!(plugin.call(0, &[], &mut output).unwrap(), 4);
    assert_eq!(i32::from_le_bytes(output), -1);
    assert_eq!(plugin.memory_size(), 65536);
    assert!(!plugin.is_poisoned());
}

#[test]
fn out_of_bounds_active_data_segment_is_rejected() {
    rejects(&Fixture {
        data_offset: Some(65536),
        ..Fixture::default()
    });
}

#[test]
fn table_initialization_and_growth_respect_the_limit() {
    let limits = Limits {
        max_table_elements: 2,
        ..Limits::default()
    };
    let fixture = Fixture {
        table: Some(Table {
            minimum: 3,
            maximum: None,
            elements: vec![],
        }),
        ..Fixture::default()
    };
    assert!(Plugin::new(&fixture.bytes(), interface(), limits).is_err());
    let mut code = Vec::new();
    i32_const(OUTPUT as i32, &mut code);
    code.extend_from_slice(&[0xd0, 0x70]); // ref.null funcref
    i32_const(2, &mut code);
    code.extend_from_slice(&[0xfc, 15, 0]); // table.grow
    return_i32(&mut code);
    let mut fixture = Fixture::call(code);
    fixture.table = Some(Table {
        minimum: 1,
        maximum: None,
        elements: vec![],
    });
    let mut plugin = Plugin::new(&fixture.bytes(), interface(), limits).unwrap();
    let mut output = [0; 4];
    assert_eq!(plugin.call(0, &[], &mut output).unwrap(), 4);
    assert_eq!(i32::from_le_bytes(output), -1);
    assert!(!plugin.is_poisoned());
}

#[test]
fn indirect_calls_cannot_escape_the_table_or_ignore_types() {
    for index in [0, 1] {
        let mut code = Vec::new();
        i32_const(index, &mut code);
        code.extend_from_slice(&[0x11, 2, 0]); // Expect ()->(), actual info(i32)->i64.
        i64_const(0, &mut code);
        let mut fixture = Fixture::call(code);
        fixture.table = Some(Table {
            minimum: 1,
            maximum: None,
            elements: vec![0],
        });
        let mut plugin = load(&fixture);
        assert!(plugin.call(0, &[], &mut []).is_err());
        assert!(plugin.is_poisoned());
    }
}

#[test]
fn an_unused_invalid_function_is_rejected_eagerly() {
    let mut fixture = Fixture::default();
    fixture.functions.push(Function::new(2, vec![0xff]));
    rejects(&fixture);
}

#[test]
fn guest_output_is_zeroed_and_host_suffix_is_preserved() {
    let mut code = vec![0x20, 0, 0x04, 0x40]; // if operation != 0, store a secret byte.
    i32_const(OUTPUT as i32, &mut code);
    i32_const(0xff, &mut code);
    code.extend_from_slice(&[0x3a, 0, 0, 0x0b]);
    i64_const(4_u64 << 32, &mut code);
    let mut plugin = load(&Fixture::call(code));
    let mut output = [0xa5; 8];
    assert_eq!(plugin.call(1, &[], &mut output).unwrap(), 4);
    assert_eq!(output, [0xff, 0, 0, 0, 0xa5, 0xa5, 0xa5, 0xa5]);
    assert_eq!(plugin.call(0, &[], &mut output).unwrap(), 4);
    assert_eq!(output, [0, 0, 0, 0, 0xa5, 0xa5, 0xa5, 0xa5]);
}

#[test]
fn additional_memories_tables_and_shared_or_64_bit_memory_are_rejected() {
    rejects(&Fixture {
        extra_memory: true,
        ..Fixture::default()
    });
    rejects(&Fixture {
        table: Some(Table {
            minimum: 1,
            maximum: None,
            elements: vec![],
        }),
        extra_table: true,
        ..Fixture::default()
    });
    rejects(&Fixture {
        memory: Some((1, Some(1))),
        memory_flags: Some(3),
        ..Fixture::default()
    });
    rejects(&Fixture {
        memory_flags: Some(4),
        ..Fixture::default()
    });
}

#[test]
fn invalid_host_policies_return_errors_without_panics() {
    let bytes = Fixture::default().bytes();
    for limits in [
        Limits {
            max_module_bytes: 7,
            ..Limits::default()
        },
        Limits {
            max_memory_bytes: 0,
            ..Limits::default()
        },
        Limits {
            max_memory_bytes: usize::MAX,
            ..Limits::default()
        },
        Limits {
            max_input_bytes: usize::MAX,
            ..Limits::default()
        },
        Limits {
            max_output_bytes: usize::MAX,
            ..Limits::default()
        },
        Limits {
            fuel_per_call: 0,
            ..Limits::default()
        },
        Limits {
            max_stack_bytes: 1023,
            ..Limits::default()
        },
        Limits {
            max_call_depth: 0,
            ..Limits::default()
        },
    ] {
        assert!(matches!(
            Plugin::new(&bytes, interface(), limits),
            Err(Error::InvalidLimits(_))
        ));
    }
}

#[test]
fn bounded_binary_mutations_never_panic_or_escape_call_limits() {
    // Deterministic smoke corpus, not a substitute for engine fuzzing or an audit.
    let original = Fixture::default().bytes();
    let limits = Limits {
        max_module_bytes: 4096,
        max_memory_bytes: 65536,
        max_input_bytes: 256,
        max_output_bytes: 256,
        max_table_elements: 16,
        max_call_depth: 16,
        max_stack_bytes: 4096,
        fuel_per_call: 1000,
    };
    for index in 0..original.len() {
        for bit in [0, 7] {
            let mut bytes = original.clone();
            bytes[index] ^= 1 << bit;
            if let Ok(mut plugin) = Plugin::new(&bytes, interface(), limits) {
                let mut output = [0xa5; 16];
                let _ = plugin.call(0, b"mutation", &mut output);
                assert!(plugin.memory_size() <= limits.max_memory_bytes);
                assert!(plugin.fuel_remaining() <= limits.fuel_per_call);
            }
        }
    }
}
