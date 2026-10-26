use jni::{
    JNIEnv, JavaVM,
    objects::{JClass, JObject, JString, JValue},
};
use std::sync::OnceLock;
use winit::platform::android::activity::AndroidApp;

static APP: OnceLock<AndroidApp> = OnceLock::new();

pub fn pick(generation: u64) -> bool {
    let Some(app) = APP.get() else {
        return false;
    };
    // SAFETY: AndroidApp owns the VM and Activity references for its lifetime.
    let Ok(vm) = (unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }) else {
        return false;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return false;
    };
    // The borrowed global Activity reference must not be deleted by Rust.
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = env.call_method(
        &activity,
        "pickTree",
        "(J)V",
        &[JValue::Long(generation as i64)],
    );
    if result.is_err() {
        let _ = env.exception_clear();
    }
    result.is_ok()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_spacemosaic_StorageActivity_deliver(
    mut env: JNIEnv<'_>,
    _: JClass<'_>,
    json: JString<'_>,
) -> jni::sys::jboolean {
    let Ok(message) = env.get_string(&json) else {
        return 0;
    };
    let message: String = message.into();
    u8::from(crate::bridge::receive(message.as_bytes()).unwrap_or(false))
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    let _ = APP.set(app.clone());
    let options = eframe::NativeOptions {
        android_app: Some(app),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "SpaceMosaic",
        options,
        Box::new(|cc| Ok(Box::new(crate::ui::ExplorerApp::new(cc)))),
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_spacemosaic_StorageActivity_begin(
    _: JNIEnv<'_>,
    _: JClass<'_>,
) -> jni::sys::jlong {
    crate::bridge::begin() as i64
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_spacemosaic_StorageActivity_isCurrent(
    _: JNIEnv<'_>,
    _: JClass<'_>,
    generation: jni::sys::jlong,
) -> jni::sys::jboolean {
    u8::from(crate::bridge::is_current(generation as u64))
}

pub fn configure(appearance: i32, language: &str) {
    let Some(app) = APP.get() else {
        return;
    };
    // AndroidApp retains these VM and Activity references.
    let Ok(vm) = (unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }) else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let Ok(tag) = env.new_string(language) else {
        return;
    };
    let result = env.call_method(
        &activity,
        "configureAppearance",
        "(ILjava/lang/String;)V",
        &[JValue::Int(appearance), JValue::Object(tag.as_ref())],
    );
    if result.is_err() {
        let _ = env.exception_clear();
    }
    let _ = env.delete_local_ref(tag);
}
