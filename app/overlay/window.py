"""浮层窗口 —— 两种形态，按配置里的 `mode` 分叉。

    top       浮在最上层：盖在所有普通窗口之上，**鼠标穿透**（点了不挡事），不抢焦点。
              做"桌面上的装饰"时用它。

    wallpaper 贴到桌面壁纸层：被所有窗口盖住，只在"看桌面"时可见。
              * Windows：把窗口挂到 `WorkerW`（承载桌面图标的那一层）下面，于是它就
                真的成了"壁纸的一部分"——这是唯一能让它在"显示桌面 / 最小化全部"时也露出来的做法；
              * 其它平台：退回 `WindowStaysOnBottomHint`（尽量沉到底），行为因 WM 而异，是 best-effort。

两种形态都：整屏大小、无边框、背景透明、`QtWebEngine` 只负责画。
大小 / 数量 / 模式的变化都是**运行时热生效**（切换模式会重建一次原生窗口）。
"""

from __future__ import annotations

from PyQt6.QtCore import QRect, Qt, pyqtSignal
from PyQt6.QtGui import QGuiApplication
from PyQt6.QtWidgets import QWidget

from .. import log

__all__ = ["MODE_TOP", "MODE_WALLPAPER", "overlay_window_flags", "OverlayWindow"]

MODE_TOP = "top"
MODE_WALLPAPER = "wallpaper"


def overlay_window_flags(mode: str, platform: str) -> Qt.WindowType:
    """浮层的窗口标志 —— **纯函数**，便于在没有显示器的环境里断言。

    * 两种形态都**无边框、不接受焦点、尽量置顶或置底**；
    * `top` 额外要 **输入透明**（`WindowTransparentForInput`）：鼠标点上去要落到它后面的
      窗口，而不是被这层装饰吃掉；
    * `wallpaper` 要 **置于底层**（Windows 上真正的"贴壁纸"靠原生 `SetParent`，见
      `_attach_to_desktop()`，但标志先给上，其它平台就靠它）。
    """
    flags = (Qt.WindowType.FramelessWindowHint
             | Qt.WindowType.WindowDoesNotAcceptFocus
             | Qt.WindowType.Tool)
    if mode == MODE_WALLPAPER:
        flags |= Qt.WindowType.WindowStaysOnBottomHint
    else:
        flags |= Qt.WindowType.WindowStaysOnTopHint
        flags |= Qt.WindowType.WindowTransparentForInput
    return flags


class OverlayWindow(QWidget):
    """整屏浮层：一扇透明窗 + 里面一层 `QtWebEngine`。

    生命周期：建一次，反复用 —— `configure()` 改大小/数量/模式，`publish()` 推每一帧。

    ⚠ **关掉 = 藏起来，不是销毁**：窗口管理器的“关闭”会走 `closeEvent`，这里把它拦成
      `hide()`（`event.ignore()`）—— 窗口对象继续留着，之后从托盘 / 配置页还能再唤回来。
      这样才能避免“把可视化关了就没手段再打开，只能重启程序”。
    """

    #: 显示 / 隐藏变化时发出来（`True`=显示了）—— 托盘据此同步「显示可视化」的勾选态。
    visibilityChanged = pyqtSignal(bool)

    def __init__(self, *, mode: str = MODE_TOP, appearance: dict | None = None,
                 parent: QWidget | None = None) -> None:
        super().__init__(parent)

        self.mode = mode if mode in (MODE_TOP, MODE_WALLPAPER) else MODE_TOP
        self.appearance: dict = dict(appearance or {})
        self._web = None

        self._setup_window()
        self._setup_renderer()

    # ------------------------------------------------------------------ 窗口
    def _setup_window(self) -> None:
        self.setWindowFlags(overlay_window_flags(self.mode, QGuiApplication.platformName()))
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground, True)
        self.setAttribute(Qt.WidgetAttribute.WA_NoSystemBackground, True)
        self.setAttribute(Qt.WidgetAttribute.WA_ShowWithoutActivating, True)
        self.setFocusPolicy(Qt.FocusPolicy.NoFocus)
        self.setCursor(Qt.CursorShape.BlankCursor)
        self.setWindowTitle("VisualSpectrum")

    def _setup_renderer(self) -> None:
        from . import webview

        self._web = webview.WebOverlay(self)
        self._web.load()
        #: 页面还没就绪时这一步会先排队，就绪后补发 —— 于是启动即拿到正确的大小 / 数量。
        self._web.set_appearance(self.appearance)

    def dispose(self) -> None:
        """收尾：把网页那一层停掉（否则 Chromium 渲染进程会一直占着）。"""
        if self._web is not None:
            self._web.dispose()
            self._web = None

    # ------------------------------------------------------------------ 显示 / 隐藏
    # ⚠ PyQt6 的桩里这些回调的参数名是 `a0`；这里用更可读的 `event`，于是 Pyright 会报
    #   "参数名不兼容" —— 那是 Qt 桩的锅，就地忽略。
    def showEvent(self, event) -> None:          # noqa: N802  # pyright: ignore[reportIncompatibleMethodOverride]
        super().showEvent(event)
        self.visibilityChanged.emit(True)

    def hideEvent(self, event) -> None:          # noqa: N802  # pyright: ignore[reportIncompatibleMethodOverride]
        super().hideEvent(event)
        self.visibilityChanged.emit(False)

    def closeEvent(self, event) -> None:         # noqa: N802  # pyright: ignore[reportIncompatibleMethodOverride]
        """关窗口 = 藏起来，**不销毁** —— 这样日后还能再唤回来。"""
        log.info("浮层窗口被关闭 —— 改为隐藏（可从托盘 / 配置页再唤回）")
        event.ignore()
        self.hide()

    # ------------------------------------------------------------------ 几何
    def _target_rect(self) -> QRect:
        """整屏几何 —— 用**主屏**（多屏时先只照顾主屏，够用且不添乱）。"""
        screen = QGuiApplication.primaryScreen()
        if screen is None:
            return QRect(0, 0, 1280, 720)
        return screen.geometry()

    def show_overlay(self) -> None:
        """铺满主屏并显示。"""
        rect = self._target_rect()
        self.setGeometry(rect)
        self.show()
        if self.mode == MODE_WALLPAPER:
            self._attach_to_desktop()
        self.raise_if_top()

    def raise_if_top(self) -> None:
        if self.mode == MODE_TOP:
            self.raise_()

    # ------------------------------------------------------------------ 配置
    def configure(self, *, appearance: dict | None = None, mode: str | None = None) -> None:
        """配置热重载后更新（大小 / 数量 / 模式都能当场生效）。"""
        if appearance is not None:
            self.appearance = dict(appearance)
        if mode is not None and mode != self.mode and mode in (MODE_TOP, MODE_WALLPAPER):
            self._switch_mode(mode)
        if self._web is not None:
            self._web.set_appearance(self.appearance)

    def _switch_mode(self, mode: str) -> None:
        """切换顶层 / 壁纸：改标志 → 重建原生窗口 → 重新贴回（或脱离）桌面。"""
        log.info(f"切换模式：{self.mode} → {mode}")
        if self.mode == MODE_WALLPAPER:
            self._detach_from_desktop()
        self.mode = mode
        self.setWindowFlags(overlay_window_flags(self.mode, QGuiApplication.platformName()))
        self.show_overlay()

    # ------------------------------------------------------------------ 推帧
    def publish(self, bands: list[float], *, live: bool = False, source: str = "") -> None:
        if self._web is not None:
            self._web.publish(bands, live=live, source=source)

    # ------------------------------------------------------------------ 平台：贴桌面
    def _attach_to_desktop(self) -> None:
        """Windows：把窗口挂到 `WorkerW` 下，真正变成"壁纸的一部分"。"""
        if QGuiApplication.platformName().lower() != "windows":
            return
        try:
            _attach_to_workerw(int(self.winId()))
        except Exception as exc:                  # noqa: BLE001 - 贴不上就退回置底
            log.warn(f"贴桌面失败（{exc}）—— 这次只置于底层")

    def _detach_from_desktop(self) -> None:
        if QGuiApplication.platformName().lower() != "windows":
            return
        try:
            _detach_from_workerw(int(self.winId()))
        except Exception:                         # noqa: BLE001
            pass


