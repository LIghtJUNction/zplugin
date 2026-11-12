#![cfg(all(feature = "macros", feature = "alloc"))]

use core::ffi::c_void;
use zplugin::{AbiValue, Error, abi, export::ExportInterface};

#[zplugin::abi_type(id = "tests.point", version = 1)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    x: i32,
    y: f64,
}

#[zplugin::abi_type(id = "tests.mode", version = 2)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Read = 3,
    Write = 7,
}

#[zplugin::abi_type(id = "tests.packet", version = 1)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Packet {
    position: Point,
    mode: Mode,
}

#[zplugin::abi_type(id = "tests.event", version = 1)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Event {
    Idle,
    Move { target: Point, mode: Mode },
    Pair(u64, i16),
    Empty {},
}

#[zplugin::interface(id = "tests.values", version = 1)]
trait Values {
    fn packet(&mut self, value: Packet) -> zplugin::Result<Packet>;
    fn event(&mut self, value: Event) -> zplugin::Result<Event>;
    fn mode(&mut self, value: Mode) -> zplugin::Result<u64>;
}

struct Identity;
impl Values for Identity {
    fn packet(&mut self, value: Packet) -> zplugin::Result<Packet> {
        Ok(value)
    }
    fn event(&mut self, value: Event) -> zplugin::Result<Event> {
        Ok(value)
    }
    fn mode(&mut self, value: Mode) -> zplugin::Result<u64> {
        Ok(value.into_wire().into())
    }
}

struct Factory;
impl zplugin::export::Factory for Factory {
    type Object = Identity;
    fn create() -> zplugin::Result<Identity> {
        Ok(Identity)
    }
}

static DESCRIPTOR: abi::PluginDescriptor =
    zplugin::export::Registration::<ValuesExport<Identity>, Factory>::pinned(abi::Version {
        major: 1,
        minor: 0,
        patch: 0,
    });

#[test]
fn structures_and_all_tagged_variants_round_trip() {
    // SAFETY: static metadata and implementations live for the whole executable.
    let plugin = unsafe { zplugin::Plugin::from_static(&DESCRIPTOR.header).unwrap() };
    let mut proxy = ValuesApi::create(&plugin).unwrap();
    let point = Point {
        x: i32::MIN,
        y: -0.0,
    };
    let packet = Packet {
        position: point,
        mode: Mode::Write,
    };
    assert_eq!(proxy.packet(packet), Ok(packet));
    for value in [
        Event::Idle,
        Event::Move {
            target: point,
            mode: Mode::Read,
        },
        Event::Pair(u64::MAX, i16::MIN),
        Event::Empty {},
    ] {
        assert_eq!(proxy.event(value), Ok(value));
    }
    assert_eq!(proxy.mode(Mode::Read), Ok(3));
}

#[test]
fn schemas_include_nested_fields_tags_and_versions() {
    assert_eq!(
        Mode::SCHEMA,
        "enum_u32_v1<tests.mode@2>{Read=3(),Write=7()}"
    );
    assert_eq!(Point::SCHEMA, "struct_v1<tests.point@1>{x:i32,y:f64}");
    assert_eq!(
        Packet::SCHEMA,
        "struct_v1<tests.packet@1>{position:struct_v1<tests.point@1>{x:i32,y:f64},mode:enum_u32_v1<tests.mode@2>{Read=3(),Write=7()}}"
    );
    assert!(ValuesApi::SPEC.signatures[0].contains(Packet::SCHEMA));
    assert!(ValuesApi::SPEC.signatures[1].contains(Event::SCHEMA));
    assert_eq!(
        ValuesApi::SPEC.signatures[2],
        "v1:mode:mut(enum_u32_v1<tests.mode@2>{Read=3(),Write=7()})->result<u64>"
    );
}

#[test]
fn invalid_enum_tags_are_errors_before_payload_interpretation() {
    assert_eq!(Mode::from_wire(0), Err(Error::InvalidArgument));
    let mut wire = Event::Pair(22, -8).into_wire();
    // SAFETY: generated tagged wire records begin with an initialized u32 tag.
    // An unknown tag is valid wire data, and conversion must reject it.
    unsafe {
        (&mut wire as *mut <Event as AbiValue>::Wire)
            .cast::<u32>()
            .write(999);
    }
    assert_eq!(Event::from_wire(wire), Err(Error::InvalidArgument));
}

