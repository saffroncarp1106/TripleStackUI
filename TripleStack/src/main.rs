#![windows_subsystem = "windows"]

mod ui;
mod window_motion;

fn main() {
    if let Err(error) = ui::run() {
        let title: Vec<u16> = "Triple Stack Skin Maker\0".encode_utf16().collect();
        let message: Vec<u16> = format!("{error:#}\0").encode_utf16().collect();
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MessageBoxW};
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_ICONERROR,
            );
        }
    }
}
