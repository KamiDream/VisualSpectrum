"""VisualSpectrum —— 配置层：schema / 默认值 / 严格校验 / 读写 / 热重载。

设计取向（几条硬性约定，后面所有代码都依赖它们）：

1. **一个 JSON 文件就是全部真相**：配置页保存 = 覆盖这个文件，运行中的浮层靠 mtime
   发现变化并热重载 —— 不需要重启程序。
2. **校验一定发生在写入之前**，而且 `validate()` 是**纯函数**（不碰磁盘）：
   于是配置页可以先调 `/api/config/validate` 试算一遍，拿到同一套中文报错。
3. **校验失败绝不破坏已有配置**：`ConfigStore.load()` 出错时保持上一次的好数据不变，
   把错误记在 `last_error` 里 —— 用户手改坏了文件，程序仍然能用。
4. **归一化**，不只是"检查"：缺省字段补齐、未知字段丢弃、数值夹到合法区间。
   写回文件的就是归一化后的结果。

配置的**形状**::

    {
      "version": 1,
      "appearance": {
        "size": 55,       # 环占屏幕短边的百分比（10~100）
        "count": 96,      # 环上频谱块的个数（16~256）
        "height": 48,     # 条高：频谱条最长为环半径的百分之多少（5~500，>100 冲出环外）
        "intensity": 100, # 强度 / 灵敏度（10~1000）
        "bass": 100, "mid": 100, "treble": 100,  # 低/中/高音占比（0~400）
        "link": true,     # 把各条顶端的中心点用细线依次相连（开/关）
        "rotate": true,   # 整个环是否缓慢自转
        "colors": ["#7EE7FF", "#B37BFF", "#FF6FD8"],  # 按频率低→高的色标（1~8 个）
        "bg_color": "#000000",  # 背景颜色（当 bg_opacity > 0 时才看得见）
        "bg_opacity": 0,       # 背景不透明度（0~100，0 = 全透明）
        "crt": true,           # CRT 扫描线 / 暗角效果（只在 bg_opacity > 0 时可开）
        "afterglow": 60,       # 余晖（荧光拖尾）强度（0~100，0 = 不留余晖）
        "barrel": 15,          # 桶形畸变强度（WebGL）（0~100，0 = 关）
        "aberration": 30,      # RGB 分色 / 色散强度（0~100，0 = 关）
        "particle_speed": 100, # 散射粒子速度倍数（20~400）
        "particle_life": 100,  # 散射粒子寿命倍数（20~400）
        "burst": true,         # 鼓点全屏扩散粒子（开关）
        "burst_sensitivity": 55,  # 鼓点灵敏度（0~100）
        "burst_speed": 100,    # 鼓点粒子速度倍数
        "burst_life": 100,     # 鼓点粒子寿命倍数
        "mode": "top"     # top（最上层、鼠标穿透） | wallpaper（贴桌面底层）
      },
      "audio": {
        "source": "system",  # system（抓系统声音） | off（只放演示波形）
        "delay_ms": 600,     # 启动后等多久再打开音频设备（0~10000）
        "device": "auto"     # auto（默认输出）或 "backend:设备名"（见 /api/audio/devices）
      },
      "render": {
        "high_fps": false    # 解锁 Chromium 帧率上限（高刷屏要 120fps 就开）；⚠ 重启生效
      }
    }
"""

from __future__ import annotations

import hashlib
import json
import os
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, NoReturn

from . import paths

__all__ = [
    "VERSION", "MODES", "AUDIO_SOURCES",
    "SIZE_RANGE", "COUNT_RANGE", "HEIGHT_RANGE", "INTENSITY_RANGE", "DELAY_RANGE",
    "MIX_RANGE", "BG_OPACITY_RANGE", "AFTERGLOW_RANGE", "PARTICLE_RANGE",
    "SENSITIVITY_RANGE", "CRT_FX_RANGE", "COLORS_MAX", "DEFAULT_COLORS",
    "ConfigError", "ConfigStore", "ReloadResult",
    "default_config", "validate", "appearance_of", "audio_of", "render_of",
]

#: 配置结构版本（结构不兼容变化时 +1；比程序新就直接拒绝，免得把新字段静默丢掉）
VERSION = 1

#: 两种模式：**顶层**（浮在最上层、鼠标穿透）与**壁纸**（贴到桌面壁纸层）。
#: 名字就是配置里存的值，两端（前端 / 后端 / 渲染）共用。
MODES = ("top", "wallpaper")

