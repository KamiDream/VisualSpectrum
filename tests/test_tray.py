"""托盘行为：**点图标 = 显示可视化**（只显示不隐藏；打开配置在右键菜单里）。

只测那个判断用的**纯函数**（不建 QApplication —— offscreen 下 `QSystemTrayIcon` 会直接崩溃）。
"""

from __future__ import annotations

from pathlib import Path

from app.tray import activation_shows_overlay

_ROOT = Path(__file__).resolve().parent.parent


def test_left_and_middle_click_show_overlay() -> None:
    assert activation_shows_overlay("Trigger") is True           # 左键
    assert activation_shows_overlay("MiddleClick") is True       # 中键


def test_double_and_right_click_do_nothing() -> None:
    # 双击在多数平台会先发一次 Trigger —— 若也绑上会触发两次，所以故意不管；
    # 右键（Context）留给菜单。
    assert activation_shows_overlay("DoubleClick") is False
    assert activation_shows_overlay("Context") is False
    assert activation_shows_overlay("Unknown") is False


def test_activation_wires_to_show_overlay_not_config() -> None:
    """源码级护栏：`_on_activated` 必须发 `showOverlayRequested`（只显示），
    不能再发 `openConfigRequested`（开配置页），也不能发 `toggleOverlayRequested`（会隐藏）。"""
    source = (_ROOT / "app" / "tray.py").read_text(encoding="utf-8")
    activated = source.split("def _on_activated", 1)[1].split("def ", 1)[0]
    assert "showOverlayRequested.emit()" in activated
    assert "toggleOverlayRequested" not in activated
    assert "openConfigRequested" not in activated
