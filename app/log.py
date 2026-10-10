"""极简日志 —— 写文件 + （可选）打终端。

用途很窄：把**看不见的东西**记下来（音频源开没开、掉没掉到 demo 模式、配置热重载了没有）。
两个通道：

* **文件**（默认 `~/.config/visualspectrum/visualspectrum.log`）：**永远全量记录**（含 debug）。
  每次启动重写（只留本次运行，免得越滚越大）。
* **stderr**：只有 `--debug` 或 `VISUALSPECTRUM_DEBUG=1` 时才输出。平时保持安静。

⚠ 写日志**绝不允许**把程序搞崩：任何 IO 异常都吞掉（大不了没有日志）。
"""

from __future__ import annotations

import os
import sys
import time
from io import TextIOBase
from pathlib import Path

__all__ = ["enable", "is_enabled", "init_file", "log_path", "debug", "info", "warn"]

_ENABLED = os.environ.get("VISUALSPECTRUM_DEBUG", "").strip() not in ("", "0", "false", "False")
_FILE: TextIOBase | None = None
_FILE_PATH: Path | None = None


def enable() -> None:
    """打开终端输出（`run.py --debug` 会调它）。文件通道不受影响，一直是开的。"""
    global _ENABLED
    _ENABLED = True


def is_enabled() -> bool:
    return _ENABLED


def init_file(path: Path | str | None = None) -> Path | None:
    """打开日志文件（**重写**：只留本次运行）。返回实际路径；失败返回 `None`。"""
    global _FILE, _FILE_PATH

    if path is None:
        try:
            from . import paths as _paths
            path = _paths.log_path()
        except Exception:                        # noqa: BLE001
            return None

    try:
        target = Path(path)
        target.parent.mkdir(parents=True, exist_ok=True)
        _FILE = open(target, "w", encoding="utf-8", errors="replace", buffering=1)
        _FILE_PATH = target
        return target
    except OSError:
        _FILE = None
        _FILE_PATH = None
        return None


def log_path() -> Path | None:
    """当前日志文件路径（没开就是 `None`）。"""
    return _FILE_PATH


def _stamp() -> str:
    """`HH:MM:SS.mmm` —— 时间戳（"中间隔了多久"这类问题全靠它）。"""
    now = time.time()
    return f"{time.strftime('%H:%M:%S', time.localtime(now))}.{int(now % 1 * 1000):03d}"


def _emit(tag: str, message: str, *, to_stderr: bool) -> None:
    line = f"[{tag} {_stamp()}] {message}"

    if _FILE is not None:
        try:
            _FILE.write(line + "\n")
        except Exception:                        # noqa: BLE001 - 日志不该影响主流程
            pass

    if to_stderr:
        print(f"  {line}", file=sys.stderr, flush=True)


def debug(message: str) -> None:
    """细节 → **只进文件**；终端要 `--debug` 才看。"""
    _emit("debug", message, to_stderr=_ENABLED)


def info(message: str) -> None:
    """重要步骤 → 文件 + 终端（终端始终显示，这类消息不多）。"""
    print(f"  {message}", flush=True)
    _emit("info", message, to_stderr=False)


def warn(message: str) -> None:
    """有东西不对但不致命 → 文件 + 终端。"""
    print(f"  ! {message}", flush=True)
    _emit("warn", message, to_stderr=False)