#: 大小的取值范围（百分比）—— 是"环的外径占**屏幕短边**的多少"。
#: 上限放到 200：允许环超出屏幕（大屏上想要"满到溢出来"的效果时能调）。
SIZE_RANGE = (5, 200)

#: 数量的取值范围（环上频谱块的个数）。
#: 下限 4（再少也还能看），上限 512（想"每一根都细如发丝"也能调，但越细越费性能）。
COUNT_RANGE = (4, 512)

#: 条高的取值范围（百分比）—— 频谱条**最长**是环半径的百分之多少。
#: ≤100 时条尖止于环边（经典圆环观感）；**>100 就是"不封顶"**：条会冲出环外
#: （放到 500 = 最多能伸到 5 倍半径，已经远超屏幕了）。
HEIGHT_RANGE = (5, 500)

#: 强度 / 灵敏度的取值范围（百分比）。
#: 100 = 原始电平；调大 = 更灵敏（同样的声音条更高），调小 = 更克制。
INTENSITY_RANGE = (10, 1000)

#: 低 / 中 / 高音的**占比**（百分比，各区域自己的增益）。
#: 100 = 不增不减；调大这一块就更突出。它们是在"各自归一化"之后再乘上去的，
#: 所以把低音调小也不会让它变成没动态的满格。
MIX_RANGE = (0, 400)

#: 背景不透明度的取值范围（百分比）。0 = 全透明（桌面直接透出来）。
BG_OPACITY_RANGE = (0, 100)

#: 余晖（荧光拖尾）的强度范围（0~100）。
#: 0 = 不留余晖（每帧全清）；越大拖尾越长。
AFTERGLOW_RANGE = (0, 100)

#: 粒子的**速度 / 寿命**倍数范围（百分比）。100 = 原样；调大 = 更快 / 活得更久。
PARTICLE_RANGE = (20, 400)

#: 鼓点灵敏度（0~100）。越大越容易触发。
SENSITIVITY_RANGE = (0, 100)

#: CRT 后处理效果（桶形畸变 / RGB 分色）的强度范围（0~100）。0 = 关。
CRT_FX_RANGE = (0, 100)

#: 颜色：一段**按频率从低到高**排列的色标（至少 1 个，最多 8 个），
#: 渲染时在色标之间插值 —— 于是"每个频率都有它自己的颜色"。
#: 1 个 = 单色；给多个就能铺出渐变。
COLORS_MAX = 8
DEFAULT_COLORS = ("#7EE7FF", "#B37BFF", "#FF6FD8")   # 青 → 紫 → 品红

_HEX_COLOR = re.compile(r"^#[0-9a-fA-F]{6}$")

#: 音频来源：
#: * `system` —— 抓系统正在播放的声音（回环捕获）；
#: * `off`    —— 不碰任何音频设备，只放合成的演示波形。
#:   ⚠ 蓝牙耳机上「抓回环」可能让耳机切到通话模式、音乐中断 —— 这时候把来源设为 `off`
#:   （或者把启动延迟调大）就能绕开。
AUDIO_SOURCES = ("system", "off")

#: 启动延迟（毫秒）：启动后等这么久再打开音频设备。
#: 目的是错开"启动那一瞬间"对音频设备的抢占（系统 / 浏览器 / 我们自己都会碰设备）。
DELAY_RANGE = (0, 10000)


class ConfigError(ValueError):
    """配置不合法。消息是给用户看的中文，会原样出现在配置页的提示里。"""


# ==========================================================================
# 默认值
# ==========================================================================

def default_config() -> dict[str, Any]:
    """出厂默认配置（全新的字典，调用方可以随便改）。"""
    return {
        "version": VERSION,
        "appearance": {
            "size": 55,
            "count": 96,
            "height": 48,
            "intensity": 100,
            "bass": 100,
            "mid": 100,
            "treble": 100,
            "link": True,
            "rotate": True,
            "colors": list(DEFAULT_COLORS),
            "bg_color": "#000000",
            "bg_opacity": 0,
            "crt": True,
            "afterglow": 60,
            "barrel": 15,
            "aberration": 30,
            "particle_speed": 100,
            "particle_life": 100,
            "burst": True,
            "burst_sensitivity": 55,
            "burst_speed": 100,
            "burst_life": 100,
            "mode": "top",
        },
        "audio": {
            "source": "system",
            "delay_ms": 600,
            "device": "auto",
        },
        "render": {
            # 解锁 Chromium 的帧率上限（高刷屏上要 120fps 就得开）。⚠ 要**重启**才生效。
            "high_fps": False,
        },
    }


# ==========================================================================
# 校验小工具
# ==========================================================================

