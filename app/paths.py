"""VisualSpectrum —— 跨平台路径解析。

三类路径分开管理，免得散落各处：

1. **只读资源** (`resource_dir`) —— `web/templates`、`web/static`、`assets/`。
   打包（PyInstaller）后要认 `sys._MEIPASS`，否则会 TemplateNotFound / 静态资源 404。
2. **可写数据** (`config_dir`) —— `config.json`、日志。
   **绝不能**写进程序目录：onefile 的"程序目录"是临时解包目录，装成只读包时更是直接失败。
3. **平台惯例位置** —— Linux 认 `XDG_CONFIG_HOME`，Windows 认 `%APPDATA%`。

测试要能完全不碰真实配置：设 `VISUALSPECTRUM_HOME` 就把第 2 类整体重定向到指定目录。
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

from . import APP_NAME

ENV_HOME = "VISUALSPECTRUM_HOME"      # 覆盖「可写数据」根目录（测试 / 便携模式用）


# --------------------------------------------------------------------------
# 1. 只读资源
# --------------------------------------------------------------------------

def is_frozen() -> bool:
    """是否被 PyInstaller（或同类工具）打包过。"""
    return bool(getattr(sys, "frozen", False))


def resource_dir() -> Path:
    """只读资源根目录 —— 里面应当有一个 `web/` 子目录。"""
    if is_frozen():
        base = getattr(sys, "_MEIPASS", None)
        if base:
            return Path(base)
        return Path(sys.executable).resolve().parent
    return Path(__file__).resolve().parent.parent


def web_dir() -> Path:
    """前端资源目录（templates + static）—— Flask 直接托管，没有构建产物。"""
    return resource_dir() / "web"


def assets_dir() -> Path:
    """静态素材目录（应用图标等）。"""
    return resource_dir() / "assets"


def templates_dir() -> Path:
    return web_dir() / "templates"


def static_dir() -> Path:
    return web_dir() / "static"


# --------------------------------------------------------------------------
# 2. 可写数据
# --------------------------------------------------------------------------

def config_dir() -> Path:
    """用户数据目录（配置与日志都在这儿）。

    * 覆盖优先：环境变量 `VISUALSPECTRUM_HOME`（测试与便携模式）
    * Windows : `%APPDATA%\\VisualSpectrum`
    * macOS   : `~/Library/Application Support/VisualSpectrum`
    * 其他/类 Unix: `$XDG_CONFIG_HOME/visualspectrum`（默认 `~/.config/visualspectrum`）
    """
    override = os.environ.get(ENV_HOME)
    if override:
        return Path(override).expanduser()

    if sys.platform == "win32":
        base = os.environ.get("APPDATA") or os.environ.get("LOCALAPPDATA")
        if base:
            return Path(base) / APP_NAME
        return Path.home() / f".{APP_NAME.lower()}"       # 没有 APPDATA 时的兜底
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / APP_NAME

    xdg = os.environ.get("XDG_CONFIG_HOME")
    root = Path(xdg).expanduser() if xdg else Path.home() / ".config"
    return root / APP_NAME.lower()


def config_path() -> Path:
    """配置文件路径（单文件 JSON）。"""
    return config_dir() / "config.json"


def log_path() -> Path:
    return config_dir() / "visualspectrum.log"


# --------------------------------------------------------------------------
# 3. 会话 / 平台判定
# --------------------------------------------------------------------------

def session_type() -> str:
    """当前会话类型：`windows` / `macos` / `wayland` / `x11` / `unknown`。

    用途：选"壁纸模式"的落地方式（Windows 用 WorkerW，其它平台退回"置于底层"）。
    """
    if sys.platform == "win32":
        return "windows"
    if sys.platform == "darwin":
        return "macos"
    if os.environ.get("WAYLAND_DISPLAY"):
        return "wayland"
    if os.environ.get("DISPLAY"):
        return "x11"
    return "unknown"


def is_windows() -> bool:
    return sys.platform == "win32"


def describe() -> dict[str, str]:
    """给启动日志与 `/api/meta` 用的一份路径/环境快照。"""
    return {
        "version_platform": sys.platform,
        "session": session_type(),
        "frozen": "yes" if is_frozen() else "no",
        "python": f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}",
        "resource_dir": str(resource_dir()),
        "web_dir": str(web_dir()),
        "config_dir": str(config_dir()),
        "config_path": str(config_path()),
        "log_path": str(log_path()),
    }
