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
        self.reserve_slot()?;
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

    #[cfg(feature = "std")]
    /// Unicode-capable counterpart of `reload` using a native filesystem path.
    /// # Safety
    /// The same native-code and cooperative-unload obligations as `reload` apply.
    pub unsafe fn reload_path<T>(
        &mut self,
        path: impl AsRef<std::path::Path>,
        policy: UnloadPolicy,
        prepare: impl FnOnce(&Rc<Plugin>) -> Result<T>,
    ) -> Result<T> {
        self.collect();
        self.reserve_slot()?;
        let candidate = match unsafe { Plugin::load_path(path, policy) } {
            Ok(plugin) => plugin,
            Err(error) => {
                if policy == UnloadPolicy::Pinned {
                    self.failed_pinned += 1;
                }
                return Err(error);
            }
        };
        self.replace(candidate, prepare)
    }

    fn reserve_slot(&mut self) -> Result<()> {
        if self.resident_retired() >= self.max_retired {
            return Err(Error::ResidentLimit);
        }
        self.retired
            .try_reserve(1)
            .map_err(|_| Error::ResidentLimit)
    }

    /// Commit an independently loaded candidate after successful initialization.
    /// The same image (including loader path aliases) is rejected.
    fn replace<T>(
        &mut self,
        candidate: Plugin,
        prepare: impl FnOnce(&Rc<Plugin>) -> Result<T>,
    ) -> Result<T> {
        if candidate.same_image(&self.current) {
            candidate.discard_duplicate(&self.current);
            return Err(Error::SamePath);
        }
        if let Some(old) = self.retired.iter().find(|old| candidate.same_image(old)) {
            candidate.discard_duplicate(old);
            return Err(Error::SamePath);
        }
        struct Staged<'a> {
            manager: &'a mut Generations,
            candidate: Option<Rc<Plugin>>,
        }
        impl Drop for Staged<'_> {
            fn drop(&mut self) {
                // Capacity was reserved before loading. Also runs during unwinding
                // in an alloc-only host; no candidate can escape resident accounting.
                if let Some(candidate) = self.candidate.take() {
                    self.manager.retired.push(candidate);
                }
            }
        }
        let mut staged = Staged {
            manager: self,
            candidate: Some(Rc::new(candidate)),
        };
        let prepared = crate::export::catch(|| prepare(staged.candidate.as_ref().unwrap()))?;
        let old = core::mem::replace(
            &mut staged.manager.current,
            staged.candidate.take().unwrap(),
        );
        staged.manager.retired.push(old);
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
                        self.retired.insert(index, Rc::new(plugin));
                        index += 1;
                    }
                },
                Err(plugin) => {
                    self.retired.insert(index, plugin);
                    index += 1;
                }
            }
        }
        closed
    }
}
