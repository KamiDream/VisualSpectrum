//! 配置层：schema / 默认值 / 严格校验 / 读写 / 热重载。
//!
//! 字段名与配置文件、`/api/*`、`web/` 配置页共用同一套 —— 改字段时要三处同步。
//!
//! 两条硬性约定：
//! 1. **校验一定发生在写入之前**，而且 [`validate`] 是纯函数（不碰磁盘）；
//! 2. **校验失败绝不破坏已有配置** —— [`ConfigStore::load`] 出错时保留上一次的好数据。

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

// ==========================================================================
// 常量（schema 与配置页共用）
// ==========================================================================

/// 配置结构版本（结构不兼容变化时 +1；比程序新就直接拒绝）。
pub const VERSION: i64 = 1;

/// 两种模式：顶层（浮在最上层、鼠标穿透）与壁纸（贴桌面底层）。
pub const MODES: [&str; 2] = ["top", "wallpaper"];

/// 音频来源：抓系统声音 / 不碰任何设备（只放演示波形）。
pub const AUDIO_SOURCES: [&str; 2] = ["system", "off"];

/// 大小：环的外径占屏幕短边的百分比（允许 >100 溢出屏幕）。
pub const SIZE_RANGE: (i64, i64) = (5, 200);
/// 数量：环上频谱块的个数。
pub const COUNT_RANGE: (i64, i64) = (4, 512);
/// 条高：条最长 = 环半径的百分比；>100 不封顶。
pub const HEIGHT_RANGE: (i64, i64) = (5, 500);
/// 强度 / 灵敏度。
pub const INTENSITY_RANGE: (i64, i64) = (10, 1000);
/// 低 / 中 / 高音占比。
pub const MIX_RANGE: (i64, i64) = (0, 400);
/// 背景不透明度（0 = 全透明）。
pub const BG_OPACITY_RANGE: (i64, i64) = (0, 100);
/// 余晖（荧光拖尾）强度。
pub const AFTERGLOW_RANGE: (i64, i64) = (0, 100);
/// 中心光晕（外圈光晕 + X 形油光）强度；100 = 原设计亮度。
pub const CENTER_GLOW_RANGE: (i64, i64) = (0, 200);
/// 自转速度：100 = 基准（0.4 rad/s ≈ 16 秒一圈）。
pub const ROTATE_SPEED_RANGE: (i64, i64) = (10, 300);
/// 粒子的速度 / 寿命倍数。
pub const PARTICLE_RANGE: (i64, i64) = (20, 400);
/// 鼓点灵敏度。
pub const SENSITIVITY_RANGE: (i64, i64) = (0, 100);
/// CRT 后处理（桶形畸变 / RGB 分色）强度。
pub const CRT_FX_RANGE: (i64, i64) = (0, 100);
/// 启动延迟（毫秒）。
pub const DELAY_RANGE: (i64, i64) = (0, 10000);

/// 色标最多几个颜色。
pub const COLORS_MAX: usize = 8;
/// 默认色标（青 → 紫 → 品红）。
pub const DEFAULT_COLORS: [&str; 3] = ["#7EE7FF", "#B37BFF", "#FF6FD8"];

// ==========================================================================
// 错误
// ==========================================================================

/// 配置不合法。消息是给用户看的中文，会原样出现在配置页的提示里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(err: std::io::Error) -> Self {
        ConfigError(err.to_string())
    }
}

type Result<T> = std::result::Result<T, ConfigError>;

fn fail<T>(path: &str, message: impl fmt::Display) -> Result<T> {
    Err(ConfigError(format!("配置项 {path} {message}")))
}

// ==========================================================================
// 类型化 schema
// ==========================================================================

