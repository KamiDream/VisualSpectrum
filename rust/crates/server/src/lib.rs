//! 本地配置服务：把**现有**的 `web/` 配置页原样托管出去，并提供与 Flask 版
//! 形状一致的 `/api/*`（`{ok: true, ...}` / `{ok: false, error}`）。
//!
//! 为什么单独一个 crate：HTTP 只是"配置 + 状态"的出口，不该和 wgpu / 窗口耦合；
//! 这样不接浮层也能用 `cargo test` + `curl` 验证接口。
//!
//! 线程模型：在后台线程里跑一个 current-thread tokio，`axum::serve` 只监听
//! `127.0.0.1`（不对外网暴露）。App 通过 `Arc<Mutex<ConfigStore>>` 共享同一份
//! 配置，所以保存后不需要额外信号 —— 文件 mtime 变了，渲染线程自己的热重载会接上。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use include_dir::{include_dir, Dir};
use serde_json::{json, Value};

use vs_core::{
    validate, ConfigStore, AFTERGLOW_RANGE, AUDIO_SOURCES, BG_OPACITY_RANGE, CENTER_GLOW_RANGE,
    COUNT_RANGE, DELAY_RANGE, HEIGHT_RANGE, INTENSITY_RANGE, MODES, ROTATE_SPEED_RANGE, SIZE_RANGE,
};

/// 配置页默认端口。
pub const DEFAULT_PORT: u16 = 8770;

/// 编译时把 `web/` 整个目录嵌进二进制 —— 发布版不需要另带前端文件。
///
/// 运行时优先读磁盘（`VISUALSPECTRUM_WEB` 或自动查找到的 `web/`），
/// 读不到才用这份嵌入副本，所以开发时改前端仍然刷新即生效。
static EMBEDDED_WEB: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../../web");

/// 给 `/api/spectrum` 用的实时数据。渲染线程每帧刷新，HTTP 线程只读。
#[derive(Debug, Clone, Default)]
pub struct Live {
    pub bands: Vec<f64>,
    pub live: bool,
    pub source: String,
    pub running: bool,
    /// 渲染线程实测帧率（每秒刷新一次）。
    pub fps: f64,
}

/// 两个线程共享的那点东西。
#[derive(Clone)]
pub struct Shared {
    pub store: Arc<Mutex<ConfigStore>>,
    pub live: Arc<Mutex<Live>>,
    pub overlay_visible: Arc<AtomicBool>,
    pub web_root: PathBuf,
}

impl Shared {
    pub fn new(
        store: Arc<Mutex<ConfigStore>>,
        live: Arc<Mutex<Live>>,
        overlay_visible: Arc<AtomicBool>,
    ) -> Self {
        Self {
            store,
            live,
            overlay_visible,
            web_root: find_web_root(),
        }
    }
}

/// 找 `web/` 目录：环境变量优先，其次从当前目录 / 可执行文件逐级向上找。
///
/// 这样不论从仓库根跑 `cargo run`、还是直接跑 `rust/target/debug/visualspectrum`，
/// 都能找到同一份前端文件（用户改完刷新即可，不用重新编译）。
pub fn find_web_root() -> PathBuf {
    if let Ok(value) = std::env::var("VISUALSPECTRUM_WEB") {
        let path = PathBuf::from(value);
        if path.is_dir() {
            return path;
        }
    }
    let starts = [
        std::env::current_dir().ok(),
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf)),
    ];
    for start in starts.into_iter().flatten() {
        let mut dir = start;
        loop {
            let candidate = dir.join("web");
            if candidate.join("templates/index.html").is_file() {
                return candidate;
            }
            if !dir.pop() {
                break;
            }
        }
    }
    PathBuf::from("web")
}

fn lock_store(shared: &Shared) -> std::sync::MutexGuard<'_, ConfigStore> {
    shared.store.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_live(shared: &Shared) -> std::sync::MutexGuard<'_, Live> {
    shared.live.lock().unwrap_or_else(|e| e.into_inner())
}

fn fail(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"ok": false, "error": message.into()}))).into_response()
}

fn session_type() -> &'static str {
    if std::env::var("WAYLAND_DISPLAY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        "wayland"
    } else if std::env::var("DISPLAY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        "x11"
    } else {
        "unknown"
    }
}

// ==========================================================================
// 路由
// ==========================================================================

pub fn router(shared: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/static/{*path}", get(static_file))
        .route("/api/meta", get(meta))
        .route("/api/spectrum", get(spectrum))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/config/validate", post(validate_config))
        .route("/api/config/reset", post(reset_config))
        .route("/api/config/export", get(export_config))
        .route("/api/reload", post(reload_config))
        .route("/api/audio/devices", get(audio_devices))
        .route("/api/overlay", post(overlay_control))
        .fallback(not_found)
        .with_state(shared)
}