def _fail(path: str, message: str) -> NoReturn:
    raise ConfigError(f"配置项 {path} {message}")


def _as_dict(value: Any, path: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        _fail(path, f"应当是一个对象，实际是 {type(value).__name__}")
    return value


def _as_int(value: Any, path: str, lo: int, hi: int) -> int:
    # bool 是 int 的子类，得单独挡掉（true 被当成 1 会很怪）
    if isinstance(value, bool) or not isinstance(value, int):
        _fail(path, f"应当是整数，实际是 {value!r}")
    if not (lo <= value <= hi):
        _fail(path, f"应当在 {lo}~{hi} 之间，实际是 {value}")
    return value


def _as_choice(value: Any, path: str, options: tuple[str, ...]) -> str:
    if not isinstance(value, str) or value not in options:
        _fail(path, f"只能是 {' / '.join(options)} 之一，实际是 {value!r}")
    return value


def _as_bool(value: Any, path: str) -> bool:
    if not isinstance(value, bool):
        _fail(path, f"应当是 true / false，实际是 {value!r}")
    return value


# ==========================================================================
# 校验
# ==========================================================================

def _validate_appearance(raw: Any) -> dict[str, Any]:
    base = default_config()["appearance"]
    if raw is None:
        return dict(base)
    data = _as_dict(raw, "appearance")
    out = dict(base)

    if "size" in data:
        out["size"] = _as_int(data["size"], "appearance.size", *SIZE_RANGE)
    if "count" in data:
        out["count"] = _as_int(data["count"], "appearance.count", *COUNT_RANGE)
    if "height" in data:
        out["height"] = _as_int(data["height"], "appearance.height", *HEIGHT_RANGE)
    if "intensity" in data:
        out["intensity"] = _as_int(data["intensity"], "appearance.intensity", *INTENSITY_RANGE)
    for key in ("bass", "mid", "treble"):
        if key in data:
            out[key] = _as_int(data[key], f"appearance.{key}", *MIX_RANGE)
    if "link" in data:
        out["link"] = _as_bool(data["link"], "appearance.link")
    if "rotate" in data:
        out["rotate"] = _as_bool(data["rotate"], "appearance.rotate")
    if "colors" in data:
        out["colors"] = _validate_colors(data["colors"], "appearance.colors")
    if "bg_color" in data:
        color = data["bg_color"]
        if not isinstance(color, str) or not _HEX_COLOR.match(color.strip()):
            _fail("appearance.bg_color", f"颜色要写成 #RRGGBB（如 #808080），实际是 {color!r}")
        out["bg_color"] = color.strip().upper()
    if "bg_opacity" in data:
        out["bg_opacity"] = _as_int(data["bg_opacity"], "appearance.bg_opacity",
                                    *BG_OPACITY_RANGE)
    if "crt" in data:
        out["crt"] = _as_bool(data["crt"], "appearance.crt")
    if "afterglow" in data:
        out["afterglow"] = _as_int(data["afterglow"], "appearance.afterglow",
                                   *AFTERGLOW_RANGE)
    for key in ("barrel", "aberration"):
        if key in data:
            out[key] = _as_int(data[key], f"appearance.{key}", *CRT_FX_RANGE)
    for key in ("particle_speed", "particle_life", "burst_speed", "burst_life"):
        if key in data:
            out[key] = _as_int(data[key], f"appearance.{key}", *PARTICLE_RANGE)
    if "burst" in data:
        out["burst"] = _as_bool(data["burst"], "appearance.burst")
    if "burst_sensitivity" in data:
        out["burst_sensitivity"] = _as_int(data["burst_sensitivity"],
                                           "appearance.burst_sensitivity", *SENSITIVITY_RANGE)
    if "mode" in data:
        out["mode"] = _as_choice(data["mode"], "appearance.mode", MODES)
    return out


def _validate_colors(raw: Any, path: str) -> list[str]:
    """一段颜色色标（按频率从低到高）。至少 1 个，最多 `COLORS_MAX` 个。"""
    if not isinstance(raw, list):
        _fail(path, "应当是一个颜色数组，例如 [\"#7EE7FF\", \"#FF6FD8\"]")
    if not raw:
        _fail(path, "至少要有 1 个颜色")
    if len(raw) > COLORS_MAX:
        _fail(path, f"最多 {COLORS_MAX} 个颜色（现在 {len(raw)} 个）")
    out: list[str] = []
    for i, value in enumerate(raw):
        if not isinstance(value, str) or not _HEX_COLOR.match(value.strip()):
            _fail(f"{path}[{i}]", f"颜色要写成 #RRGGBB（如 #7EE7FF），实际是 {value!r}")
        out.append(value.strip().upper())
    return out


def _validate_audio(raw: Any) -> dict[str, Any]:
    base = default_config()["audio"]
    if raw is None:
        return dict(base)
    data = _as_dict(raw, "audio")
    out = dict(base)

    if "source" in data:
        out["source"] = _as_choice(data["source"], "audio.source", AUDIO_SOURCES)
    if "delay_ms" in data:
        out["delay_ms"] = _as_int(data["delay_ms"], "audio.delay_ms", *DELAY_RANGE)
    if "device" in data:
        out["device"] = _as_device(data["device"], "audio.device")
    return out


def _as_device(value: Any, path: str) -> str:
    """捕获设备的标识：`auto` 或 `<backend>:<设备名>`。

    ⚠ 刻意**不校验设备是否存在**：设备可能临时拔了/改名了，但配置不该因此存不下 ——
      运行时打不开会自动退回 `auto`（见 `audio._open_source`）。
    """
    if not isinstance(value, str):
        _fail(path, f"应当是字符串（`auto` 或 `backend:设备名`），实际是 {value!r}")
    text = value.strip()
    if not text:
        return "auto"
    if len(text) > 200:
        _fail(path, "太长了")
    return text


def _validate_render(raw: Any) -> dict[str, Any]:
    base = default_config()["render"]
    if raw is None:
        return dict(base)
    data = _as_dict(raw, "render")
    out = dict(base)
    if "high_fps" in data:
        out["high_fps"] = _as_bool(data["high_fps"], "render.high_fps")
    return out


def validate(data: Any) -> dict[str, Any]:
    """校验 + 归一化。返回**新字典**（不修改入参）；不合法抛 `ConfigError`。

    配置页的 `/api/config/validate` 与保存前的检查走的都是这一个入口，
    所以"试算"与"真存"的判定完全一致。
    """
    root = _as_dict(data, "配置")

    version = root.get("version", VERSION)
    if isinstance(version, bool) or not isinstance(version, int):
        _fail("version", f"应当是整数，实际是 {version!r}")
    if version > VERSION:
        _fail("version", f"配置版本 {version} 比本程序支持的 {VERSION} 新 —— 请升级程序，"
                         f"否则新字段会被丢弃")

    return {
        "version": VERSION,
        "appearance": _validate_appearance(root.get("appearance")),
        "audio": _validate_audio(root.get("audio")),
        "render": _validate_render(root.get("render")),
    }


# ==========================================================================
# 存取
# ==========================================================================

@dataclass
class ReloadResult:
    """`reload_if_changed()` 的结果 —— 让调用方能说清"发生了什么"。"""

    changed: bool = False
    error: str | None = None
    data: dict[str, Any] | None = None


@dataclass
class ConfigStore:
    """配置文件的读写句柄。

    典型用法（桌面端）::

        store = ConfigStore()
        store.load_or_default()        # 启动：没有就写默认值，坏了就用默认值并记错误
        ...
        store.reload_if_changed()      # 每次检查前后调一次，拿热重载后的新配置

    典型用法（Flask 侧）::

        store.load()                   # 取当前配置（含归一化结果）
        store.save(new_data)           # 严格校验通过后原子写回
    """

    path: Path = field(default_factory=paths.config_path)
    data: dict[str, Any] = field(default_factory=default_config)
    last_error: str | None = None
    #: 每次成功加载 / 保存都 +1。
    #: **为什么还需要它**：配置页（Flask）与浮层跑在同一个进程里，保存是**我们自己**写的文件
    #: —— `save()` 顺手更新了指纹，于是 mtime 判定会说"没被外部改过"，浮层就永远不知道
    #: "该重新应用了"。这个计数器把这个盲区补上（上层比较它即可）。
    revision: int = 0
    #: 上一次见到的文件指纹（mtime 纳秒 + 大小），用于热重载判定
    _stamp: tuple[int, int] | None = field(default=None, repr=False)

    # ---------------------------------------------------------------- 读
    def _stamp_of_file(self) -> tuple[int, int] | None:
        try:
            st = self.path.stat()
        except OSError:
            return None
        return (st.st_mtime_ns, st.st_size)

    def exists(self) -> bool:
        return self.path.is_file()

    def load(self) -> dict[str, Any]:
        """读文件 + 校验。**成功才替换内存里的数据**；失败保持原数据并抛 `ConfigError`。"""
        try:
            raw_text = self.path.read_text(encoding="utf-8")
        except FileNotFoundError:
            raise ConfigError(f"配置文件不存在：{self.path}（可以先调 save_default() 生成一份）")
        except OSError as exc:
            raise ConfigError(f"读不了配置文件 {self.path}：{exc}") from exc

        try:
            parsed = json.loads(raw_text)
        except json.JSONDecodeError as exc:
            raise ConfigError(
                f"配置文件不是合法 JSON（第 {exc.lineno} 行第 {exc.colno} 列）：{exc.msg}") from exc

        clean = validate(parsed)
        self.data = clean
        self.last_error = None
        self._stamp = self._stamp_of_file()
        self.revision += 1
        return clean

    def load_or_default(self, *, write_back: bool = True) -> dict[str, Any]:
        """启动时的稳妥入口：文件不在就写一份默认的；文件坏了就用默认值并记下错误。"""
        if not self.exists():
            if write_back:
                try:
                    self.save(default_config())
                except (ConfigError, OSError) as exc:
                    self.last_error = f"写默认配置失败：{exc}"
            else:
                self.data = default_config()
            return self.data

        try:
            return self.load()
        except ConfigError as exc:
            self.last_error = str(exc)
            return self.data                          # 保持默认值，程序照常能用

    # ---------------------------------------------------------------- 写
    def save(self, data: Any) -> dict[str, Any]:
        """校验 → 归一化 → **原子写入**（先写临时文件再 rename，断电不会写出半个文件）。"""
        clean = validate(data)
        target = Path(self.path)
        target.parent.mkdir(parents=True, exist_ok=True)
        tmp = target.with_name(target.name + ".tmp")
        text = json.dumps(clean, ensure_ascii=False, indent=2) + "\n"
        try:
            tmp.write_text(text, encoding="utf-8")
            os.replace(tmp, target)                   # 同目录 rename 是原子的
        except OSError as exc:
            raise ConfigError(f"写配置文件失败 {target}：{exc}") from exc

        self.data = clean
        self.last_error = None
        self._stamp = self._stamp_of_file()           # 自己写的，别当成"外部改动"再重载一遍
        self.revision += 1                            # 但要让同进程的上层知道该重新应用
        return clean

    def save_default(self) -> dict[str, Any]:
        return self.save(default_config())

    # ---------------------------------------------------------------- 热重载
    def changed_on_disk(self) -> bool:
        """文件是否被外部改过（配置页保存 / 用户手改）—— 只看指纹，不读内容。"""
        current = self._stamp_of_file()
        if current is None:
            return self._stamp is not None
        return current != self._stamp

    def reload_if_changed(self) -> ReloadResult:
        """被改过就重载。**重载失败不动现有数据**，只把错误带回给调用方。"""
        if not self.changed_on_disk():
            return ReloadResult(changed=False)
        if not self.exists():
            self._stamp = None
            self.last_error = f"配置文件不见了：{self.path}"
            return ReloadResult(changed=False, error=self.last_error)
        try:
            data = self.load()
        except ConfigError as exc:
            self.last_error = str(exc)
            self._stamp = self._stamp_of_file()       # 记下指纹，免得每帧都重试同一个坏文件
            return ReloadResult(changed=False, error=self.last_error)
        return ReloadResult(changed=True, data=data)

    # ---------------------------------------------------------------- 杂项
    def fingerprint(self) -> str:
        """当前数据的稳定指纹（给 HTTP ETag 用：内容没变就不必回传整份配置）。"""
        blob = json.dumps(self.data, ensure_ascii=False, sort_keys=True).encode("utf-8")
        return hashlib.sha1(blob).hexdigest()[:16]


# ==========================================================================
# 便捷访问器（浮层与后端都用它，免得各处自己翻字典）
# ==========================================================================

def appearance_of(cfg: dict[str, Any]) -> dict[str, Any]:
    """配置里的外观段（大小 / 数量 / 条高 / 强度 / 连线 / 旋转 / 模式）。"""
    appearance = cfg.get("appearance")
    return appearance if isinstance(appearance, dict) else default_config()["appearance"]


def audio_of(cfg: dict[str, Any]) -> dict[str, Any]:
    """配置里的音频段（来源 / 启动延迟 / 设备）。"""
    audio = cfg.get("audio")
    return audio if isinstance(audio, dict) else default_config()["audio"]


def render_of(cfg: dict[str, Any]) -> dict[str, Any]:
    """配置里的渲染段（高刷开关）。"""
    render = cfg.get("render")
    return render if isinstance(render, dict) else default_config()["render"]
