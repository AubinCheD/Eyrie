//! Android : dossier privé de l'application, nom de l'appareil, liens et vibration (par JNI), et le lecteur de QR
//! code (caméra du NDK, `camera2ndk` et `mediandk`, décodage par `rqrr`).

use std::ffi::{CStr, c_void};
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use jni::JavaVM;
use jni::objects::{JObject, JValue};
use slint::android::AndroidApp;

use crate::platform::{Frame, Notice};

static APP: OnceLock<AndroidApp> = OnceLock::new();

pub fn init(app: AndroidApp) {
    let _ = APP.set(app);
}

/// Appelle Java dans le fil courant, avec l'activité ; une exception Java est effacée et vaut `None`.
fn with_java<R>(f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
    let app = APP.get()?;
    // SAFETY : pointeurs donnés par android-activity, valables tant que l'application vit.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    let r = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    r.ok()
}

pub fn data_dir() -> PathBuf {
    APP.get().and_then(|a| a.internal_data_path()).unwrap_or_else(|| PathBuf::from("/data/local/tmp/eyrie"))
}

fn build_field(name: &str) -> Option<String> {
    with_java(|env, _| {
        let v = env.get_static_field("android/os/Build", name, "Ljava/lang/String;")?.l()?;
        let s: String = env.get_string(&v.into())?.into();
        Ok(s)
    })
}

pub fn device_model() -> String {
    let maker = build_field("MANUFACTURER").unwrap_or_default();
    let model = build_field("MODEL").unwrap_or_default();
    if model.to_lowercase().starts_with(&maker.to_lowercase()) { model } else { format!("{maker} {model}") }
}

/// Le nom donné au téléphone dans ses réglages, sinon son modèle.
pub fn device_name() -> String {
    let name = with_java(|env, activity| {
        let resolver =
            env.call_method(activity, "getContentResolver", "()Landroid/content/ContentResolver;", &[])?.l()?;
        let key = env.new_string("device_name")?;
        let v = env
            .call_static_method(
                "android/provider/Settings$Global",
                "getString",
                "(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&resolver), JValue::Object(&key)],
            )?
            .l()?;
        if v.is_null() {
            return Ok(String::new());
        }
        Ok(env.get_string(&v.into())?.into())
    });
    name.filter(|n| !n.trim().is_empty()).unwrap_or_else(device_model)
}

/// Ouvre un lien web dans le navigateur du téléphone.
pub fn open_url(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    with_java(|env, activity| {
        let s = env.new_string(url)?;
        let uri = env
            .call_static_method(
                "android/net/Uri",
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&s)],
            )?
            .l()?;
        let action = env.new_string("android.intent.action.VIEW")?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        env.call_method(activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    });
}

