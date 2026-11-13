//! Opt-in sandbox for untrusted Wasm32 plugins (requires Rust 1.86 and `alloc`).
//!
//! No imports, WASI, native pointers, shared memory, or start function are exposed.
//! Every guest invocation is fuel-metered. Linear memory, tables, execution stack,
//! module bytes and transfer buffers are bounded. These are not a process-RSS or
//! compilation-time limit; use OS process isolation where that threat model matters.
//! The sandbox depends on Wasmi's correctness and does not isolate native plugins.

#![forbid(unsafe_code)]

use alloc::string::{String, ToString};
use core::{fmt, marker::PhantomData, ops::Range};
use wasmi::{
    CompilationMode, Config, EnforcedLimits, Engine, ExternType, Instance, Memory, Module, Store,
    StoreLimits, StoreLimitsBuilder, TrapCode, TypedFunc,
};

use crate::wasm_guest::{ABI_VERSION, CALL_EXPORT, INFO_EXPORT};

/// Application-owned identifier and exact interface version; neither is an authorization token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Interface {
    pub id: u64,
    pub version: u32,
}

/// Per-instance policy. Defaults are finite; the engine and store are not exposed.
///
/// Memory is **guest linear memory**, not total RSS. The module-byte cap and strict
/// engine validation limits also apply, but compilation has no wall-clock deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_module_bytes: usize,
    pub max_memory_bytes: usize,
    pub max_table_elements: usize,
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    pub fuel_per_call: u64,
    /// Maximum interpreter value-stack size in bytes.
    pub max_stack_bytes: usize,
    /// Maximum guest call-stack depth.
    pub max_call_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_module_bytes: 4 * 1024 * 1024,
            max_memory_bytes: 16 * 1024 * 1024,
            max_table_elements: 4096,
            max_input_bytes: 1024 * 1024,
            max_output_bytes: 1024 * 1024,
            fuel_per_call: 10_000_000,
            max_stack_bytes: 64 * 1024,
            max_call_depth: 128,
        }
    }
}

impl Limits {
    fn validate(self) -> Result<()> {
        if self.max_module_bytes < 8 {
            return Err(Error::InvalidLimits(
                "module limit must allow an 8-byte header",
            ));
        }
        if self.max_memory_bytes == 0 || self.max_memory_bytes > u32::MAX as usize {
            return Err(Error::InvalidLimits(
                "linear-memory limit must fit a nonzero u32",
            ));
        }
        if self.max_input_bytes > self.max_memory_bytes
            || self.max_output_bytes > self.max_memory_bytes
        {
            return Err(Error::InvalidLimits(
                "transfer limits exceed linear-memory limit",
            ));
        }
        if self.fuel_per_call == 0 || self.max_stack_bytes < 1024 || self.max_call_depth == 0 {
            return Err(Error::InvalidLimits(
                "fuel/depth must be nonzero and stack at least 1024 bytes",
            ));
        }
        Ok(())
    }
}

/// Distinguishes policy, compatibility, application, and execution failures.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    InvalidLimits(&'static str),
    ModuleTooLarge { size: usize, limit: usize },
    InvalidModule(String),
    ImportsDenied,
    InvalidExport(&'static str),
    AbiVersion { found: u64 },
    InterfaceId { expected: u64, found: u64 },
    InterfaceVersion { expected: u32, found: u64 },
    InvalidBuffer(&'static str),
    InputTooLarge { size: usize, limit: usize },
    GuestStatus(i32),
    InvalidOutput { size: usize, limit: usize },
    FuelExhausted,
    StackExhausted,
    Trap(String),
    Instantiation(String),
    Poisoned,
}

pub type Result<T> = core::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimits(reason) => write!(f, "invalid Wasm policy: {reason}"),
            Self::ModuleTooLarge { size, limit } => {
                write!(f, "Wasm module has {size} bytes; limit {limit}")
            }
            Self::InvalidModule(reason) => {
                write!(f, "invalid or unsupported Wasm module: {reason}")
            }
            Self::ImportsDenied => {
                f.write_str("Wasm imports are forbidden; no host capabilities are granted")
            }
            Self::InvalidExport(name) => {
                write!(f, "Wasm export {name} is missing or has the wrong type")
            }
            Self::AbiVersion { found } => write!(f, "Wasm ABI {found}; expected {ABI_VERSION}"),
            Self::InterfaceId { expected, found } => {
                write!(f, "Wasm interface {found:#x}; expected {expected:#x}")
            }
            Self::InterfaceVersion { expected, found } => {
                write!(f, "Wasm interface version {found}; expected {expected}")
            }
            Self::InvalidBuffer(reason) => write!(f, "invalid Wasm scratch buffer: {reason}"),
            Self::InputTooLarge { size, limit } => {
                write!(f, "Wasm input has {size} bytes; limit {limit}")
            }
            Self::GuestStatus(status) => write!(f, "Wasm application returned status {status}"),
            Self::InvalidOutput { size, limit } => {
                write!(f, "Wasm returned {size} bytes; output capacity {limit}")
            }
            Self::FuelExhausted => f.write_str("Wasm execution exhausted its fuel budget"),
            Self::StackExhausted => f.write_str("Wasm execution exceeded its stack limit"),
            Self::Trap(reason) => write!(f, "Wasm execution trapped: {reason}"),
            Self::Instantiation(reason) => write!(f, "Wasm instantiation failed: {reason}"),
            Self::Poisoned => f.write_str("Wasm instance is poisoned; instantiate a fresh plugin"),
        }
    }
}

