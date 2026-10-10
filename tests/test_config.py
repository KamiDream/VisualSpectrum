"""配置层：校验 / 归一化 / 原子写 / 热重载。"""

from __future__ import annotations

import json

import pytest

from app.config import (
    AFTERGLOW_RANGE,
    AUDIO_SOURCES,
    BG_OPACITY_RANGE,
    COUNT_RANGE,
    CRT_FX_RANGE,
    DELAY_RANGE,
    HEIGHT_RANGE,
    INTENSITY_RANGE,
    MIX_RANGE,
    MODES,
    PARTICLE_RANGE,
    SENSITIVITY_RANGE,
    SIZE_RANGE,
    ConfigError,
    ConfigStore,
    appearance_of,
    audio_of,
    default_config,
    render_of,
    validate,
)


def test_default_is_valid() -> None:
    assert validate(default_config()) == default_config()


def test_appearance_of_default() -> None:
    appearance = appearance_of({})
    assert appearance["mode"] in MODES
    assert SIZE_RANGE[0] <= appearance["size"] <= SIZE_RANGE[1]


def test_missing_fields_are_filled() -> None:
    clean = validate({"version": 1, "appearance": {}})
    assert clean["appearance"] == default_config()["appearance"]


def test_unknown_fields_are_dropped() -> None:
    clean = validate({"version": 1, "appearance": {"size": 40, "bogus": 1}, "extra": True})
    assert "extra" not in clean
    assert "bogus" not in clean["appearance"]
    assert clean["appearance"]["size"] == 40


@pytest.mark.parametrize("size", [SIZE_RANGE[0] - 1, SIZE_RANGE[1] + 1, 0])
def test_size_out_of_range(size: int) -> None:
    with pytest.raises(ConfigError):
        validate({"appearance": {"size": size}})


@pytest.mark.parametrize("count", [COUNT_RANGE[0] - 1, COUNT_RANGE[1] + 1])
def test_count_out_of_range(count: int) -> None:
    with pytest.raises(ConfigError):
        validate({"appearance": {"count": count}})


def test_bool_is_not_int() -> None:
    with pytest.raises(ConfigError):
        validate({"appearance": {"size": True}})


def test_default_has_height_and_rotate() -> None:
    appearance = default_config()["appearance"]
    assert HEIGHT_RANGE[0] <= appearance["height"] <= HEIGHT_RANGE[1]
    assert appearance["rotate"] is True


@pytest.mark.parametrize("height", [HEIGHT_RANGE[0] - 1, HEIGHT_RANGE[1] + 1])
def test_height_out_of_range(height: int) -> None:
    with pytest.raises(ConfigError):
        validate({"appearance": {"height": height}})


def test_rotate_must_be_bool() -> None:
    with pytest.raises(ConfigError):
        validate({"appearance": {"rotate": "yes"}})
    assert validate({"appearance": {"rotate": False}})["appearance"]["rotate"] is False


def test_default_has_intensity_and_link() -> None:
    appearance = default_config()["appearance"]
    assert INTENSITY_RANGE[0] <= appearance["intensity"] <= INTENSITY_RANGE[1]
    assert appearance["link"] is True


@pytest.mark.parametrize("intensity", [INTENSITY_RANGE[0] - 1, INTENSITY_RANGE[1] + 1])
def test_intensity_out_of_range(intensity: int) -> None:
    with pytest.raises(ConfigError):
        validate({"appearance": {"intensity": intensity}})


def test_link_must_be_bool() -> None:
    with pytest.raises(ConfigError):
        validate({"appearance": {"link": "yes"}})
    assert validate({"appearance": {"link": False}})["appearance"]["link"] is False


def test_default_audio() -> None:
    audio = default_config()["audio"]
    assert audio["source"] in AUDIO_SOURCES
    assert DELAY_RANGE[0] <= audio["delay_ms"] <= DELAY_RANGE[1]
    assert audio_of({})["source"] in AUDIO_SOURCES


def test_bad_audio_source() -> None:
    with pytest.raises(ConfigError):
        validate({"audio": {"source": "mic"}})


def test_audio_delay_range() -> None:
    with pytest.raises(ConfigError):
        validate({"audio": {"delay_ms": DELAY_RANGE[1] + 1}})
    assert validate({"audio": {"delay_ms": 0}})["audio"]["delay_ms"] == 0


def test_default_audio_device() -> None:
    assert default_config()["audio"]["device"] == "auto"


def test_audio_device_normalized() -> None:
    assert validate({"audio": {"device": "   "}})["audio"]["device"] == "auto"
    key = "soundcard:Speakers (Realtek)"
    assert validate({"audio": {"device": key}})["audio"]["device"] == key


def test_audio_device_must_be_string() -> None:
    with pytest.raises(ConfigError):
        validate({"audio": {"device": 123}})


def test_colors_default() -> None:
    colors = default_config()["appearance"]["colors"]
    assert colors and all(c.startswith("#") and len(c) == 7 for c in colors)


def test_colors_normalized_and_validated() -> None:
    clean = validate({"appearance": {"colors": ["#7ee7ff", "#ff6fd8"]}})
    assert clean["appearance"]["colors"] == ["#7EE7FF", "#FF6FD8"]
    with pytest.raises(ConfigError):
        validate({"appearance": {"colors": []}})
    with pytest.raises(ConfigError):
        validate({"appearance": {"colors": ["red"]}})
    with pytest.raises(ConfigError):
        validate({"appearance": {"colors": ["#fff"]}})
    with pytest.raises(ConfigError):
        validate({"appearance": {"colors": ["#7EE7FF"] * 9}})


