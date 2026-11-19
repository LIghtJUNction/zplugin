//! Small binary fixtures keep the adversarial tests independent of a WAT compiler.
//! This is a fixture encoder, not a validator or execution engine.

pub const ID: u64 = 0x7a70_6c75_6769_6e01;
pub const INPUT: u32 = 0;
pub const OUTPUT: u32 = 1024;
pub const CAPACITY: u32 = 256;

pub fn unsigned(mut value: u32, bytes: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        bytes.push(byte | if value == 0 { 0 } else { 0x80 });
        if value == 0 {
            break;
        }
    }
}

fn signed(mut value: i64, bytes: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let last = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        bytes.push(byte | if last { 0 } else { 0x80 });
        if last {
            break;
        }
    }
}

pub fn i32_const(value: i32, bytes: &mut Vec<u8>) {
    bytes.push(0x41);
    signed(i64::from(value), bytes);
}

pub fn i64_const(value: u64, bytes: &mut Vec<u8>) {
    bytes.push(0x42);
    signed(value as i64, bytes);
}

pub fn constant(value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    i64_const(value, &mut bytes);
    bytes
}

pub fn infinite_loop() -> Vec<u8> {
    vec![0x03, 0x40, 0x0c, 0x00, 0x0b, 0x42, 0x00]
}

fn text(value: &str, bytes: &mut Vec<u8>) {
    unsigned(value.len() as u32, bytes);
    bytes.extend_from_slice(value.as_bytes());
}

fn section(id: u8, content: Vec<u8>, module: &mut Vec<u8>) {
    module.push(id);
    unsigned(content.len() as u32, module);
    module.extend_from_slice(&content);
}

fn limits(min: u32, max: Option<u32>, bytes: &mut Vec<u8>) {
    bytes.push(u8::from(max.is_some()));
    unsigned(min, bytes);
    if let Some(max) = max {
        unsigned(max, bytes);
    }
}

#[derive(Clone)]
pub struct Function {
    pub kind: u32,
    pub code: Vec<u8>,
}

impl Function {
    pub fn new(kind: u32, code: Vec<u8>) -> Self {
        Self { kind, code }
    }
}

pub enum Import {
    Function(&'static str, &'static str),
    Memory,
    Table,
    Global,
}

pub struct Table {
    pub minimum: u32,
    pub maximum: Option<u32>,
    pub elements: Vec<u32>,
}

pub struct Export {
    pub name: &'static str,
    pub kind: u8,
    pub index: u32,
}

pub struct Fixture {
    pub info: [u64; 7],
    pub info_code: Option<Vec<u8>>,
    pub functions: Vec<Function>,
    pub memory: Option<(u32, Option<u32>)>,
    pub memory_flags: Option<u8>,
    pub extra_memory: bool,
    pub table: Option<Table>,
    pub extra_table: bool,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
    pub start: Option<u32>,
    pub data_offset: Option<u32>,
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            info: [
                1,
                ID,
                1,
                INPUT.into(),
                CAPACITY.into(),
                OUTPUT.into(),
                CAPACITY.into(),
            ],
            info_code: None,
            functions: vec![Function::new(0, Vec::new()), Function::new(1, echo())],
            memory: Some((1, None)),
            memory_flags: None,
            extra_memory: false,
            table: None,
            extra_table: false,
            imports: Vec::new(),
            exports: vec![
                Export {
                    name: "memory",
                    kind: 2,
                    index: 0,
                },
                Export {
                    name: "zplugin_wasm_info_v1",
                    kind: 0,
                    index: 0,
                },
                Export {
                    name: "zplugin_wasm_call_v1",
                    kind: 0,
                    index: 1,
                },
            ],
            start: None,
            data_offset: None,
        }
    }
}

