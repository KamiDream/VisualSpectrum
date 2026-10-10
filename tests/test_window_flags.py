"""窗口标志的纯函数（不需要显示器，也不需要 QApplication）。"""

from __future__ import annotations

import pytest

pytest.importorskip("PyQt6.QtWidgets")

from PyQt6.QtCore import Qt  # noqa: E402

from app.overlay.window import (  # noqa: E402
    MODE_TOP, MODE_WALLPAPER, OverlayWindow, overlay_window_flags,
)


def test_top_is_on_top_and_click_through() -> None:
    flags = overlay_window_flags(MODE_TOP, "windows")
    assert bool(flags & Qt.WindowType.WindowStaysOnTopHint)
    assert bool(flags & Qt.WindowType.WindowTransparentForInput)
    assert not bool(flags & Qt.WindowType.WindowStaysOnBottomHint)


def test_wallpaper_is_on_bottom_and_not_click_through() -> None:
    flags = overlay_window_flags(MODE_WALLPAPER, "windows")
    assert bool(flags & Qt.WindowType.WindowStaysOnBottomHint)
    assert not bool(flags & Qt.WindowType.WindowTransparentForInput)


def test_both_are_frameless_and_focusless() -> None:
    for mode in (MODE_TOP, MODE_WALLPAPER):
        flags = overlay_window_flags(mode, "xcb")
        assert bool(flags & Qt.WindowType.FramelessWindowHint)
        assert bool(flags & Qt.WindowType.WindowDoesNotAcceptFocus)


def test_overlay_hides_on_close_and_signals_visibility() -> None:
    """关掉窗口 = 隐藏（不销毁），并能发出 visibilityChanged —— 这是"关了还能再唤回"的基础。"""
    assert hasattr(OverlayWindow, "closeEvent")
    assert hasattr(OverlayWindow, "showEvent")
    assert hasattr(OverlayWindow, "hideEvent")
    assert "visibilityChanged" in OverlayWindow.__dict__      # 类里自己定义的信号