impl core::error::Error for Error {}

fn execution_error(error: wasmi::Error) -> Error {
    match error.as_trap_code() {
        Some(TrapCode::OutOfFuel) => Error::FuelExhausted,
        Some(TrapCode::StackOverflow) => Error::StackExhausted,
        _ => Error::Trap(error.to_string()),
    }
}

/// One isolated, stateful Wasm instance. All execution is serialized through `&mut self`.
///
/// This type is deliberately `!Send + !Sync`. Dropping it releases the interpreter
/// state without executing guest code. There are no implicit background tasks or
/// library leases. A trap or invalid success length permanently poisons the instance.
pub struct Plugin {
    store: Store<StoreLimits>,
    memory: Memory,
    call: TypedFunc<(i32, i32, i32), i64>,
    input: Range<usize>,
    output: Range<usize>,
    fuel: u64,
    poisoned: bool,
    _local: PhantomData<*mut ()>,
}

impl fmt::Debug for Plugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("wasm::Plugin")
            .field("input_capacity", &self.input_capacity())
            .field("output_capacity", &self.output_capacity())
            .field("memory_bytes", &self.memory_size())
            .field("poisoned", &self.poisoned)
            .finish_non_exhaustive()
    }
}

impl Plugin {
    /// Validate, instantiate and bind an untrusted binary `.wasm` plugin.
    ///
    /// All seven metadata queries share one fuel budget. This method accepts no
    /// native libraries, text-format modules, imports, or start sections. Loading
    /// does not grant file, network, clock, environment, or process capabilities.
    pub fn new(bytes: &[u8], interface: Interface, limits: Limits) -> Result<Self> {
        limits.validate()?;
        if bytes.len() > limits.max_module_bytes {
            return Err(Error::ModuleTooLarge {
                size: bytes.len(),
                limit: limits.max_module_bytes,
            });
        }
        if !bytes.starts_with(b"\0asm\x01\0\0\0") {
            return Err(Error::InvalidModule(
                "expected a binary WebAssembly core module v1".to_string(),
            ));
        }
        let mut config = Config::default();
        config
            .consume_fuel(true)
            .enforced_limits(EnforcedLimits::strict())
            .allow_start_fn(false)
            .ignore_custom_sections(true)
            .compilation_mode(CompilationMode::Eager)
            .wasm_multi_memory(false)
            .wasm_custom_page_sizes(false)
            .set_min_stack_height(1024)
            .set_max_stack_height(limits.max_stack_bytes)
            .set_max_recursion_depth(limits.max_call_depth)
            .set_max_cached_stacks(1);
        let engine = Engine::new(&config);
        let module =
            Module::new(&engine, bytes).map_err(|e| Error::InvalidModule(e.to_string()))?;
        if module.imports().next().is_some() {
            return Err(Error::ImportsDenied);
        }
        // Cargo can unify another dependency's `wasmi/memory64` feature. Enforce
        // our Wasm32 protocol independently of the engine's enabled features.
        if !matches!(module.get_export("memory"), Some(ExternType::Memory(ty)) if !ty.is_64()) {
            return Err(Error::InvalidExport("memory"));
        }
        let store_limits = StoreLimitsBuilder::new()
            .memory_size(limits.max_memory_bytes)
            .memories(1)
            .tables(1)
            .table_elements(limits.max_table_elements)
            .instances(1)
            .build();
        let mut store = Store::new(&engine, store_limits);
        store.limiter(|limits| limits);
        store
            .set_fuel(limits.fuel_per_call)
            .map_err(execution_error)?;
        // The validated configuration rejects start sections; imports is empty.
        let instance = Instance::new(&mut store, &module, &[])
            .map_err(|e| Error::Instantiation(e.to_string()))?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or(Error::InvalidExport("memory"))?;
        let info = instance
            .get_typed_func::<i32, i64>(&store, INFO_EXPORT)
            .map_err(|_| Error::InvalidExport(INFO_EXPORT))?;
        let call = instance
            .get_typed_func::<(i32, i32, i32), i64>(&store, CALL_EXPORT)
            .map_err(|_| Error::InvalidExport(CALL_EXPORT))?;
        let mut query = |field| {
            info.call(&mut store, field)
                .map(|n| n as u64)
                .map_err(execution_error)
        };
        let abi = query(0)?;
        if abi != ABI_VERSION {
            return Err(Error::AbiVersion { found: abi });
        }
        let id = query(1)?;
        if id != interface.id {
            return Err(Error::InterfaceId {
                expected: interface.id,
                found: id,
            });
        }
        let version = query(2)?;
        if version != u64::from(interface.version) {
            return Err(Error::InterfaceVersion {
                expected: interface.version,
                found: version,
            });
        }
        let input_ptr = query(3)?;
        let input_capacity = query(4)?;
        let output_ptr = query(5)?;
        let output_capacity = query(6)?;
        let memory_size = memory.data_size(&store);
        let input = buffer_range(
            input_ptr,
            input_capacity,
            memory_size,
            limits.max_input_bytes,
        )?;
        let output = buffer_range(
            output_ptr,
            output_capacity,
            memory_size,
            limits.max_output_bytes,
        )?;
        if !input.is_empty()
            && !output.is_empty()
            && input.start < output.end
            && output.start < input.end
        {
            return Err(Error::InvalidBuffer("input and output overlap"));
        }
        Ok(Self {
            store,
            memory,
            call,
            input,
            output,
            fuel: limits.fuel_per_call,
            poisoned: false,
            _local: PhantomData,
        })
    }