/// 外观段（大小 / 数量 / 条高 / 强度 / 连线 / 旋转 / 颜色 / 背景 / CRT / 中心光晕 / 粒子 / 模式）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Appearance {
    pub size: i64,
    pub count: i64,
    pub height: i64,
    pub intensity: i64,
    pub bass: i64,
    pub mid: i64,
    pub treble: i64,
    pub link: bool,
    pub rotate: bool,
    /// 自转速度百分比（100 = 基准 0.4 rad/s）。
    pub rotate_speed: i64,
    pub colors: Vec<String>,
    pub bg_color: String,
    pub bg_opacity: i64,
    pub crt: bool,
    pub afterglow: i64,
    /// 中心光晕强度（百分比，100 = 原设计亮度）。
    pub center_glow: i64,
    pub barrel: i64,
    pub aberration: i64,
    pub particle_speed: i64,
    pub particle_life: i64,
    pub burst: bool,
    pub burst_sensitivity: i64,
    pub burst_speed: i64,
    pub burst_life: i64,
    pub mode: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            size: 55,
            count: 96,
            height: 48,
            intensity: 100,
            bass: 100,
            mid: 100,
            treble: 100,
            link: true,
            rotate: true,
            rotate_speed: 100,
            colors: DEFAULT_COLORS.iter().map(|c| (*c).to_string()).collect(),
            bg_color: "#000000".to_string(),
            bg_opacity: 0,
            crt: true,
            afterglow: 60,
            center_glow: 60,
            barrel: 15,
            aberration: 30,
            particle_speed: 100,
            particle_life: 100,
            burst: true,
            burst_sensitivity: 55,
            burst_speed: 100,
            burst_life: 100,
            mode: "top".to_string(),
        }
    }
}

impl Appearance {
    /// 低 / 中 / 高音的增益倍数（配置里是百分比）。
    pub fn gains(&self) -> [f64; 3] {
        [
            self.bass as f64 / 100.0,
            self.mid as f64 / 100.0,
            self.treble as f64 / 100.0,
        ]
    }
}

/// 音频段（来源 / 启动延迟 / 设备）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Audio {
    pub source: String,
    pub delay_ms: i64,
    pub device: String,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            source: "system".to_string(),
            delay_ms: 600,
            device: "auto".to_string(),
        }
    }
}

/// 渲染段（性能相关）。
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Render {
    /// 解锁帧率上限（高刷屏到 120fps）。Rust 版里由渲染循环直接决定是否限到 120。
    pub high_fps: bool,
}

/// 整份配置。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Config {
    pub version: i64,
    pub appearance: Appearance,
    pub audio: Audio,
    pub render: Render,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: VERSION,
            appearance: Appearance::default(),
            audio: Audio::default(),
            render: Render::default(),
        }
    }
}

/// 便捷访问器（避免各处自己翻字段）。
pub fn appearance_of(cfg: &Config) -> &Appearance {
    &cfg.appearance
}

pub fn audio_of(cfg: &Config) -> &Audio {
    &cfg.audio
}

pub fn render_of(cfg: &Config) -> &Render {
    &cfg.render
}

// ==========================================================================
// 校验小工具
// ==========================================================================

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn as_dict<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>> {
    value.as_object().ok_or_else(|| {
        ConfigError(format!(
            "配置项 {path} 应当是一个对象，实际是 {}",
            value_kind(value)
        ))
    })
}

fn as_int(value: &Value, path: &str, range: (i64, i64)) -> Result<i64> {
    // serde_json 的 Bool 与 Number 是分开的 —— `true` 不会被当成 1。
    let n = value
        .as_i64()
        .ok_or_else(|| ConfigError(format!("配置项 {path} 应当是整数，实际是 {value}")))?;
    let (lo, hi) = range;
    if n < lo || n > hi {
        return fail(path, format!("应当在 {lo}~{hi} 之间，实际是 {n}"));
    }
    Ok(n)
}

fn as_choice(value: &Value, path: &str, options: &[&str]) -> Result<String> {
    if let Some(text) = value.as_str() {
        if options.contains(&text) {
            return Ok(text.to_string());
        }
    }
    fail(
        path,
        format!("只能是 {} 之一，实际是 {value}", options.join(" / ")),
    )
}

fn as_bool(value: &Value, path: &str) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| ConfigError(format!("配置项 {path} 应当是 true / false，实际是 {value}")))
}

