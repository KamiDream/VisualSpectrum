"""窗口标志的纯函数（不需要显示器，也不需要 QApplication）。"""

from __future__ import annotations

import pytest

pytest.importorskip("PyQt6.QtWidgets")

from PyQt6.QtCore import Qt  # noqa: E402

from app.overlay.window import MODE_TOP, MODE_WALLPAPER, overlay_window_flags  # noqa: E402


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
