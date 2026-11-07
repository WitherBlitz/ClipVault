#![windows_subsystem = "windows"]
mod ui;
fn main() {
    if let Err(error) = ui::run() {
        let title = clipvault::clipboard::wide("ClipVault");
        let message = clipvault::clipboard::wide(&error);
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                0x10,
            );
        }
    }
}
