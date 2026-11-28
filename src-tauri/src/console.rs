//! The command line mode of a program built without a console.
//!
//! `PixelIt.exe` is a window application, so Windows starts it without
//! standard streams unless they are redirected. For commands typed in a
//! terminal the process attaches to the console of its parent; redirected
//! streams (pipes of an MCP client, `> file`) are left as they are.

#[cfg(windows)]
mod attached {
    use std::fs::OpenOptions;
    use std::os::windows::io::IntoRawHandle;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    fn missing(stream: STD_HANDLE) -> bool {
        // SAFETY: no preconditions; the handle is only inspected.
        match unsafe { GetStdHandle(stream) } {
            Ok(handle) => handle.is_invalid() || handle.0.is_null(),
            Err(_) => true,
        }
    }

    /// Points a missing standard stream at the console device.
    fn connect(stream: STD_HANDLE, device: &str, write: bool) {
        if !missing(stream) {
            return;
        }
        let Ok(file) = OpenOptions::new().read(true).write(write).open(device) else {
            return;
        };
        // The handle is handed over to the process for its whole life.
        let handle = HANDLE(file.into_raw_handle());
        // SAFETY: `handle` is a valid, open handle owned by nobody else.
        let _ = unsafe { SetStdHandle(stream, handle) };
    }

    pub fn attach() -> bool {
        if !missing(STD_OUTPUT_HANDLE) && !missing(STD_ERROR_HANDLE) && !missing(STD_INPUT_HANDLE) {
            return false;
        }
        // SAFETY: no preconditions. Fails when the parent has no console, for
        // example when started from Explorer; then there is nowhere to print.
        if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_err() {
            return false;
        }
        connect(STD_OUTPUT_HANDLE, "CONOUT$", true);
        connect(STD_ERROR_HANDLE, "CONOUT$", true);
        connect(STD_INPUT_HANDLE, "CONIN$", false);
        true
    }
}

/// Makes the standard streams usable for a command. Returns whether a
/// terminal was attached, in which case the caller ends its output with a
/// line break so the prompt of the terminal stays readable.
#[cfg(windows)]
pub fn attach() -> bool {
    attached::attach()
}

/// Elsewhere a program always has its streams.
#[cfg(not(windows))]
pub fn attach() -> bool {
    false
}