    /// Run one operation with a fresh fuel budget and bounded byte copies.
    ///
    /// The guest receives `min(output.len(), output_capacity())` writable bytes,
    /// initially zeroed. Host output is unchanged on any error. Positive and
    /// negative nonzero guest statuses are returned as `Error::GuestStatus` and
    /// do not poison the instance. Input-size errors also leave it usable.
    pub fn call(&mut self, operation: u32, input: &[u8], output: &mut [u8]) -> Result<usize> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        if input.len() > self.input_capacity() {
            return Err(Error::InputTooLarge {
                size: input.len(),
                limit: self.input_capacity(),
            });
        }
        let capacity = output.len().min(self.output_capacity());
        // Any unexpected internal failure also leaves this instance fail-closed.
        self.poisoned = true;
        self.store.set_fuel(self.fuel).map_err(execution_error)?;
        self.memory
            .write(&mut self.store, self.input.start, input)
            .map_err(|_| Error::InvalidBuffer("input is outside linear memory"))?;
        self.memory
            .data_mut(&mut self.store)
            .get_mut(self.output.start..self.output.start + capacity)
            .ok_or(Error::InvalidBuffer("output is outside linear memory"))?
            .fill(0);
        let result = self
            .call
            .call(
                &mut self.store,
                (operation as i32, input.len() as i32, capacity as i32),
            )
            .map_err(execution_error)? as u64;
        let status = result as i32;
        if status != 0 {
            self.poisoned = false;
            return Err(Error::GuestStatus(status));
        }
        let written = (result >> 32) as usize;
        if written > capacity {
            return Err(Error::InvalidOutput {
                size: written,
                limit: capacity,
            });
        }
        self.memory
            .read(&self.store, self.output.start, &mut output[..written])
            .map_err(|_| Error::InvalidBuffer("result is outside linear memory"))?;
        self.poisoned = false;
        Ok(written)
    }

    pub fn input_capacity(&self) -> usize {
        self.input.len()
    }
    pub fn output_capacity(&self) -> usize {
        self.output.len()
    }
    /// Current guest linear-memory bytes, excluding interpreter/host allocations.
    pub fn memory_size(&self) -> usize {
        self.memory.data_size(&self.store)
    }
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
    /// Remaining guest instruction budget after binding or the most recent call.
    pub fn fuel_remaining(&self) -> u64 {
        self.store.get_fuel().unwrap_or(0)
    }
}

fn buffer_range(pointer: u64, capacity: u64, memory: usize, limit: usize) -> Result<Range<usize>> {
    let start = u32::try_from(pointer)
        .map_err(|_| Error::InvalidBuffer("pointer does not fit wasm32"))? as usize;
    let size = u32::try_from(capacity)
        .map_err(|_| Error::InvalidBuffer("capacity does not fit wasm32"))? as usize;
    if size > limit {
        return Err(Error::InvalidBuffer("capacity exceeds host policy"));
    }
    let end = start
        .checked_add(size)
        .filter(|end| *end <= memory)
        .ok_or(Error::InvalidBuffer("range is outside linear memory"))?;
    Ok(start..end)
}
