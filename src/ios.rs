unsafe extern "C" {
    fn spacemosaic_pick_folder(generation: u64);
    fn spacemosaic_install_accessibility();
    fn spacemosaic_configure(appearance: i32, language: *const std::ffi::c_char);
}

pub fn pick(generation: u64) -> bool {
    // Swift dispatches presentation to the main queue and always returns a result.
    unsafe { spacemosaic_pick_folder(generation) };
    true
}

/// # Safety
/// `bytes` must point to `length` readable bytes for this call. The bridge copies them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spacemosaic_deliver(bytes: *const u8, length: usize) -> bool {
    if bytes.is_null() || length > crate::bridge::MAX_MESSAGE_BYTES {
        return false;
    }
    // SAFETY: caller guarantees the buffer is alive for the duration of this call.
    let message = unsafe { std::slice::from_raw_parts(bytes, length) };
    crate::bridge::receive(message).unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn spacemosaic_run() {
    let options = eframe::NativeOptions {
        run_and_return: false,
        ..Default::default()
    };
    let _ = eframe::run_native(
        "SpaceMosaic",
        options,
        Box::new(|cc| Ok(Box::new(crate::ui::ExplorerApp::new(cc)))),
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn spacemosaic_begin() -> u64 {
    crate::bridge::begin()
}

pub fn install_accessibility() {
    // UIKit installs its native launcher once an actual winit window exists.
    unsafe { spacemosaic_install_accessibility() };
}

#[unsafe(no_mangle)]
pub extern "C" fn spacemosaic_is_current(generation: u64) -> bool {
    crate::bridge::is_current(generation)
}

pub fn configure(appearance: i32, language: &str) {
    if let Ok(language) = std::ffi::CString::new(language) {
        // Swift copies this null-terminated tag before dispatching to its main queue.
        unsafe { spacemosaic_configure(appearance, language.as_ptr()) };
    }
}
