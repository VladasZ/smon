//! Detecting whether a serial port is already in use.
//!
//! A COM port allows only one open at a time, so opening it tells us if someone else holds it.
//! We open with zero desired access, which performs that check without initialising the UART, so
//! it never toggles DTR/RTS and never resets the attached device.
//!
//! Two smon instances polling at the same instant would each briefly hold the port and so report
//! the other as busy. [`Lock`] is a cross-process named mutex that serialises the probe pass, so
//! only one instance probes at a time and they never see each other.

#[cfg(windows)]
mod imp {
    use std::{
        ffi::{OsStr, c_void},
        os::windows::ffi::OsStrExt,
        ptr::null_mut,
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut c_void,
            disposition: u32,
            flags: u32,
            template: *mut c_void,
        ) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetLastError() -> u32;
        fn CreateMutexW(security: *mut c_void, initial_owner: i32, name: *const u16)
        -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
        fn ReleaseMutex(handle: *mut c_void) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain([0]).collect()
    }

    pub struct Lock {
        handle: *mut c_void,
        owned: bool,
    }

    impl Lock {
        pub fn acquire() -> Lock {
            const WAIT_OBJECT_0: u32 = 0;
            const WAIT_ABANDONED: u32 = 0x80;
            unsafe {
                let handle = CreateMutexW(null_mut(), 0, wide("smon_port_probe").as_ptr());
                let owned = if handle.is_null() {
                    false
                } else {
                    matches!(WaitForSingleObject(handle, 1000), WAIT_OBJECT_0 | WAIT_ABANDONED)
                };
                Lock { handle, owned }
            }
        }
    }

    impl Drop for Lock {
        fn drop(&mut self) {
            unsafe {
                if self.owned {
                    ReleaseMutex(self.handle);
                }
                if !self.handle.is_null() {
                    CloseHandle(self.handle);
                }
            }
        }
    }

    pub fn is_busy(port: &str) -> bool {
        const OPEN_EXISTING: u32 = 3;
        const ERROR_ACCESS_DENIED: u32 = 5;
        const ERROR_SHARING_VIOLATION: u32 = 32;
        let invalid = usize::MAX as *mut c_void; // INVALID_HANDLE_VALUE is -1

        unsafe {
            let handle = CreateFileW(
                wide(&format!(r"\\.\{port}")).as_ptr(),
                0,
                0,
                null_mut(),
                OPEN_EXISTING,
                0,
                null_mut(),
            );
            if handle == invalid {
                let err = GetLastError();
                return err == ERROR_ACCESS_DENIED || err == ERROR_SHARING_VIOLATION;
            }
            CloseHandle(handle);
            false
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct Lock;

    impl Lock {
        pub fn acquire() -> Lock {
            Lock
        }
    }

    pub fn is_busy(_port: &str) -> bool {
        false
    }
}

pub use imp::{Lock, is_busy};