impl Fixture {
    pub fn call(code: Vec<u8>) -> Self {
        let mut fixture = Self::default();
        fixture.functions[1].code = code;
        fixture
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut module = b"\0asm\x01\0\0\0".to_vec();
        // 0: info(i32)->i64; 1: call(i32,i32,i32)->i64;
        // 2: start()->(); 3: deliberately incompatible ()->i32.
        section(
            1,
            vec![
                4, 0x60, 1, 0x7f, 1, 0x7e, 0x60, 3, 0x7f, 0x7f, 0x7f, 1, 0x7e, 0x60, 0, 0, 0x60, 0,
                1, 0x7f,
            ],
            &mut module,
        );
        let imported_functions = self
            .imports
            .iter()
            .filter(|i| matches!(i, Import::Function(..)))
            .count() as u32;
        if !self.imports.is_empty() {
            let mut bytes = Vec::new();
            unsigned(self.imports.len() as u32, &mut bytes);
            for import in &self.imports {
                let (namespace, name) = match import {
                    Import::Function(namespace, name) => (*namespace, *name),
                    Import::Memory => ("env", "memory"),
                    Import::Table => ("env", "table"),
                    Import::Global => ("env", "global"),
                };
                text(namespace, &mut bytes);
                text(name, &mut bytes);
                match import {
                    Import::Function(..) => bytes.extend_from_slice(&[0, 2]),
                    Import::Table => bytes.extend_from_slice(&[1, 0x70, 0, 1]),
                    Import::Memory => bytes.extend_from_slice(&[2, 0, 1]),
                    Import::Global => bytes.extend_from_slice(&[3, 0x7f, 0]),
                }
            }
            section(2, bytes, &mut module);
        }
        let mut functions = Vec::new();
        unsigned(self.functions.len() as u32, &mut functions);
        for function in &self.functions {
            unsigned(function.kind, &mut functions);
        }
        section(3, functions, &mut module);
        if let Some(table) = &self.table {
            let mut bytes = vec![1 + u8::from(self.extra_table), 0x70];
            limits(table.minimum, table.maximum, &mut bytes);
            if self.extra_table {
                bytes.extend_from_slice(&[0x70, 0, 1]);
            }
            section(4, bytes, &mut module);
        }
        if let Some((minimum, maximum)) = self.memory {
            let mut bytes = vec![1 + u8::from(self.extra_memory)];
            limits(minimum, maximum, &mut bytes);
            if let Some(flags) = self.memory_flags {
                bytes[1] = flags;
            }
            if self.extra_memory {
                bytes.extend_from_slice(&[0, 1]);
            }
            section(5, bytes, &mut module);
        }
        // A mutable counter is available to state-isolation fixtures.
        section(6, vec![1, 0x7f, 1, 0x41, 0, 0x0b], &mut module);
        let mut exports = Vec::new();
        unsigned(self.exports.len() as u32, &mut exports);
        for export in &self.exports {
            text(export.name, &mut exports);
            exports.push(export.kind);
            unsigned(
                export.index
                    + if export.kind == 0 {
                        imported_functions
                    } else {
                        0
                    },
                &mut exports,
            );
        }
        section(7, exports, &mut module);
        if let Some(start) = self.start {
            let mut bytes = Vec::new();
            unsigned(start + imported_functions, &mut bytes);
            section(8, bytes, &mut module);
        }
        if let Some(table) = &self.table {
            if !table.elements.is_empty() {
                let mut bytes = vec![1, 0, 0x41, 0, 0x0b];
                unsigned(table.elements.len() as u32, &mut bytes);
                for function in &table.elements {
                    unsigned(*function + imported_functions, &mut bytes);
                }
                section(9, bytes, &mut module);
            }
        }
        let mut code = Vec::new();
        unsigned(self.functions.len() as u32, &mut code);
        for (index, function) in self.functions.iter().enumerate() {
            let mut body = vec![0]; // No additional locals.
            if index == 0 {
                body.extend_from_slice(self.info_code.as_deref().unwrap_or(&self.info_body()));
            } else {
                body.extend_from_slice(&function.code);
            }
            body.push(0x0b);
            unsigned(body.len() as u32, &mut code);
            code.extend_from_slice(&body);
        }
        section(10, code, &mut module);
        if let Some(offset) = self.data_offset {
            let mut bytes = vec![1, 0];
            i32_const(offset as i32, &mut bytes);
            bytes.extend_from_slice(&[0x0b, 1, 0xff]);
            section(11, bytes, &mut module);
        }
        module
    }

    pub fn info_body(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (index, value) in self.info.iter().enumerate() {
            bytes.extend_from_slice(&[0x20, 0]);
            i32_const(index as i32, &mut bytes);
            bytes.extend_from_slice(&[0x46, 0x04, 0x40]);
            i64_const(*value, &mut bytes);
            bytes.extend_from_slice(&[0x0f, 0x0b]);
        }
        i64_const(0, &mut bytes);
        bytes
    }
}

pub fn echo() -> Vec<u8> {
    let mut code = vec![0x20, 1, 0x20, 2, 0x4b, 0x04, 0x40];
    i64_const(u32::MAX as u64, &mut code); // Application status -1.
    code.extend_from_slice(&[0x0f, 0x0b]);
    i32_const(OUTPUT as i32, &mut code);
    i32_const(INPUT as i32, &mut code);
    code.extend_from_slice(&[0x20, 1, 0xfc, 0x0a, 0, 0, 0x20, 1, 0xad]);
    i64_const(32, &mut code);
    code.push(0x86);
    code
}

/// Write the top i32 operand to output[0..4], and report four output bytes.
/// The caller must push OUTPUT before computing that operand.
pub fn return_i32(code: &mut Vec<u8>) {
    code.extend_from_slice(&[0x36, 2, 0]);
    i64_const(4_u64 << 32, code);
}
