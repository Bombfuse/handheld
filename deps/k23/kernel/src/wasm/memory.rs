// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::sync::atomic::Ordering;

use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::types::MemoryType;
use crate::wasm::vm::{ExportedMemory, VMMemoryImport, VmPtr};

#[derive(Clone, Copy, Debug)]
pub struct Memory(Stored<ExportedMemory>);

impl Memory {
    pub fn ty(self, store: &StoreOpaque) -> MemoryType {
        let export = &store[self.0];
        MemoryType::from_wasm_memory(&export.memory)
    }

    /// Returns a shared slice of the memory data.
    pub fn data<'a>(self, store: &'a StoreOpaque) -> &'a [u8] {
        let export = &store[self.0];
        unsafe {
            let def = export.definition.as_ref();
            let len = def.current_length(Ordering::Relaxed);
            core::slice::from_raw_parts(def.base.as_ptr(), len)
        }
    }

    /// Returns a mutable slice of the memory data.
    pub fn data_mut<'a>(self, store: &'a mut StoreOpaque) -> &'a mut [u8] {
        let export = &store[self.0];
        unsafe {
            let def = export.definition.as_ref();
            let len = def.current_length(Ordering::Relaxed);
            core::slice::from_raw_parts_mut(def.base.as_ptr(), len)
        }
    }

    pub(super) fn from_exported_memory(store: &mut StoreOpaque, export: ExportedMemory) -> Self {
        let stored = store.add_memory(export);
        Self(stored)
    }
    pub(super) fn as_vmmemory_import(self, store: &mut StoreOpaque) -> VMMemoryImport {
        let export = &store[self.0];
        VMMemoryImport {
            from: VmPtr::from(export.definition),
            vmctx: VmPtr::from(export.vmctx),
            index: export.index,
        }
    }
}
