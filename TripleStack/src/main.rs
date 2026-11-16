#![windows_subsystem = "windows"]

mod audio_examples;
mod audio_playback;
mod audio_presets;
mod audio_tab;
mod editor_search;
mod lazer;
mod lazer_browser;
mod number_font_panel;
mod skin_export;
mod ui;
mod window_motion;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args
        .get(1)
        .is_some_and(|arg| arg == "--compile-stable-presets")
    {
        let result = (|| -> anyhow::Result<()> {
            anyhow::ensure!(
                args.len() == 4,
                "Usage: --compile-stable-presets SKINS_DIRECTORY REPORT_JSON"
            );
            let report = audio_presets::PresetLibrary::default()
                .compile_stable(std::path::Path::new(&args[2]), |_| {})?;
            std::fs::write(&args[3], serde_json::to_vec_pretty(&report)?)?;
            anyhow::ensure!(
                report.errors.is_empty(),
                "Some skins could not be imported; see the report"
            );
            Ok(())
        })();
        if let Err(error) = result {
            if let Some(report) = args
                .get(3)
                .filter(|path| !std::path::Path::new(path).is_file())
            {
                let _ = std::fs::write(
                    report,
                    serde_json::json!({"error": format!("{error:#}")}).to_string(),
                );
            }
            std::process::exit(1);
        }
        return;
    }
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