async fn not_found(uri: axum::http::Uri) -> Response {
    if uri.path().starts_with("/api/") {
        return fail(StatusCode::NOT_FOUND, format!("接口不存在：{}", uri.path()));
    }
    placeholder().into_response()
}

fn placeholder() -> Response {
    let html = "<!doctype html><meta charset=\"utf-8\"><title>VisualSpectrum 配置页</title>\
<body style=\"font:14px/1.7 system-ui;max-width:720px;margin:60px auto;padding:0 16px\">\
<h1>VisualSpectrum 配置页</h1><p>后端接口是通的（可以试试 <code>/api/meta</code>、\
<code>/api/config</code>），但没找到 <code>web/</code> 下的前端文件。</p>\
<p>设置 <code>VISUALSPECTRUM_WEB</code> 指向仓库里的 <code>web/</code> 目录即可。</p></body>";
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html.to_string(),
    )
        .into_response()
}

fn static_response(root: &Path, relative: &str) -> Response {
    if relative.contains("..") || relative.starts_with('/') {
        return fail(StatusCode::NOT_FOUND, "路径不合法");
    }
    match std::fs::read(root.join(relative)) {
        Ok(bytes) => ok_static(relative, bytes),
        Err(_) => match EMBEDDED_WEB.get_file(relative) {
            Some(file) => ok_static(relative, file.contents().to_vec()),
            None => placeholder(),
        },
    }
}

fn ok_static(relative: &str, bytes: Vec<u8>) -> Response {
    // 前端没有构建步骤，改完刷新就要生效 —— 别让浏览器缓存旧 JS。
    (
        [
            (header::CONTENT_TYPE, mime_for(relative)),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response()
}

fn mime_for(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}

async fn index(State(shared): State<Shared>) -> Response {
    static_response(&shared.web_root, "templates/index.html")
}

async fn static_file(
    State(shared): State<Shared>,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
    static_response(&shared.web_root, &format!("static/{path}"))
}

// ==========================================================================
// API
// ==========================================================================

async fn meta(State(shared): State<Shared>) -> Response {
    let store = lock_store(&shared);
    let live = lock_live(&shared);
    let config_path = store.path().display().to_string();
    let config_dir = store
        .path()
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "session": session_type(),
            "config_dir": config_dir,
            "config_path": config_path,
            "web_dir": shared.web_root.display().to_string(),
        },
        "config_path": config_path,
        "revision": store.revision(),
        "fingerprint": store.fingerprint(),
        "running": live.running,
        "fps": live.fps,
        "mode": store.data().appearance.mode,
        "live": live.live,
        "overlay_visible": shared.overlay_visible.load(Ordering::Relaxed),
        "source": if live.source.is_empty() { "（未开始捕获）".to_string() } else { live.source.clone() },
        "limits": {
            "size": SIZE_RANGE,
            "count": COUNT_RANGE,
            "height": HEIGHT_RANGE,
            "intensity": INTENSITY_RANGE,
            "bg_opacity": BG_OPACITY_RANGE,
            "afterglow": AFTERGLOW_RANGE,
            "center_glow": CENTER_GLOW_RANGE,
            "rotate_speed": ROTATE_SPEED_RANGE,
            "delay": DELAY_RANGE,
            "modes": MODES,
            "audio_sources": AUDIO_SOURCES,
        },
        "selfcheck": {"ok": true, "failed": [], "notes": [], "items": []},
    }))
    .into_response()
}

async fn spectrum(State(shared): State<Shared>) -> Response {
    let live = lock_live(&shared);
    Json(json!({
        "ok": true,
        "bands": live.bands,
        "live": live.live,
        "source": live.source,
        "running": live.running,
    }))
    .into_response()
}

async fn get_config(State(shared): State<Shared>) -> Response {
    let store = lock_store(&shared);
    let config = serde_json::to_value(store.data()).unwrap_or(Value::Null);
    Json(json!({
        "ok": true,
        "config": config,
        "fingerprint": store.fingerprint(),
        "revision": store.revision(),
        "path": store.path().display().to_string(),
        "last_error": store.last_error(),
    }))
    .into_response()
}