fn is_hex_color(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

fn as_color(value: &Value, path: &str, example: &str) -> Result<String> {
    match value.as_str().map(str::trim) {
        Some(text) if is_hex_color(text) => Ok(text.to_ascii_uppercase()),
        _ => fail(
            path,
            format!("颜色要写成 #RRGGBB（如 {example}），实际是 {value}"),
        ),
    }
}

// ==========================================================================
// 校验（纯函数）
// ==========================================================================

fn validate_appearance(raw: Option<&Value>) -> Result<Appearance> {
    let mut out = Appearance::default();
    let Some(raw) = raw else { return Ok(out) };
    if raw.is_null() {
        return Ok(out);
    }
    let data = as_dict(raw, "appearance")?;

    if let Some(v) = data.get("size") {
        out.size = as_int(v, "appearance.size", SIZE_RANGE)?;
    }
    if let Some(v) = data.get("count") {
        out.count = as_int(v, "appearance.count", COUNT_RANGE)?;
    }
    if let Some(v) = data.get("height") {
        out.height = as_int(v, "appearance.height", HEIGHT_RANGE)?;
    }
    if let Some(v) = data.get("intensity") {
        out.intensity = as_int(v, "appearance.intensity", INTENSITY_RANGE)?;
    }
    for (key, slot) in [
        ("bass", &mut out.bass),
        ("mid", &mut out.mid),
        ("treble", &mut out.treble),
    ] {
        if let Some(v) = data.get(key) {
            *slot = as_int(v, &format!("appearance.{key}"), MIX_RANGE)?;
        }
    }
    if let Some(v) = data.get("link") {
        out.link = as_bool(v, "appearance.link")?;
    }
    if let Some(v) = data.get("rotate") {
        out.rotate = as_bool(v, "appearance.rotate")?;
    }
    if let Some(v) = data.get("rotate_speed") {
        out.rotate_speed = as_int(v, "appearance.rotate_speed", ROTATE_SPEED_RANGE)?;
    }
    if let Some(v) = data.get("colors") {
        out.colors = validate_colors(v, "appearance.colors")?;
    }
    if let Some(v) = data.get("bg_color") {
        out.bg_color = as_color(v, "appearance.bg_color", "#808080")?;
    }
    if let Some(v) = data.get("bg_opacity") {
        out.bg_opacity = as_int(v, "appearance.bg_opacity", BG_OPACITY_RANGE)?;
    }
    if let Some(v) = data.get("crt") {
        out.crt = as_bool(v, "appearance.crt")?;
    }
    if let Some(v) = data.get("afterglow") {
        out.afterglow = as_int(v, "appearance.afterglow", AFTERGLOW_RANGE)?;
    }
    if let Some(v) = data.get("center_glow") {
        out.center_glow = as_int(v, "appearance.center_glow", CENTER_GLOW_RANGE)?;
    }
    for (key, slot) in [
        ("barrel", &mut out.barrel),
        ("aberration", &mut out.aberration),
    ] {
        if let Some(v) = data.get(key) {
            *slot = as_int(v, &format!("appearance.{key}"), CRT_FX_RANGE)?;
        }
    }
    for (key, slot) in [
        ("particle_speed", &mut out.particle_speed),
        ("particle_life", &mut out.particle_life),
        ("burst_speed", &mut out.burst_speed),
        ("burst_life", &mut out.burst_life),
    ] {
        if let Some(v) = data.get(key) {
            *slot = as_int(v, &format!("appearance.{key}"), PARTICLE_RANGE)?;
        }
    }
    if let Some(v) = data.get("burst") {
        out.burst = as_bool(v, "appearance.burst")?;
    }
    if let Some(v) = data.get("burst_sensitivity") {
        out.burst_sensitivity = as_int(v, "appearance.burst_sensitivity", SENSITIVITY_RANGE)?;
    }
    if let Some(v) = data.get("mode") {
        out.mode = as_choice(v, "appearance.mode", &MODES)?;
    }
    Ok(out)
}

fn validate_colors(value: &Value, path: &str) -> Result<Vec<String>> {
    let Some(items) = value.as_array() else {
        return fail(path, "应当是一个颜色数组，例如 [\"#7EE7FF\", \"#FF6FD8\"]");
    };
    if items.is_empty() {
        return fail(path, "至少要有 1 个颜色");
    }
    if items.len() > COLORS_MAX {
        return fail(
            path,
            format!("最多 {COLORS_MAX} 个颜色（现在 {} 个）", items.len()),
        );
    }
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        out.push(as_color(item, &format!("{path}[{index}]"), "#7EE7FF")?);
    }
    Ok(out)
}