/// Une vibration brève (toucher d'une touche, appairage).
pub fn haptic() {
    with_java(|env, activity| {
        let name = env.new_string("vibrator")?;
        let vib = env
            .call_method(
                activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        if vib.is_null() {
            return Ok(());
        }
        let effect = env
            .call_static_method(
                "android/os/VibrationEffect",
                "createOneShot",
                "(JI)Landroid/os/VibrationEffect;",
                &[JValue::Long(12), JValue::Int(-1)],
            )?
            .l()?;
        env.call_method(&vib, "vibrate", "(Landroid/os/VibrationEffect;)V", &[JValue::Object(&effect)])?;
        Ok(())
    });
}

pub fn can_scan() -> bool {
    true
}

/// L'activité est-elle au premier plan ? Tenue par les événements d'android-activity (`lib.rs`).
static FOREGROUND: AtomicBool = AtomicBool::new(true);

pub fn set_foreground(yes: bool) {
    FOREGROUND.store(yes, Ordering::Relaxed);
}

pub fn in_background() -> bool {
    !FOREGROUND.load(Ordering::Relaxed)
}

const NOTIFICATIONS: &str = "android.permission.POST_NOTIFICATIONS";

/// Android 13 et suivants demandent la permission d'afficher des notifications ; demandée une fois appairé.
pub fn ask_notifications() {
    with_java(|env, activity| {
        let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;
        if sdk < 33 {
            return Ok(());
        }
        let p = env.new_string(NOTIFICATIONS)?;
        let granted =
            env.call_method(activity, "checkSelfPermission", "(Ljava/lang/String;)I", &[JValue::Object(&p)])?.i()? == 0;
        if !granted {
            let list = env.new_object_array(1, "java/lang/String", &p)?;
            env.call_method(
                activity,
                "requestPermissions",
                "([Ljava/lang/String;I)V",
                &[JValue::Object(&list), JValue::Int(8)],
            )?;
        }
        Ok(())
    });
}

/// Quelqu'un regarde le téléphone : écran allumé et déverrouillé.
fn someone_looking(env: &mut jni::JNIEnv, activity: &JObject) -> jni::errors::Result<bool> {
    const SERVICE: &str = "(Ljava/lang/String;)Ljava/lang/Object;";
    let power = env.new_string("power")?;
    let power = env.call_method(activity, "getSystemService", SERVICE, &[JValue::Object(&power)])?.l()?;
    let keyguard = env.new_string("keyguard")?;
    let keyguard = env.call_method(activity, "getSystemService", SERVICE, &[JValue::Object(&keyguard)])?.l()?;
    let on = env.call_method(&power, "isInteractive", "()Z", &[])?.z()?;
    let locked = env.call_method(&keyguard, "isKeyguardLocked", "()Z", &[])?.z()?;
    Ok(on && !locked)
}

/// Une notification du système : le détail si quelqu'un regarde le téléphone, sa forme discrète sinon (voir
/// `Notice::shown`).
pub fn notify(notice: &Notice) {
    with_java(|env, activity| {
        // Dans le doute (appel en échec), le téléphone est tenu pour verrouillé : la forme discrète.
        let looking = someone_looking(env, activity).unwrap_or(false);
        if env.exception_check()? {
            env.exception_clear()?;
        }
        let (title, body, visibility) = notice.shown(looking);
        let service = env.new_string("notification")?;
        let nm = env
            .call_method(
                activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&service)],
            )?
            .l()?;
        let channel_id = env.new_string("eyrie")?;
        let channel_name = env.new_string("Ce qui vous attend sur le PC")?;
        // IMPORTANCE_HIGH : une session Claude qui attend mérite d'apparaître en haut de l'écran.
        let channel = env.new_object(
            "android/app/NotificationChannel",
            "(Ljava/lang/String;Ljava/lang/CharSequence;I)V",
            &[JValue::Object(&channel_id), JValue::Object(&channel_name), JValue::Int(4)],
        )?;
        env.call_method(
            &nm,
            "createNotificationChannel",
            "(Landroid/app/NotificationChannel;)V",
            &[JValue::Object(&channel)],
        )?;
        let pm = env.call_method(activity, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[])?.l()?;
        let package = env.call_method(activity, "getPackageName", "()Ljava/lang/String;", &[])?.l()?;
        let launch = env
            .call_method(
                &pm,
                "getLaunchIntentForPackage",
                "(Ljava/lang/String;)Landroid/content/Intent;",
                &[JValue::Object(&package)],
            )?
            .l()?;
        // FLAG_IMMUTABLE | FLAG_UPDATE_CURRENT.
        let pending = env
            .call_static_method(
                "android/app/PendingIntent",
                "getActivity",
                "(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;",
                &[
                    JValue::Object(activity),
                    JValue::Int(0),
                    JValue::Object(&launch),
                    JValue::Int(0x0400_0000 | 0x0800_0000),
                ],
            )?
            .l()?;
        let builder = env.new_object(
            "android/app/Notification$Builder",
            "(Landroid/content/Context;Ljava/lang/String;)V",
            &[JValue::Object(activity), JValue::Object(&channel_id)],
        )?;
        let info =
            env.call_method(activity, "getApplicationInfo", "()Landroid/content/pm/ApplicationInfo;", &[])?.l()?;
        let icon = env.get_field(&info, "icon", "I")?.i()?;
        const B: &str = "Landroid/app/Notification$Builder;";
        env.call_method(&builder, "setSmallIcon", format!("(I){B}"), &[JValue::Int(icon)])?;
        let t = env.new_string(title)?;
        let b = env.new_string(body)?;
        env.call_method(&builder, "setContentTitle", format!("(Ljava/lang/CharSequence;){B}"), &[JValue::Object(&t)])?;
        env.call_method(&builder, "setContentText", format!("(Ljava/lang/CharSequence;){B}"), &[JValue::Object(&b)])?;
        // Sur la notification, pas sur le canal : les réglages d'un canal sont à l'utilisateur une fois créé.
        env.call_method(&builder, "setVisibility", format!("(I){B}"), &[JValue::Int(visibility)])?;
        env.call_method(
            &builder,
            "setContentIntent",
            format!("(Landroid/app/PendingIntent;){B}"),
            &[JValue::Object(&pending)],
        )?;
        env.call_method(&builder, "setAutoCancel", format!("(Z){B}"), &[JValue::Bool(1)])?;
        let n = env.call_method(&builder, "build", "()Landroid/app/Notification;", &[])?.l()?;
        let tag = env.new_string(&notice.tag)?;
        env.call_method(
            &nm,
            "notify",
            "(Ljava/lang/String;ILandroid/app/Notification;)V",
            &[JValue::Object(&tag), JValue::Int(1), JValue::Object(&n)],
        )?;
        Ok(())
    });
}

