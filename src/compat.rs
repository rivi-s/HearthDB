//! ClassicAPI interop.
//!
//! ClassicAPI hooks the same two engine functions we do, with its own copy of
//! MinHook. Two MinHook instances don't coordinate: whoever patches last wins,
//! because its trampoline holds whatever bytes were at the target when the hook
//! was *created*. We used to create ours in `DllMain`, before anyone had
//! patched, so our trampoline jumped straight into the engine and ClassicAPI's
//! detour was skipped entirely — its `C_*` globals silently never registered.
//!
//! MinHook relocates a `JMP rel32` prologue into the trampoline, so hooking a
//! detour that's already live chains onto it instead of replacing it. We just
//! have to go second. ClassicAPI's `Load` is an idempotent latched installer,
//! so we call it ourselves before hooking and stop caring about dlls.txt order.

use std::ffi::c_void;

/// ClassicAPI's `extern "C" DWORD Load()`, exported undecorated via its .def.
type ClassicApiLoad = unsafe extern "C" fn() -> u32;

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleA(module: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, proc_name: *const u8) -> *mut c_void;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClassicApi {
    Absent,
    Installed,
    /// Loaded, but we couldn't confirm its hooks are live. We install anyway;
    /// worst case is the unchained behaviour we had before.
    Unconfirmed,
}

impl ClassicApi {
    pub fn as_str(self) -> &'static str {
        match self {
            ClassicApi::Absent => "absent",
            ClassicApi::Installed => "installed",
            ClassicApi::Unconfirmed => "present, install unconfirmed",
        }
    }
}

/// Runs ClassicAPI's installer to completion so its detours are live before we
/// create ours. If another thread is mid-install this blocks until it finishes.
pub unsafe fn ensure_installed() -> ClassicApi {
    let module = GetModuleHandleA(b"ClassicAPI.dll\0".as_ptr());
    if module.is_null() {
        return ClassicApi::Absent;
    }

    let load = GetProcAddress(module, b"Load\0".as_ptr());
    if load.is_null() {
        return ClassicApi::Unconfirmed;
    }

    let load: ClassicApiLoad = std::mem::transmute(load);
    if load() == 0 {
        ClassicApi::Installed
    } else {
        ClassicApi::Unconfirmed
    }
}

/// True when `target` already starts with a `JMP rel32`, i.e. someone else's
/// detour is live there and ours will chain onto it. Diagnostic only.
pub unsafe fn is_detoured(target: usize) -> bool {
    std::ptr::read_volatile(target as *const u8) == 0xE9
}