fn as_device(value: &Value, path: &str) -> Result<String> {
    let Some(text) = value.as_str() else {
        return fail(
            path,
            format!("应当是字符串（`auto` 或 `backend:设备名`），实际是 {value}"),
        );
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok("auto".to_string());
    }
    if trimmed.chars().count() > 200 {
        return fail(path, "太长了");
    }
    Ok(trimmed.to_string())
}

fn validate_audio(raw: Option<&Value>) -> Result<Audio> {
    let mut out = Audio::default();
    let Some(raw) = raw else { return Ok(out) };
    if raw.is_null() {
        return Ok(out);
    }
    let data = as_dict(raw, "audio")?;
    if let Some(v) = data.get("source") {
        out.source = as_choice(v, "audio.source", &AUDIO_SOURCES)?;
    }
    if let Some(v) = data.get("delay_ms") {
        out.delay_ms = as_int(v, "audio.delay_ms", DELAY_RANGE)?;
    }
    if let Some(v) = data.get("device") {
        out.device = as_device(v, "audio.device")?;
    }
    Ok(out)
}

fn validate_render(raw: Option<&Value>) -> Result<Render> {
    let mut out = Render::default();
    let Some(raw) = raw else { return Ok(out) };
    if raw.is_null() {
        return Ok(out);
    }
    let data = as_dict(raw, "render")?;
    if let Some(v) = data.get("high_fps") {
        out.high_fps = as_bool(v, "render.high_fps")?;
    }
    Ok(out)
}

/// 校验 + 归一化。返回**新对象**（不修改入参）；不合法抛 [`ConfigError`]。
///
/// 配置页的 `/api/config/validate` 与保存前的检查走的都是这一个入口，
/// 所以"试算"与"真存"的判定完全一致。
pub fn validate(data: &Value) -> Result<Config> {
    let root = as_dict(data, "配置")?;

    if let Some(version) = root.get("version") {
        let n = version
            .as_i64()
            .ok_or_else(|| ConfigError(format!("配置项 version 应当是整数，实际是 {version}")))?;
        if n > VERSION {
            return fail(
                "version",
                format!(
                    "配置版本 {n} 比本程序支持的 {VERSION} 新 —— 请升级程序，否则新字段会被丢弃"
                ),
            );
        }
    }

    Ok(Config {
        version: VERSION,
        appearance: validate_appearance(root.get("appearance"))?,
        audio: validate_audio(root.get("audio"))?,
        render: validate_render(root.get("render"))?,
    })
}

// ==========================================================================
// 存取
// ==========================================================================

/// `reload_if_changed()` 的结果 —— 让调用方能说清"发生了什么"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadResult {
    pub changed: bool,
    pub error: Option<String>,
}

