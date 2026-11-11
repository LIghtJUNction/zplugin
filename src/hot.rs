//! Request-boundary generations. Existing leases continue using their original code.
use crate::{Error, Plugin, Result, UnloadPolicy};
use alloc::{rc::Rc, vec::Vec};
use core::ffi::CStr;

/// The manager is single-threaded. Acquire one lease per request/session and reuse
/// it for all calls. Old generations count toward the bound until safely closed.
pub struct Generations {
    current: Rc<Plugin>,
    retired: Vec<Rc<Plugin>>,
    max_retired: usize,
    failed_pinned: usize,
}

impl Generations {
    pub fn new(current: Plugin, max_retired: usize) -> Self {
        Self {
            current: Rc::new(current),
            retired: Vec::new(),
            max_retired,
            failed_pinned: 0,
        }
    }

    /// Acquire once at a request or session boundary; individual calls do not clone.
    pub fn current(&self) -> Rc<Plugin> {
        Rc::clone(&self.current)
    }
    pub fn retired_count(&self) -> usize {
        self.retired.len()
    }
    pub fn max_retired(&self) -> usize {
        self.max_retired
    }
    /// Includes failed pinned load attempts, conservatively counted as resident.
    pub fn resident_retired(&self) -> usize {
        self.retired.len() + self.failed_pinned
    }

    /// Load from a separate path, validate/create/restore in `prepare`, then commit.
    /// Any failure leaves the current generation unchanged. The caller passes a
    /// versioned snapshot explicitly through the closure when state is involved.
    /// # Safety
    /// The trust and unload contracts of Plugin::load[_cooperative] apply.
    pub unsafe fn reload<T>(
        &mut self,
        path: &CStr,
        policy: UnloadPolicy,
        prepare: impl FnOnce(&Rc<Plugin>) -> Result<T>,
    ) -> Result<T> {
        self.collect();
        if self.resident_retired() >= self.max_retired {
            return Err(Error::ResidentLimit);
        }
        let candidate = match policy {
            UnloadPolicy::Pinned => match unsafe { Plugin::load(path) } {
                Ok(plugin) => plugin,
                Err(error) => {
                    self.failed_pinned += 1;
                    return Err(error);
                }
            },
            UnloadPolicy::Cooperative => unsafe { Plugin::load_cooperative(path)? },
        };
        self.replace(candidate, prepare)
    }

    /// Commit an independently loaded candidate after successful initialization.
    /// The same image (including loader path aliases) is rejected.
    pub fn replace<T>(
        &mut self,
        candidate: Plugin,
        prepare: impl FnOnce(&Rc<Plugin>) -> Result<T>,
    ) -> Result<T> {
        self.collect();
        if self.resident_retired() >= self.max_retired {
            return Err(Error::ResidentLimit);
        }
        if candidate.same_image(&self.current) {
            candidate.discard_duplicate(&self.current);
            return Err(Error::SamePath);
        }
        if let Some(old) = self.retired.iter().find(|old| candidate.same_image(old)) {
            candidate.discard_duplicate(old);
            return Err(Error::SamePath);
        }
        let candidate = Rc::new(candidate);
        let prepared = match prepare(&candidate) {
            Ok(value) => value,
            Err(error) => {
                // A failing closure may have exported a lease or a panicking
                // factory may pin its image. Count it until genuinely reclaimed.
                self.retired.push(candidate);
                self.collect();
                return Err(error);
            }
        };
        let old = core::mem::replace(&mut self.current, candidate);
        self.retired.push(old);
        Ok(prepared)
    }

    /// Close only generations with no leases and a successful quiescence response.
    /// Busy/pinned generations stay resident and continue counting toward the cap.
    pub fn collect(&mut self) -> usize {
        let mut closed = 0;
        let mut index = 0;
        while index < self.retired.len() {
            if Rc::strong_count(&self.retired[index]) != 1
                || self.retired[index].unload_policy() != UnloadPolicy::Cooperative
            {
                index += 1;
                continue;
            }
            let candidate = self.retired.swap_remove(index);
            match Rc::try_unwrap(candidate) {
                Ok(plugin) => match plugin.try_close() {
                    Ok(()) => closed += 1,
                    Err((_, plugin)) => {
                        self.retired.push(Rc::new(plugin));
                        index += 1;
                    }
                },
                Err(plugin) => {
                    self.retired.push(plugin);
                    index += 1;
                }
            }
        }
        closed
    }
}
