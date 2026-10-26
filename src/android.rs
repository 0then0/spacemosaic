use jni::{
    Env, EnvUnowned, JavaVM,
    errors::{Result, ThrowRuntimeExAndDefault},
    jni_sig, jni_str,
    objects::{JClass, JObject, JString, JValue},
    refs::Global,
};
use std::sync::{Arc, Mutex, OnceLock};
use winit::platform::android::activity::AndroidApp;

static APP: OnceLock<Mutex<Option<AndroidApp>>> = OnceLock::new();
fn current_app() -> Option<AndroidApp> {
    APP.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

fn with_activity(action: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<()>) -> bool {
    let Some(app) = current_app() else {
        return false;
    };
    with_activity_on(&app, action)
}
fn with_activity_on(
    app: &AndroidApp,
    action: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<()>,
) -> bool {
    // SAFETY: AndroidApp retains this live VM for the duration of the callback.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env| -> Result<()> {
        let raw_activity = app.activity_as_ptr().cast();
        // SAFETY: AndroidApp owns this global reference. The cast only borrows it
        // within the attached frame; it neither deletes nor takes ownership of it.
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw_activity)? };
        action(env, &activity)
    })
    .is_ok()
}

pub fn pick(generation: u64) -> bool {
    with_activity(|env, activity| {
        env.call_method(
            activity,
            jni_str!("pickTree"),
            jni_sig!("(J)V"),
            &[JValue::Long(generation as i64)],
        )?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_spacemosaic_StorageActivity_deliver<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    json: JString<'local>,
) -> jni::sys::jboolean {
    env.with_env(|env| -> Result<bool> {
        let message = json.try_to_string(env)?;
        Ok(crate::bridge::receive(message.as_bytes()).unwrap_or(false))
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    *APP.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(app.clone());
    crate::bridge::begin();
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
pub extern "system" fn Java_dev_spacemosaic_StorageActivity_begin<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
) -> jni::sys::jlong {
    env.with_env(|_| -> Result<_> { Ok(crate::bridge::begin() as i64) })
        .resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_spacemosaic_StorageActivity_isCurrent<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    generation: jni::sys::jlong,
) -> jni::sys::jboolean {
    env.with_env(|_| -> Result<bool> { Ok(crate::bridge::is_current(generation as u64)) })
        .resolve::<ThrowRuntimeExAndDefault>()
}

pub fn configure(appearance: i32, language: &str) {
    with_activity(|env, activity| {
        let tag = JString::from_str(env, language)?;
        env.call_method(
            activity,
            jni_str!("configureAppearance"),
            jni_sig!("(ILjava/lang/String;)V"),
            &[JValue::Int(appearance), JValue::Object(tag.as_ref())],
        )?;
        // The attachment's local frame releases the string reference on return.
        Ok(())
    });
}

// Capture the Activity for each scan. Recreation cannot redirect an old cursor's
// close/cancel operations into the next Activity instance.
pub fn storage_transport() -> crate::provider::Transport {
    let app = current_app();
    Arc::new(move |request| {
        let Some(app) = &app else {
            return Err("provider_unavailable".into());
        };
        let mut response = None;
        if with_activity_on(app, |env, activity| {
            let request = JString::from_str(env, request)?;
            let value = env
                .call_method(
                    activity,
                    jni_str!("provider"),
                    jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
                    &[JValue::Object(request.as_ref())],
                )?
                .l()?;
            let text = env.cast_local::<JString>(value)?;
            response = Some(text.try_to_string(env)?);
            Ok(())
        }) {
            response.ok_or_else(|| "provider_unavailable".into())
        } else {
            Err("provider_unavailable".into())
        }
    })
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_spacemosaic_StorageActivity_resultPage<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    request: JString<'local>,
) -> jni::sys::jstring {
    env.with_env(|env| -> Result<_> {
        let request = request.try_to_string(env)?;
        Ok(JString::from_str(env, crate::mobile::page(&request))?.into_raw())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_spacemosaic_StorageActivity_exportDone<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    id: jni::sys::jlong,
    outcome: jni::sys::jint,
) {
    env.with_env(|_| -> Result<()> {
        crate::mobile::export_complete(id as u64, crate::mobile::ExportOutcome::code(outcome));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_spacemosaic_StorageActivity_back<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
) -> jni::sys::jboolean {
    env.with_env(|_| -> Result<bool> { Ok(crate::mobile::back()) })
        .resolve::<ThrowRuntimeExAndDefault>()
}
