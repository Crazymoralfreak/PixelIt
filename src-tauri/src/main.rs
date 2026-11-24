// No console window in release builds: the window mode never needs one, the
// command line attaches to the terminal it was started from.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    std::process::exit(pixelit_lib::run());
}
