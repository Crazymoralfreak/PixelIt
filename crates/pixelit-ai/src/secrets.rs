//! API keys, kept in the Windows Credential Manager.
//!
//! A key is stored per server host under the name `PixelIt:<host>`; it never
//! reaches the settings files, the logs or the interface.

use pixelit_core::{Error, Result};

#[cfg(windows)]
use windows::core::{HSTRING, PWSTR};
#[cfg(windows)]
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

/// Longest accepted key, in bytes.
const MAX_KEY: usize = 2048;

fn target(host: &str) -> String {
    format!("PixelIt:{}", host.trim().to_ascii_lowercase())
}

/// Stores the key of a host, replacing the previous one.
#[cfg(windows)]
pub fn set(host: &str, key: &str) -> Result<()> {
    let key = key.trim();
    if key.is_empty() || key.len() > MAX_KEY {
        return Err(Error::new("badKey"));
    }
    let mut name: Vec<u16> = target(host).encode_utf16().chain([0]).collect();
    let mut user: Vec<u16> = "PixelIt".encode_utf16().chain([0]).collect();
    let mut blob = key.as_bytes().to_vec();
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(name.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_mut_ptr()),
        ..CREDENTIALW::default()
    };
    // SAFETY: every pointer of the structure refers to a buffer that lives
    // until the call returns, and the sizes match the buffers.
    unsafe { CredWriteW(&raw const credential, 0) }
        .map_err(|error| Error::new("keyStoreFailed").detail(error))
}

/// The stored key of a host.
#[cfg(windows)]
pub fn get(host: &str) -> Option<String> {
    let name = HSTRING::from(target(host));
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `credential` receives a buffer owned by the system; it is read
    // while valid and released with `CredFree` below.
    unsafe {
        CredReadW(&name, CRED_TYPE_GENERIC, None, &raw mut credential).ok()?;
        if credential.is_null() {
            return None;
        }
        let size = (*credential).CredentialBlobSize as usize;
        let blob = (*credential).CredentialBlob;
        let key = if blob.is_null() || size == 0 || size > MAX_KEY {
            None
        } else {
            String::from_utf8(std::slice::from_raw_parts(blob, size).to_vec()).ok()
        };
        CredFree(credential.cast());
        key
    }
}

/// Removes the key of a host; a missing key is fine.
#[cfg(windows)]
pub fn delete(host: &str) {
    let name = HSTRING::from(target(host));
    // SAFETY: the name is a valid NUL-terminated wide string.
    let _ = unsafe { CredDeleteW(&name, CRED_TYPE_GENERIC, None) };
}

#[cfg(not(windows))]
pub fn set(_host: &str, _key: &str) -> Result<()> {
    Err(Error::new("keyStoreFailed"))
}

#[cfg(not(windows))]
pub fn get(_host: &str) -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn delete(_host: &str) {}

/// Whether a key is stored for the host.
pub fn has(host: &str) -> bool {
    get(host).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_normalized() {
        assert_eq!(target(" API.OpenAI.com "), "PixelIt:api.openai.com");
    }

    #[test]
    fn empty_keys_are_rejected() {
        assert!(set("example.invalid", "  ").is_err());
    }
}