def test_mix_defaults_and_range() -> None:
    appearance = default_config()["appearance"]
    for key in ("bass", "mid", "treble"):
        assert MIX_RANGE[0] <= appearance[key] <= MIX_RANGE[1]
    for key in ("bass", "mid", "treble"):
        with pytest.raises(ConfigError):
            validate({"appearance": {key: MIX_RANGE[1] + 1}})
    assert validate({"appearance": {"bass": 0}})["appearance"]["bass"] == 0


def test_background_defaults_and_validation() -> None:
    appearance = default_config()["appearance"]
    assert appearance["bg_opacity"] == 0
    assert appearance["bg_color"] == "#000000"            # 默认黑
    assert appearance["crt"] is True
    assert BG_OPACITY_RANGE[0] <= appearance["bg_opacity"] <= BG_OPACITY_RANGE[1]
    clean = validate({"appearance": {"bg_color": "#a1b2c3", "bg_opacity": 40, "crt": False}})
    assert clean["appearance"]["bg_color"] == "#A1B2C3"     # 归一化成大写
    assert clean["appearance"]["bg_opacity"] == 40
    assert clean["appearance"]["crt"] is False
    with pytest.raises(ConfigError):
        validate({"appearance": {"bg_color": "grey"}})
    with pytest.raises(ConfigError):
        validate({"appearance": {"bg_opacity": BG_OPACITY_RANGE[1] + 1}})
    with pytest.raises(ConfigError):
        validate({"appearance": {"crt": "yes"}})


def test_render_defaults_and_validation() -> None:
    assert default_config()["render"]["high_fps"] is False
    assert render_of({})["high_fps"] is False
    assert validate({"render": {"high_fps": True}})["render"]["high_fps"] is True
    with pytest.raises(ConfigError):
        validate({"render": {"high_fps": "yes"}})


def test_afterglow_default_and_range() -> None:
    appearance = default_config()["appearance"]
    assert AFTERGLOW_RANGE[0] <= appearance["afterglow"] <= AFTERGLOW_RANGE[1]
    with pytest.raises(ConfigError):
        validate({"appearance": {"afterglow": AFTERGLOW_RANGE[1] + 1}})
    assert validate({"appearance": {"afterglow": 0}})["appearance"]["afterglow"] == 0


def test_crt_fx_defaults_and_validation() -> None:
    appearance = default_config()["appearance"]
    for key in ("barrel", "aberration"):
        assert CRT_FX_RANGE[0] <= appearance[key] <= CRT_FX_RANGE[1]
    for key in ("barrel", "aberration"):
        with pytest.raises(ConfigError):
            validate({"appearance": {key: CRT_FX_RANGE[1] + 1}})
    # 0 = 关，是合法值
    assert validate({"appearance": {"barrel": 0}}) ["appearance"]["barrel"] == 0


def test_particles_defaults_and_validation() -> None:
    appearance = default_config()["appearance"]
    for key in ("particle_speed", "particle_life", "burst_speed", "burst_life"):
        assert PARTICLE_RANGE[0] <= appearance[key] <= PARTICLE_RANGE[1]
    assert appearance["burst"] is True
    assert SENSITIVITY_RANGE[0] <= appearance["burst_sensitivity"] <= SENSITIVITY_RANGE[1]
    with pytest.raises(ConfigError):
        validate({"appearance": {"particle_speed": PARTICLE_RANGE[1] + 1}})
    with pytest.raises(ConfigError):
        validate({"appearance": {"burst": "yes"}})
    with pytest.raises(ConfigError):
        validate({"appearance": {"burst_sensitivity": SENSITIVITY_RANGE[1] + 1}})


def test_bad_mode() -> None:
    with pytest.raises(ConfigError):
        validate({"appearance": {"mode": "floating"}})


def test_newer_version_is_rejected() -> None:
    with pytest.raises(ConfigError):
        validate({"version": 999})


def test_store_roundtrip(tmp_path) -> None:
    store = ConfigStore(path=tmp_path / "config.json")
    store.load_or_default()
    assert (tmp_path / "config.json").is_file()

    store.save({"appearance": {"size": 30, "count": 32, "mode": "wallpaper"}})
    on_disk = json.loads((tmp_path / "config.json").read_text(encoding="utf-8"))
    assert on_disk["appearance"]["mode"] == "wallpaper"
    assert on_disk["version"] == 1


def test_store_bad_file_keeps_previous(tmp_path) -> None:
    path = tmp_path / "config.json"
    store = ConfigStore(path=path)
    store.load_or_default()
    good = dict(store.data)

    path.write_text("{ not json", encoding="utf-8")
    store.reload_if_changed()
    assert store.last_error is not None
    assert store.data == good          # 坏文件不动内存里的好数据


def test_save_sets_revision_but_not_changed(tmp_path) -> None:
    store = ConfigStore(path=tmp_path / "config.json")
    store.load_or_default()
    before = store.revision
    store.save({"appearance": {"size": 20}})
    assert store.revision == before + 1
    assert store.changed_on_disk() is False   # 自己写的不算"外部改动"


def test_external_edit_is_detected(tmp_path) -> None:
    path = tmp_path / "config.json"
    store = ConfigStore(path=path)
    store.load_or_default()
    path.write_text(json.dumps({"version": 1, "appearance": {"size": 77}}), encoding="utf-8")
    result = store.reload_if_changed()
    assert result.changed is True
    assert store.data["appearance"]["size"] == 77
