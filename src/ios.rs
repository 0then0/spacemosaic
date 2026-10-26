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

unsafe extern "C" {
    fn spacemosaic_provider(request: *const std::ffi::c_char) -> *mut std::ffi::c_char;
    fn spacemosaic_provider_free(response: *mut std::ffi::c_char);
}
pub fn request(request: &str) -> Result<String, String> {
    let request = std::ffi::CString::new(request).map_err(|_| "metadata_unavailable")?;
    // SAFETY: Swift returns an owned, NUL-terminated UTF-8 allocation. Copy it
    // before invoking the matching native deallocator, on every return path.
    unsafe {
        let response = spacemosaic_provider(request.as_ptr());
        if response.is_null() {
            return Err("provider_unavailable".into());
        }
        let result = std::ffi::CStr::from_ptr(response)
            .to_str()
            .map(str::to_owned)
            .map_err(|_| "metadata_unavailable".to_owned());
        spacemosaic_provider_free(response);
        result
    }
}
/// # Safety
/// `request` points to a readable NUL-terminated UTF-8 string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spacemosaic_result_page(
    request: *const std::ffi::c_char,
) -> *mut std::ffi::c_char {
    if request.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: native caller owns the input buffer for the duration of the call.
    let request = unsafe { std::ffi::CStr::from_ptr(request) }.to_string_lossy();
    std::ffi::CString::new(crate::mobile::page(&request))
        .map(std::ffi::CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// `value` is null or an allocation returned by `spacemosaic_result_page`, freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spacemosaic_free_result(value: *mut std::ffi::c_char) {
    if !value.is_null() {
        drop(unsafe { std::ffi::CString::from_raw(value) });
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn spacemosaic_export_done(id: u64, outcome: i32) {
    crate::mobile::export_complete(id, crate::mobile::ExportOutcome::code(outcome));
}