#[test]
fn invalid_input_does_not_initialize_output_or_poison_object() {
    let mut object = zplugin::export::Object::new(Identity);
    let call = <ValuesExport<Identity> as ExportInterface>::METHODS[2]
        .call
        .unwrap();
    #[repr(C)]
    struct Args {
        value: u32,
    }
    let args = Args { value: 900 };
    let mut result = 0xface_beef_u64;
    // SAFETY: exactly the generated method's wire layout, valid object/output
    // pointers, and an invalid-but-initialized enum tag to exercise validation.
    let status = unsafe {
        call(
            (&mut object as *mut zplugin::export::Object<Identity>).cast::<c_void>(),
            (&args as *const Args).cast(),
            (&mut result as *mut u64).cast(),
        )
    };
    assert_eq!(status, Error::InvalidArgument.status());
    assert_eq!(result, 0xface_beef);
    let args = Args { value: 3 };
    let status = unsafe {
        call(
            (&mut object as *mut zplugin::export::Object<Identity>).cast::<c_void>(),
            (&args as *const Args).cast(),
            (&mut result as *mut u64).cast(),
        )
    };
    assert_eq!(status, 0);
    assert_eq!(result, 3);
}

#[test]
fn c_header_contains_real_shapes_and_exact_method_signatures() {
    let mut header = String::new();
    ValuesApi::write_c_header(&mut header).unwrap();
    assert!(header.contains("typedef struct zp_tests_2epoint_v1 {"));
    assert!(header.contains("int32_t x;"));
    assert!(header.contains("#define zp_tests_2emode_v2_Write UINT32_C(7)"));
    assert!(header.contains("typedef struct zp_tests_2eevent_v1 {"));
    assert!(header.contains("uint32_t tag;"));
    assert!(header.contains(ValuesApi::SPEC.signatures[0]));
    assert!(header.contains("Values_INTERFACE_INIT(methods, create, destroy)"));
}

mod outer {
    #[zplugin::abi_type(id = "tests.parent", version = 1)]
    #[derive(Clone, Copy)]
    pub struct Parent {
        pub value: u64,
    }
    pub mod inner {
        #[zplugin::abi_type(id = "tests.relative", version = 1)]
        #[derive(Clone, Copy)]
        pub struct Relative {
            pub parent: super::Parent,
        }
        #[zplugin::interface(id = "tests.relative-interface", version = 1)]
        #[allow(dead_code)]
        pub trait RelativeApiTest {
            fn value(&self, value: super::Parent) -> zplugin::Result<Relative>;
        }
    }
}

#[test]
fn relative_type_paths_resolve_at_the_declaration_site() {
    let value = outer::inner::Relative {
        parent: outer::Parent { value: 42 },
    };
    assert_eq!(
        outer::inner::Relative::from_wire(value.into_wire())
            .unwrap()
            .parent
            .value,
        42
    );
}

#[zplugin::abi_type(id = "tests.same-enum", version = 1)]
#[derive(Clone, Copy)]
enum Fieldless {
    Empty,
}

#[zplugin::abi_type(id = "tests.same-enum", version = 1)]
#[derive(Clone, Copy)]
enum EmptyPayload {
    Empty {},
}

#[test]
fn enum_wire_kind_is_part_of_the_compatibility_schema() {
    assert_ne!(Fieldless::SCHEMA, EmptyPayload::SCHEMA);
    assert_ne!(
        core::mem::size_of::<<Fieldless as AbiValue>::Wire>(),
        core::mem::size_of::<<EmptyPayload as AbiValue>::Wire>()
    );
}

#[zplugin::interface(id = "tests.unicode-c-names", version = 1)]
#[allow(dead_code)]
trait Café {
    fn r#type(&self) -> zplugin::Result<u64>;
    fn 读取(&self) -> zplugin::Result<u64>;
}

#[test]
fn generated_headers_escape_unicode_names_and_strip_rust_raw_prefixes() {
    let mut header = String::new();
    CaféApi::write_c_header(&mut header).unwrap();
    assert!(header.is_ascii());
    assert!(!header.contains("r#"));
    assert!(header.contains("Caf_ue9__type_args"));
    assert!(header.contains("Caf_ue9___u8bfb__u53d6__args"));
}
