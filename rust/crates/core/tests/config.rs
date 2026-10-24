//! 配置层的单元测试。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};
use vs_core::{validate, Config, ConfigStore};

fn default_json() -> Value {
    serde_json::to_value(Config::default()).expect("默认配置应当能序列化")
}

fn temp_path(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("vs-core-{}-{nanos}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{tag}.json"))
}

#[test]
fn default_roundtrips() {
    assert_eq!(validate(&default_json()).unwrap(), Config::default());
}

#[test]
fn missing_fields_are_filled() {
    let clean = validate(&json!({"version": 1, "appearance": {}})).unwrap();
    assert_eq!(clean.appearance, Config::default().appearance);
    assert_eq!(clean.audio, Config::default().audio);
}

#[test]
fn unknown_fields_are_dropped() {
    let clean = validate(&json!({
        "appearance": {"size": 40, "bogus": 1},
        "extra": true
    }))
    .unwrap();
    assert_eq!(clean.appearance.size, 40);
    let text = serde_json::to_string(&clean).unwrap();
    assert!(!text.contains("bogus"));
    assert!(!text.contains("extra"));
}

#[test]
fn size_out_of_range_rejected() {
    assert!(validate(&json!({"appearance": {"size": 4}})).is_err());
    assert!(validate(&json!({"appearance": {"size": 201}})).is_err());
}

#[test]
fn bool_is_not_int() {
    assert!(validate(&json!({"appearance": {"size": true}})).is_err());
}

#[test]
fn colors_are_normalized_and_validated() {
    let clean = validate(&json!({"appearance": {"colors": ["#7ee7ff", "#ff6fd8"]}})).unwrap();
    assert_eq!(clean.appearance.colors, vec!["#7EE7FF", "#FF6FD8"]);

    assert!(validate(&json!({"appearance": {"colors": []}})).is_err());
    assert!(validate(&json!({"appearance": {"colors": ["red"]}})).is_err());
    assert!(validate(&json!({"appearance": {"colors": ["#fff"]}})).is_err());
    let nine = vec!["#7EE7FF"; 9];
    assert!(validate(&json!({"appearance": {"colors": nine}})).is_err());
}

#[test]
fn mode_and_source_choices() {
    assert!(validate(&json!({"appearance": {"mode": "floating"}})).is_err());
    assert!(validate(&json!({"audio": {"source": "mic"}})).is_err());

    let clean = validate(&json!({
        "appearance": {"mode": "wallpaper"},
        "audio": {"source": "off"}
    }))
    .unwrap();
    assert_eq!(clean.appearance.mode, "wallpaper");
    assert_eq!(clean.audio.source, "off");
}

#[test]
fn device_is_trimmed_and_defaults_to_auto() {
    assert_eq!(
        validate(&json!({"audio": {"device": "   "}}))
            .unwrap()
            .audio
            .device,
        "auto"
    );
    let key = "soundcard:Speakers (Realtek)";
    assert_eq!(
        validate(&json!({"audio": {"device": key}}))
            .unwrap()
            .audio
            .device,
        key
    );
    assert!(validate(&json!({"audio": {"device": 123}})).is_err());
}

#[test]
fn newer_version_rejected() {
    assert!(validate(&json!({"version": 999})).is_err());
}

#[test]
fn store_roundtrip_and_revision() {
    let path = temp_path("roundtrip");
    let mut store = ConfigStore::new(&path);
    store.load_or_default(true);
    assert!(path.is_file(), "首次启动应当写出默认配置");

    let before = store.revision();
    store
        .save(&json!({"appearance": {"count": 32, "mode": "wallpaper"}}))
        .unwrap();
    assert_eq!(store.revision(), before + 1);
    assert_eq!(store.data().appearance.mode, "wallpaper");

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"wallpaper\""));
    assert!(!store.changed_on_disk(), "自己写的不算外部改动");
}

#[test]
fn bad_file_keeps_previous_data() {
    let path = temp_path("bad");
    let mut store = ConfigStore::new(&path);
    store.load_or_default(true);
    let good = store.data().clone();

    std::fs::write(&path, "{ not json").unwrap();
    let result = store.reload_if_changed();
    assert!(!result.changed);
    assert!(result.error.is_some());
    assert_eq!(store.data(), &good, "坏文件不该覆盖内存里的好数据");
}

#[test]
fn fingerprint_changes_with_content() {
    let path = temp_path("fingerprint");
    let mut store = ConfigStore::new(&path);
    store.load_or_default(true);
    let before = store.fingerprint();
    store.save(&json!({"appearance": {"size": 42}})).unwrap();
    assert_ne!(before, store.fingerprint());
}
