//! Windows Credential Manager adapter for Classic PAT storage.
//!
//! Stores generic credentials with `CRED_TYPE_GENERIC` and
//! `CRED_PERSIST_LOCAL_MACHINE`. The token is encoded as UTF-8 bytes in the
//! credential blob. Target names follow [`CredentialRef::target_name`] and
//! contain no secret material.
//!
//! Tests create and delete their own unique entries under a unique account id
//! and clean up even on failure, so they never read or modify any real saved
//! credential.

use crate::auth::credential::{CredentialError, CredentialRef, CredentialStore};
use crate::auth::token::Pat;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows::core::PCWSTR;

/// Windows Credential Manager backed store.
#[derive(Debug, Clone, Default)]
pub struct WindowsCredentialStore;

impl WindowsCredentialStore {
    pub fn new() -> Self {
        Self
    }
}

impl CredentialStore for WindowsCredentialStore {
    fn save(&self, reference: &CredentialRef, token: &Pat) -> Result<(), CredentialError> {
        let target_name = reference.target_name();
        let mut wide_target = to_wide(&target_name);
        let mut wide_user = to_wide("githux");
        let blob = token.expose().as_bytes().to_vec();

        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: windows::core::PWSTR(wide_target.as_mut_ptr()),
            UserName: windows::core::PWSTR(wide_user.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_ptr() as *mut _,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };

        // SAFETY: CredWriteW reads our buffers during the call; they stay valid.
        unsafe { CredWriteW(&credential, 0) }.map_err(map_err)
    }

    fn load(&self, reference: &CredentialRef) -> Result<Option<Pat>, CredentialError> {
        let target_name = reference.target_name();
        let mut wide_target = to_wide(&target_name);
        let mut raw_ptr: *mut CREDENTIALW = std::ptr::null_mut();

        // SAFETY: valid target name and output pointer.
        let read_result = unsafe {
            CredReadW(
                PCWSTR(wide_target.as_mut_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut raw_ptr,
            )
        };

        if let Err(e) = read_result {
            if is_not_found(&e) {
                return Ok(None);
            }
            return Err(map_err(e));
        }

        // SAFETY: raw_ptr is valid after success and must be freed with CredFree.
        unsafe {
            let credential = &*raw_ptr;
            let blob_len = credential.CredentialBlobSize as usize;
            let bytes =
                std::slice::from_raw_parts(credential.CredentialBlob as *const u8, blob_len);
            let result = std::str::from_utf8(bytes)
                .map(|s| Some(Pat::from_str(s)))
                .map_err(|_| CredentialError::Corrupted);
            CredFree(raw_ptr as *const _ as *const std::ffi::c_void);
            result
        }
    }

    fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialError> {
        let target_name = reference.target_name();
        let mut wide_target = to_wide(&target_name);

        // SAFETY: valid target name pointer.
        let result =
            unsafe { CredDeleteW(PCWSTR(wide_target.as_mut_ptr()), CRED_TYPE_GENERIC, None) };
        if let Err(e) = result {
            if is_not_found(&e) {
                // Idempotent delete.
                return Ok(());
            }
            return Err(map_err(e));
        }
        Ok(())
    }

    fn describe(&self) -> &'static str {
        "Windows Credential Manager"
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// The Credential APIs return Win32 error codes wrapped in `HRESULT_FROM_WIN32`,
/// so the low 16 bits carry the actual system error.
fn win32_code(hresult: windows::core::HRESULT) -> u32 {
    (hresult.0 & 0xFFFF) as u32
}

fn is_not_found(e: &windows::core::Error) -> bool {
    win32_code(e.code()) == ERROR_NOT_FOUND.0
}

fn map_err(e: windows::core::Error) -> CredentialError {
    let code = win32_code(e.code());
    CredentialError::Platform {
        message: format!("Windows Credential Manager error {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::HostId;

    /// Exercises the real Windows Credential Manager with a throwaway unique
    /// entry and cleans it up even on failure. This test only runs on Windows
    /// and never touches any pre-existing credential.
    #[test]
    fn windows_credential_roundtrip() {
        let store = WindowsCredentialStore::new();
        // Unique account id so we cannot collide with real accounts.
        let reference = CredentialRef::new(HostId::github(), 0xDEAD_BEEF_CAFE);
        let token = Pat::from_str("ghp_test_roundtrip_unique_token");

        // Cleanup guard ensures deletion even on assertion failure.
        let cleanup = scope_guard::guard((), |_| {
            let _ = store.delete(&reference);
        });

        // Start clean.
        let _ = store.delete(&reference);

        store.save(&reference, &token).expect("save should succeed");

        let loaded = store
            .load(&reference)
            .expect("load should succeed")
            .expect("credential should exist after save");
        assert_eq!(loaded.expose(), "ghp_test_roundtrip_unique_token");

        store.delete(&reference).expect("delete should succeed");
        assert!(
            store
                .load(&reference)
                .expect("load after delete should succeed")
                .is_none(),
            "credential should be gone after delete"
        );

        std::mem::forget(cleanup); // already cleaned up
    }
}

/// Minimal scope guard for cleanup-on-panic in tests.
mod scope_guard {
    pub struct Guard<T, F: FnOnce(T)> {
        value: Option<T>,
        cleanup: Option<F>,
    }

    pub fn guard<T, F: FnOnce(T)>(value: T, cleanup: F) -> Guard<T, F> {
        Guard {
            value: Some(value),
            cleanup: Some(cleanup),
        }
    }

    impl<T, F: FnOnce(T)> Drop for Guard<T, F> {
        fn drop(&mut self) {
            if let (Some(value), Some(cleanup)) = (self.value.take(), self.cleanup.take()) {
                cleanup(value);
            }
        }
    }
}
