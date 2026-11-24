//! The "Open in PixelIt" item of the Explorer context menu, for pictures and
//! folders. The entries live in the part of the registry that belongs to the
//! current user, so no administrator rights are needed; the uninstaller
//! removes the same keys.

use std::path::Path;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegDeleteTreeW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
};

use pixelit_core::{Error, Result};

/// Registry keys of the menu item: every kind of picture, and folders.
const KEYS: [&str; 2] = [
    r"Software\Classes\SystemFileAssociations\image\shell\PixelIt",
    r"Software\Classes\Directory\shell\PixelIt",
];

fn failed(code: u32) -> Error {
    Error::new("registryFailed").with("code", code)
}

/// Writes a text value; the key is created when it does not exist.
fn set_value(key: &str, name: Option<&str>, value: &str) -> Result<()> {
    let key = HSTRING::from(key);
    let name = name.map(HSTRING::from);
    // A registry text is UTF-16 with the closing zero, counted in bytes.
    let data: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let bytes = u32::try_from(data.len() * 2).map_err(|_| failed(0))?;
    // SAFETY: the strings are zero-terminated and alive during the call;
    // `data` holds exactly `bytes` bytes.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &key,
            name.as_ref()
                .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr())),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            bytes,
        )
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(failed(status.0))
    }
}

fn remove_key(key: &str) -> Result<()> {
    // SAFETY: the string is zero-terminated and alive during the call.
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key)) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(failed(status.0))
    }
}

/// The command of the menu item: the program with the chosen path.
fn command(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", exe.display())
}

/// Adds or removes the menu item. `label` is its text in the language of the
/// interface.
pub fn set_context_menu(enabled: bool, label: &str) -> Result<()> {
    if !enabled {
        return KEYS.iter().try_for_each(|key| remove_key(key));
    }
    let exe = std::env::current_exe()?;
    for key in KEYS {
        set_value(key, None, label)?;
        set_value(key, Some("Icon"), &format!("\"{}\"", exe.display()))?;
        set_value(&format!(r"{key}\command"), None, &command(&exe))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_quotes_both_paths() {
        let line = command(Path::new(r"C:\Program Files\PixelIt\PixelIt.exe"));
        assert_eq!(line, r#""C:\Program Files\PixelIt\PixelIt.exe" "%1""#);
    }
}
