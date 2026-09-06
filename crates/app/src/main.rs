// Release builds are GUI-subsystem binaries: without this, launching
// `ide.exe` on Windows opens a console window first and leaves it behind the
// IDE for the whole session. Debug builds keep the console so `println!` and
// panic output stay visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

// Without an explicit AppUserModelID, Windows falls back to deriving the
// taskbar's app identity from the exe path — and for an unpackaged,
// unsigned, repeatedly-rebuilt-at-the-same-path binary like this one, that
// fallback is where the taskbar (unlike the title bar, which paints
// straight from the live window's HICON) is known to show a generic or
// blank icon even though the exe's own icon resource is fine. Must run
// before QApplication/any window is created.
#[cfg(windows)]
fn set_windows_app_user_model_id() {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let id: Vec<u16> = "FlorianKraemer.Kestrel\0".encode_utf16().collect();
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
    }
}

#[cfg(not(windows))]
fn set_windows_app_user_model_id() {}

fn main() -> ExitCode {
    set_windows_app_user_model_id();
    let exit_code = ui_shell::run_app();
    ExitCode::from(exit_code as u8)
}