/// 配置文件的读写句柄。
///
/// ```no_run
/// use vs_core::ConfigStore;
/// let mut store = ConfigStore::with_default_path();
/// store.load_or_default(true);          // 启动：没有就写默认值，坏了就用默认值并记错误
/// store.reload_if_changed();            // 每次检查前后调一次，拿热重载结果
/// ```
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    data: Config,
    last_error: Option<String>,
    /// 每次成功加载 / 保存都 +1（同进程里"自己写的"也能被上层感知）。
    revision: u64,
    /// 上一次见到的文件指纹（mtime 纳秒 + 大小），用于热重载判定。
    stamp: Option<(u128, u64)>,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            data: Config::default(),
            last_error: None,
            revision: 0,
            stamp: None,
        }
    }

    pub fn with_default_path() -> Self {
        Self::new(default_config_path())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn data(&self) -> &Config {
        &self.data
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    fn stamp_of_file(&self) -> Option<(u128, u64)> {
        let meta = fs::metadata(&self.path).ok()?;
        let modified = meta.modified().ok()?;
        let nanos = modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some((nanos, meta.len()))
    }

    /// 读文件 + 校验。**成功才替换内存里的数据**；失败保持原数据并返回错误。
    pub fn load(&mut self) -> Result<&Config> {
        let text = fs::read_to_string(&self.path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                ConfigError(format!(
                    "配置文件不存在：{}（可以先调 save_default() 生成一份）",
                    self.path.display()
                ))
            } else {
                ConfigError(format!("读不了配置文件 {}：{err}", self.path.display()))
            }
        })?;
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|err| ConfigError(format!("配置文件不是合法 JSON：{err}")))?;
        let clean = validate(&parsed)?;
        self.data = clean;
        self.last_error = None;
        self.stamp = self.stamp_of_file();
        self.revision += 1;
        Ok(&self.data)
    }

    /// 启动时的稳妥入口：文件不在就写一份默认的；文件坏了就用默认值并记下错误。
    pub fn load_or_default(&mut self, write_back: bool) -> &Config {
        if !self.exists() {
            if write_back {
                if let Err(err) = self.save_config(&Config::default()) {
                    self.last_error = Some(format!("写默认配置失败：{err}"));
                }
            } else {
                self.data = Config::default();
            }
            return &self.data;
        }
        if let Err(err) = self.load().map(|_| ()) {
            self.last_error = Some(err.to_string());
        }
        &self.data
    }

    /// 校验 → 归一化 → 原子写回（先写临时文件再 rename，断电不会写出半个文件）。
    pub fn save(&mut self, data: &Value) -> Result<&Config> {
        let clean = validate(data)?;
        self.write(&clean)
    }

    /// 已归一化的 [`Config`] 直接写回（调用方保证它合法）。
    pub fn save_config(&mut self, config: &Config) -> Result<&Config> {
        self.write(config)
    }

    pub fn save_default(&mut self) -> Result<&Config> {
        self.write(&Config::default())
    }

    fn write(&mut self, clean: &Config) -> Result<&Config> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                ConfigError(format!("写配置文件失败 {}：{err}", self.path.display()))
            })?;
        }
        let text = serde_json::to_string_pretty(clean)
            .map_err(|err| ConfigError(format!("序列化配置失败：{err}")))?;
        let mut tmp_name = self.path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp = self.path.with_file_name(tmp_name);
        fs::write(&tmp, format!("{text}\n"))
            .map_err(|err| ConfigError(format!("写配置文件失败 {}：{err}", self.path.display())))?;
        fs::rename(&tmp, &self.path)
            .map_err(|err| ConfigError(format!("写配置文件失败 {}：{err}", self.path.display())))?;

        self.data = clean.clone();
        self.last_error = None;
        self.stamp = self.stamp_of_file();
        self.revision += 1;
        Ok(&self.data)
    }

    /// 文件是否被外部改过 —— 只看指纹，不读内容。
    pub fn changed_on_disk(&self) -> bool {
        match self.stamp_of_file() {
            None => self.stamp.is_some(),
            Some(current) => Some(current) != self.stamp,
        }
    }

    /// 被改过就重载。**重载失败不动现有数据**，只把错误带回给调用方。
    pub fn reload_if_changed(&mut self) -> ReloadResult {
        if !self.changed_on_disk() {
            return ReloadResult {
                changed: false,
                error: None,
            };
        }
        if !self.exists() {
            self.stamp = None;
            self.last_error = Some(format!("配置文件不见了：{}", self.path.display()));
            return ReloadResult {
                changed: false,
                error: self.last_error.clone(),
            };
        }
        match self.load().map(|_| ()) {
            Ok(()) => ReloadResult {
                changed: true,
                error: None,
            },
            Err(err) => {
                self.last_error = Some(err.to_string());
                self.stamp = self.stamp_of_file(); // 记下指纹，免得每帧都重试同一个坏文件
                ReloadResult {
                    changed: false,
                    error: self.last_error.clone(),
                }
            }
        }
    }

    /// 当前数据的稳定指纹（给 HTTP ETag 用：内容没变就不必回传整份配置）。
    ///
    /// 用 FNV-1a 而不是 sha1 —— 这里只需要"内容变没变"的稳定标识，不需要密码学强度。
    pub fn fingerprint(&self) -> String {
        let text = serde_json::to_string(&self.data).unwrap_or_default();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
}

/// 默认配置文件路径。
///
/// * `VISUALSPECTRUM_HOME` 覆盖（测试 / 便携模式）；
/// * 否则 `$XDG_CONFIG_HOME/visualspectrum/config.json`；
/// * 再否则 `$HOME/.config/visualspectrum/config.json`。
pub fn default_config_path() -> PathBuf {
    if let Ok(home) = std::env::var("VISUALSPECTRUM_HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join("config.json");
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg)
                .join("visualspectrum")
                .join("config.json");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("visualspectrum")
        .join("config.json")
}