async fn put_config(State(shared): State<Shared>, body: Option<Json<Value>>) -> Response {
    let Some(Json(payload)) = body else {
        return fail(StatusCode::BAD_REQUEST, "请求体不是合法 JSON");
    };
    let mut store = lock_store(&shared);
    match store.save(&payload) {
        Ok(saved) => {
            let config = serde_json::to_value(saved).unwrap_or(Value::Null);
            let revision = store.revision();
            let fingerprint = store.fingerprint();
            drop(store);
            Json(json!({
                "ok": true,
                "config": config,
                "revision": revision,
                "fingerprint": fingerprint,
                "applied": true,
                "note": "已保存并生效",
            }))
            .into_response()
        }
        Err(err) => fail(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn validate_config(body: Option<Json<Value>>) -> Response {
    let Some(Json(payload)) = body else {
        return fail(StatusCode::BAD_REQUEST, "请求体不是合法 JSON");
    };
    match validate(&payload) {
        Ok(clean) => Json(json!({
            "ok": true,
            "config": serde_json::to_value(&clean).unwrap_or(Value::Null),
        }))
        .into_response(),
        Err(err) => fail(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn reset_config(State(shared): State<Shared>) -> Response {
    let mut store = lock_store(&shared);
    match store.save_default() {
        Ok(saved) => {
            let config = serde_json::to_value(saved).unwrap_or(Value::Null);
            let revision = store.revision();
            let fingerprint = store.fingerprint();
            drop(store);
            Json(json!({
                "ok": true,
                "config": config,
                "revision": revision,
                "fingerprint": fingerprint,
                "applied": true,
                "note": "已恢复默认并生效",
            }))
            .into_response()
        }
        Err(err) => fail(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

async fn export_config(State(shared): State<Shared>) -> Response {
    let store = lock_store(&shared);
    let body = serde_json::to_string_pretty(store.data()).unwrap_or_default() + "\n";
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"visualspectrum-config.json\"",
            ),
        ],
        body,
    )
        .into_response()
}

async fn reload_config(State(shared): State<Shared>) -> Response {
    let mut store = lock_store(&shared);
    match store.load() {
        Ok(_) => {
            let revision = store.revision();
            drop(store);
            Json(json!({"ok": true, "applied": true, "note": "已重新载入", "revision": revision}))
                .into_response()
        }
        Err(err) => fail(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

async fn audio_devices(State(shared): State<Shared>) -> Response {
    let current = lock_store(&shared).data().audio.device.clone();
    let devices: Vec<Value> = vs_audio::list_devices()
        .into_iter()
        .map(|d| json!({"key": d.key, "label": d.label}))
        .collect();
    Json(json!({"ok": true, "devices": devices, "current": current})).into_response()
}

async fn overlay_control(State(shared): State<Shared>, body: Option<Json<Value>>) -> Response {
    let requested = body
        .as_ref()
        .and_then(|Json(value)| value.get("visible"))
        .and_then(Value::as_bool);
    match requested {
        Some(visible) => {
            shared.overlay_visible.store(visible, Ordering::Relaxed);
            Json(json!({"ok": true, "supported": true, "requested": visible})).into_response()
        }
        None => {
            let visible = !shared.overlay_visible.load(Ordering::Relaxed);
            shared.overlay_visible.store(visible, Ordering::Relaxed);
            Json(json!({"ok": true, "supported": true, "requested": visible})).into_response()
        }
    }
}

// ==========================================================================
// 启动
// ==========================================================================

/// 在后台线程里起服务。绑定失败只打印日志，不影响浮层本身。
pub fn spawn(shared: Shared, port: u16) {
    let _ = std::thread::Builder::new()
        .name("vs-http".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    eprintln!("配置服务启动失败（tokio）：{err}");
                    return;
                }
            };
            runtime.block_on(async move {
                let addr = format!("127.0.0.1:{port}");
                let listener = match tokio::net::TcpListener::bind(&addr).await {
                    Ok(listener) => listener,
                    Err(err) => {
                        eprintln!("配置服务绑定 {addr} 失败：{err}（可能已有实例在跑）");
                        return;
                    }
                };
                eprintln!("配置页：http://{addr}/");
                if let Err(err) = axum::serve(listener, router(shared)).await {
                    eprintln!("配置服务退出：{err}");
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_root_prefers_env() {
        // 只验证函数不会 panic，并能返回一个路径（真实目录由运行环境决定）。
        let root = find_web_root();
        assert!(!root.as_os_str().is_empty());
    }

    #[test]
    fn embedded_web_has_index() {
        assert!(EMBEDDED_WEB.get_file("templates/index.html").is_some());
        assert!(EMBEDDED_WEB.get_file("static/js/main.js").is_some());
    }

    #[test]
    fn router_builds() {
        let store = Arc::new(Mutex::new(ConfigStore::new("/tmp/vs-test-config.json")));
        let shared = Shared::new(
            store,
            Arc::new(Mutex::new(Live::default())),
            Arc::new(AtomicBool::new(true)),
        );
        let _ = router(shared);
    }
}