const CAMERA: &str = "android.permission.CAMERA";

fn camera_granted() -> bool {
    with_java(|env, activity| {
        let p = env.new_string(CAMERA)?;
        Ok(env.call_method(activity, "checkSelfPermission", "(Ljava/lang/String;)I", &[JValue::Object(&p)])?.i()? == 0)
    })
    .unwrap_or(false)
}

fn ask_camera() {
    with_java(|env, activity| {
        let p = env.new_string(CAMERA)?;
        let list = env.new_object_array(1, "java/lang/String", &p)?;
        env.call_method(
            activity,
            "requestPermissions",
            "([Ljava/lang/String;I)V",
            &[JValue::Object(&list), JValue::Int(7)],
        )?;
        Ok(())
    });
}

static SCANNING: AtomicBool = AtomicBool::new(false);

pub fn stop_scan() {
    SCANNING.store(false, Ordering::Relaxed);
}

/// Lit des QR codes par la caméra arrière jusqu'à `stop_scan` : l'image (pour le viseur) et chaque texte décodé.
pub fn start_scan(on_frame: Box<dyn Fn(Frame) + Send>, on_code: Box<dyn Fn(String) + Send>) {
    if SCANNING.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = std::thread::Builder::new().name("eyrie-camera".into()).spawn(move || {
        if !camera_granted() {
            ask_camera();
            // L'utilisateur répond à la demande du système.
            let deadline = Instant::now() + Duration::from_secs(90);
            while !camera_granted() {
                if !SCANNING.load(Ordering::Relaxed) || Instant::now() > deadline {
                    SCANNING.store(false, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
        // SAFETY : la caméra n'est utilisée que dans ce fil, et tout est libéré à la fin de `scan`.
        if let Err(e) = unsafe { camera::scan(&on_frame, &on_code) } {
            eprintln!("caméra : {e}");
        }
        SCANNING.store(false, Ordering::Relaxed);
    });
}

mod camera {
    use super::*;
    use ndk_sys as ndk;

    const WIDTH: i32 = 1280;
    const HEIGHT: i32 = 720;
    /// Largeur de l'image du viseur envoyée à l'interface.
    const PREVIEW: usize = 360;

    unsafe extern "C" fn on_device(_: *mut c_void, _: *mut ndk::ACameraDevice) {}
    unsafe extern "C" fn on_device_error(_: *mut c_void, _: *mut ndk::ACameraDevice, _: i32) {}
    unsafe extern "C" fn on_session(_: *mut c_void, _: *mut ndk::ACameraCaptureSession) {}

    fn ok(status: ndk::camera_status_t, what: &str) -> Result<(), String> {
        if status.0 == 0 { Ok(()) } else { Err(format!("{what} ({})", status.0)) }
    }

    /// La caméra arrière et l'orientation de son capteur (degrés).
    unsafe fn back_camera(mgr: *mut ndk::ACameraManager) -> Result<(std::ffi::CString, i32), String> {
        let mut list: *mut ndk::ACameraIdList = null_mut();
        ok(unsafe { ndk::ACameraManager_getCameraIdList(mgr, &mut list) }, "liste des caméras")?;
        let mut found = None;
        let n = unsafe { (*list).numCameras } as usize;
        for i in 0..n {
            let id = unsafe { *(*list).cameraIds.add(i) };
            let mut meta: *mut ndk::ACameraMetadata = null_mut();
            if unsafe { ndk::ACameraManager_getCameraCharacteristics(mgr, id, &mut meta) }.0 != 0 {
                continue;
            }
            let mut entry: ndk::ACameraMetadata_const_entry = unsafe { std::mem::zeroed() };
            let facing = if unsafe {
                ndk::ACameraMetadata_getConstEntry(meta, ndk::acamera_metadata_tag::ACAMERA_LENS_FACING.0, &mut entry)
            }
            .0 == 0
            {
                unsafe { *entry.data.u8_ }
            } else {
                255
            };
            let mut orientation = 90;
            if unsafe {
                ndk::ACameraMetadata_getConstEntry(
                    meta,
                    ndk::acamera_metadata_tag::ACAMERA_SENSOR_ORIENTATION.0,
                    &mut entry,
                )
            }
            .0 == 0
            {
                orientation = unsafe { *entry.data.i32_ };
            }
            unsafe { ndk::ACameraMetadata_free(meta) };
            // 1 : arrière ; à défaut, la première trouvée.
            if facing == 1 || found.is_none() {
                found = Some((unsafe { CStr::from_ptr(id) }.to_owned(), orientation));
                if facing == 1 {
                    break;
                }
            }
        }
        unsafe { ndk::ACameraManager_deleteCameraIdList(list) };
        found.ok_or_else(|| "aucune caméra".to_string())
    }

    pub unsafe fn scan(on_frame: &dyn Fn(Frame), on_code: &dyn Fn(String)) -> Result<(), String> {
        let mgr = unsafe { ndk::ACameraManager_create() };
        let (id, orientation) = unsafe { back_camera(mgr) }?;
        let mut reader: *mut ndk::AImageReader = null_mut();
        let format = ndk::AIMAGE_FORMATS::AIMAGE_FORMAT_YUV_420_888.0 as i32;
        if unsafe { ndk::AImageReader_new(WIDTH, HEIGHT, format, 3, &mut reader) }.0 != 0 {
            return Err("lecteur d'images".into());
        }
        let mut window: *mut ndk::ANativeWindow = null_mut();
        unsafe { ndk::AImageReader_getWindow(reader, &mut window) };
        let mut device_cb = ndk::ACameraDevice_StateCallbacks {
            context: null_mut(),
            onDisconnected: Some(on_device),
            onError: Some(on_device_error),
        };
        let mut device: *mut ndk::ACameraDevice = null_mut();
        ok(unsafe { ndk::ACameraManager_openCamera(mgr, id.as_ptr(), &mut device_cb, &mut device) }, "ouverture")?;
        let mut container: *mut ndk::ACaptureSessionOutputContainer = null_mut();
        let mut output: *mut ndk::ACaptureSessionOutput = null_mut();
        let mut target: *mut ndk::ACameraOutputTarget = null_mut();
        let mut request: *mut ndk::ACaptureRequest = null_mut();
        let mut session: *mut ndk::ACameraCaptureSession = null_mut();
        let session_cb = ndk::ACameraCaptureSession_stateCallbacks {
            context: null_mut(),
            onClosed: Some(on_session),
            onReady: Some(on_session),
            onActive: Some(on_session),
        };
        let setup = (|| -> Result<(), String> {
            unsafe {
                ok(ndk::ACaptureSessionOutputContainer_create(&mut container), "sorties")?;
                ok(ndk::ACaptureSessionOutput_create(window as *mut _, &mut output), "sortie")?;
                ok(ndk::ACaptureSessionOutputContainer_add(container, output), "sortie")?;
                ok(ndk::ACameraOutputTarget_create(window as *mut _, &mut target), "cible")?;
                ok(
                    ndk::ACameraDevice_createCaptureRequest(
                        device,
                        ndk::ACameraDevice_request_template::TEMPLATE_PREVIEW,
                        &mut request,
                    ),
                    "requête",
                )?;
                ok(ndk::ACaptureRequest_addTarget(request, target), "requête")?;
                // Mise au point continue : un QR code se lit de près.
                let af: u8 = 4;
                ndk::ACaptureRequest_setEntry_u8(request, ndk::acamera_metadata_tag::ACAMERA_CONTROL_AF_MODE.0, 1, &af);
                ok(ndk::ACameraDevice_createCaptureSession(device, container, &session_cb, &mut session), "session")?;
                ok(
                    ndk::ACameraCaptureSession_setRepeatingRequest(session, null_mut(), 1, &mut request, null_mut()),
                    "aperçu",
                )
            }
        })();
        let mut frames = 0u64;
        while setup.is_ok() && SCANNING.load(Ordering::Relaxed) {
            let mut image: *mut ndk::AImage = null_mut();
            if unsafe { ndk::AImageReader_acquireLatestImage(reader, &mut image) }.0 != 0 || image.is_null() {
                std::thread::sleep(Duration::from_millis(15));
                continue;
            }
            let (mut w, mut h, mut stride, mut len) = (0i32, 0i32, 0i32, 0i32);
            let mut data: *mut u8 = null_mut();
            unsafe {
                ndk::AImage_getWidth(image, &mut w);
                ndk::AImage_getHeight(image, &mut h);
                ndk::AImage_getPlaneRowStride(image, 0, &mut stride);
                ndk::AImage_getPlaneData(image, 0, &mut data, &mut len);
            }
            if !data.is_null() && w > 0 && h > 0 {
                // SAFETY : le plan Y de l'image, valable jusqu'à `AImage_delete`.
                let luma = unsafe { std::slice::from_raw_parts(data, len as usize) };
                let (w, h, stride) = (w as usize, h as usize, stride as usize);
                on_frame(preview(luma, w, h, stride, orientation));
                // Le décodage, une image sur deux : il coûte plus que l'aperçu.
                frames += 1;
                if frames % 2 == 0
                    && let Some(text) = decode(luma, w, h, stride)
                {
                    on_code(text);
                }
            }
            unsafe { ndk::AImage_delete(image) };
            std::thread::sleep(Duration::from_millis(30));
        }
        unsafe {
            if !session.is_null() {
                ndk::ACameraCaptureSession_stopRepeating(session);
                ndk::ACameraCaptureSession_close(session);
            }
            if !request.is_null() {
                ndk::ACaptureRequest_free(request);
            }
            if !target.is_null() {
                ndk::ACameraOutputTarget_free(target);
            }
            if !output.is_null() {
                ndk::ACaptureSessionOutput_free(output);
            }
            if !container.is_null() {
                ndk::ACaptureSessionOutputContainer_free(container);
            }
            ndk::ACameraDevice_close(device);
            ndk::AImageReader_delete(reader);
            ndk::ACameraManager_delete(mgr);
        }
        setup
    }

    /// Le texte du premier QR code trouvé dans le plan Y.
    fn decode(luma: &[u8], w: usize, h: usize, stride: usize) -> Option<String> {
        let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| luma[y * stride + x]);
        img.detect_grids().into_iter().find_map(|g| g.decode().ok().map(|(_, text)| text))
    }

    /// L'aperçu en niveaux de gris, tourné selon le capteur (portrait), réduit à `PREVIEW` de large.
    fn preview(luma: &[u8], w: usize, h: usize, stride: usize, orientation: i32) -> Frame {
        let rotated = orientation == 90 || orientation == 270;
        let (ow, oh) = if rotated { (h, w) } else { (w, h) };
        let scale = (ow as f32 / PREVIEW as f32).max(1.0);
        let (pw, ph) = ((ow as f32 / scale) as usize, (oh as f32 / scale) as usize);
        let mut buf = Frame::new(pw as u32, ph as u32);
        let px = buf.make_mut_slice();
        for y in 0..ph {
            for x in 0..pw {
                let (ox, oy) = ((x as f32 * scale) as usize, (y as f32 * scale) as usize);
                let (sx, sy) = match orientation {
                    90 => (oy, h - 1 - ox),
                    180 => (w - 1 - ox, h - 1 - oy),
                    270 => (w - 1 - oy, ox),
                    _ => (ox, oy),
                };
                let v = luma[(sy.min(h - 1)) * stride + sx.min(w - 1)];
                px[y * pw + x] = slint::Rgba8Pixel { r: v, g: v, b: v, a: 255 };
            }
        }
        buf
    }
}
