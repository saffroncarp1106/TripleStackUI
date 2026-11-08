use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{cell::Cell, rc::Rc};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCDESTROY},
    },
};

pub struct WindowMotion {
    state: Rc<State>,
}

struct State {
    window: Cell<HWND>,
    active: Cell<bool>,
    ctx: egui::Context,
}

impl WindowMotion {
    pub fn attach(creation: &eframe::CreationContext<'_>) -> std::io::Result<Self> {
        let handle = creation.window_handle().map_err(std::io::Error::other)?;
        let RawWindowHandle::Win32(window) = handle.as_raw() else {
            return Err(std::io::Error::other("Missing the Windows window handle"));
        };
        let window = window.hwnd.get() as HWND;
        let state = Rc::new(State {
            window: Cell::new(window),
            active: Cell::new(false),
            ctx: creation.egui_ctx.clone(),
        });
        // Keep the callback's state alive until the subclass is removed.
        let data = Rc::as_ptr(&state) as usize;
        if unsafe { SetWindowSubclass(window, Some(window_proc), 0, data) } == 0 {
            return Err(std::io::Error::other(
                "Could not subscribe to window drag events",
            ));
        }
        Ok(Self { state })
    }

    pub fn active(&self) -> bool {
        self.state.active.get()
    }
}

impl Drop for WindowMotion {
    fn drop(&mut self) {
        let window = self.state.window.get();
        if !window.is_null() {
            unsafe { RemoveWindowSubclass(window, Some(window_proc), 0) };
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    let state = unsafe { &*(data as *const State) };
    match message {
        WM_ENTERSIZEMOVE => state.active.set(true),
        WM_EXITSIZEMOVE => {
            state.active.set(false);
            state.ctx.request_repaint();
        }
        WM_NCDESTROY => {
            state.window.set(std::ptr::null_mut());
            unsafe { RemoveWindowSubclass(window, Some(window_proc), id) };
        }
        _ => {}
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}
