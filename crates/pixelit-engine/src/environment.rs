//! The search path for the programs PixelIt starts.
//!
//! On macOS a program started from Finder gets the short search path of the
//! system: no Homebrew, no Node.js or Python the user installed. A terminal
//! has them, because the login shell adds them. So the login shell is asked
//! for its search path once, and the programs PixelIt starts (MCP servers,
//! ComfyUI, Forge) are given that one. Windows and Linux hand the full path
//! to every program, and nothing is changed there.

use std::env;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

/// Printed before and after the search path: a shell may print a greeting
/// of its own around it.
const MARK: &str = "__PIXELIT_PATH__";

/// A shell that takes longer has something to ask; it is not waited for.
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);
const SHELL_POLL: Duration = Duration::from_millis(20);

/// The shell of a Mac whose user never chose another one.
const USUAL_SHELL: &str = "/bin/zsh";

/// Where Homebrew puts programs; looked into even when the shell did not answer.
const USUAL_FOLDERS: &str = "/opt/homebrew/bin:/usr/local/bin";

static SEARCH_PATH: OnceLock<OsString> = OnceLock::new();

/// The text between the two marks in what the shell printed.
fn between_marks(output: &str) -> Option<&str> {
    let rest = &output[output.find(MARK)? + MARK.len()..];
    let path = rest[..rest.find(MARK)?].trim();
    (!path.is_empty()).then_some(path)
}

/// Joins search paths: earlier lists first, every folder once.
fn merged(lists: &[&str]) -> String {
    let mut folders: Vec<&str> = Vec::new();
    for folder in lists.iter().flat_map(|list| list.split(':')) {
        if !folder.is_empty() && !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    folders.join(":")
}

/// Asks the login shell of the user for its search path.
fn ask_shell() -> Option<String> {
    let shell = env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| OsString::from(USUAL_SHELL));
    // `printenv` reads the path as programs get it, whatever the shell calls
    // its own variable.
    let script = format!("echo {MARK}; /usr/bin/printenv PATH; echo {MARK}");
    let mut child = Command::new(shell)
        .args(["-ilc", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut output = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = output.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < SHELL_TIMEOUT => thread::sleep(SHELL_POLL),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                // A program the shell left behind may hold the pipe open:
                // the reader is not waited for.
                return None;
            }
        }
    }
    let text = reader.join().ok()?;
    between_marks(&text).map(str::to_owned)
}

/// The search path for a program PixelIt starts; `None` where the one of
/// this process is already complete.
pub fn search_path() -> Option<&'static OsStr> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let path = SEARCH_PATH.get_or_init(|| {
        let own = env::var("PATH").unwrap_or_default();
        let shell = ask_shell().unwrap_or_default();
        OsString::from(merged(&[&shell, &own, USUAL_FOLDERS]))
    });
    Some(path.as_os_str())
}

/// Lets a program that is about to start find what a terminal would find.
/// Call it before the environment the user set for that program is added, so
/// a search path of the user's own wins.
pub fn with_search_path(command: &mut Command) {
    if let Some(path) = search_path() {
        command.env("PATH", path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_is_taken_from_between_the_marks() {
        let output = format!("Welcome!\n{MARK}\n/opt/homebrew/bin:/usr/bin\n{MARK}\nbye\n");
        assert_eq!(between_marks(&output), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(between_marks(&format!("{MARK}\n\n{MARK}")), None);
        assert_eq!(between_marks(&format!("{MARK}\n/usr/bin")), None);
        assert_eq!(between_marks("no marks at all"), None);
    }

    #[test]
    fn folders_are_joined_once_and_in_order() {
        assert_eq!(merged(&["/a:/b", "/b:/c", "", "/a:/d"]), "/a:/b:/c:/d");
        assert_eq!(merged(&["", ""]), "");
    }

    #[test]
    fn only_a_mac_gets_another_search_path() {
        assert_eq!(search_path().is_some(), cfg!(target_os = "macos"));
    }
}