# --------------------------------------------------------------------------
# Windows：WorkerW / Progman（只在 Windows 上被调用）
# --------------------------------------------------------------------------

_WM_SPAWN_WORKERW = 0x052C


def _attach_to_workerw(hwnd: int) -> None:
    """把 `hwnd` 挂进桌面壁纸层。

    步骤（业界通行的做法）：给 `Progman` 发匿名的 `0x052C` 消息，逼它**生出**一个
    `WorkerW`（承载桌面图标那层下面还有一个空的 WorkerW，就是我们要挂的那层）；
    然后枚举顶层窗口，找到"在 SHELLDLL_DefView 之后"的那个 WorkerW，`SetParent` 过去。
    """
    import ctypes
    from ctypes import wintypes

    user32 = ctypes.windll.user32
    progman = user32.FindWindowW("Progman", None)
    if not progman:
        raise RuntimeError("找不到 Progman（这不是标准的 Windows 桌面？）")

    # 逼出 WorkerW
    result = wintypes.DWORD()
    user32.SendMessageTimeoutW(progman, _WM_SPAWN_WORKERW, 0, 0, 0, 1000,
                               ctypes.byref(result))

    workerw = 0
    enum_proc = ctypes.WINFUNCTYPE(ctypes.c_bool, wintypes.HWND, wintypes.LPARAM)

    def _callback(top_hwnd, _lparam):
        nonlocal workerw
        shell = user32.FindWindowExW(top_hwnd, 0, "SHELLDLL_DefView", None)
        if shell:
            # 承载桌面图标的那个 WorkerW 的**下一个**兄弟才是空的那层
            workerw = user32.FindWindowExW(0, top_hwnd, "WorkerW", None)
        return True

    user32.EnumWindows(enum_proc(_callback), 0)
    target = workerw or progman
    if not user32.SetParent(hwnd, target):
        raise RuntimeError("SetParent 失败")

    # 成为子窗口后位置变成相对父窗口 —— 重新铺满整个桌面（WorkerW 覆盖整屏）
    rect = wintypes.RECT()
    user32.GetClientRect(target, ctypes.byref(rect))
    swp_nozorder, swp_noactivate = 0x0004, 0x0010
    user32.SetWindowPos(hwnd, 0, 0, 0, rect.right - rect.left, rect.bottom - rect.top,
                        swp_nozorder | swp_noactivate)


def _detach_from_workerw(hwnd: int) -> None:
    """把窗口从桌面层摘回来（切回顶层模式时用）。"""
    import ctypes

    ctypes.windll.user32.SetParent(hwnd, 0)
