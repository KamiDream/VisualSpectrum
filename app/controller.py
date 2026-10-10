"""编排：配置 → 音频引擎 → 浮层窗口。

三件事：

1. **推帧**：用一个 QTimer 以 ~50fps 把音频引擎的"最新一帧频段"推给浮层网页；
2. **热重载**：另一个低频 QTimer 盯着配置文件，被改了就当场应用（大小 / 数量 / 模式都能热生效）；
3. **对外**：给配置页提供"当前频谱 / 运行状态"（`latest_spectrum()`），让预览显示真实数据。
"""

from __future__ import annotations

from PyQt6.QtCore import QObject, QTimer, pyqtSignal

from . import log
from .audio import AudioEngine
from .config import ConfigStore, appearance_of, audio_of
from .overlay import MODE_TOP, OverlayWindow

__all__ = ["OverlayController"]

#: 往浮层推数据的间隔（毫秒）—— ~50fps，比音频帧略快，画面不会"追不上"。
FRAME_MS = 20

#: 盯配置文件的间隔（毫秒）—— 1s 足够，且几乎不耗 CPU。
RELOAD_MS = 1000


class OverlayController(QObject):
    """把音频、配置、浮层串起来的那个对象。"""

    statusChanged = pyqtSignal(str)
    toast = pyqtSignal(str)
    #: 从**别的线程**（Flask 配置页）请求重载。Qt 对象只能在主线程碰，所以这里
    #: 只发一个信号，真正的重载由主线程的槽 `_on_reload_requested` 跑。
    reloadRequested = pyqtSignal(str)
    #: 从别的线程（配置页）请求显示/隐藏浮层：`True`/`False`，或 `None` = 切换。
    #: 同样只能发信号 —— 真正的 show/hide 由主线程的槽 `_on_overlay_requested` 跑。
    overlayRequested = pyqtSignal(object)
    #: 浮层显示状态变化（`True`=显示了）—— 转发自窗口，给托盘同步用。
    overlayVisibilityChanged = pyqtSignal(bool)

    def __init__(self, store: ConfigStore, parent: QObject | None = None) -> None:
        super().__init__(parent)
        self.store = store
        self._appearance = dict(appearance_of(store.data))
        self._audio = dict(audio_of(store.data))

        self._engine = AudioEngine(count=int(self._appearance["count"]),
                                   source=str(self._audio["source"]),
                                   delay_ms=int(self._audio["delay_ms"]),
                                   device=str(self._audio.get("device", "auto")),
                                   gains=self._gains())
        self._window = OverlayWindow(mode=self._mode(), appearance=self._appearance)
        #: 浮层是否可见（**在主线程维护的一个普通 bool**）—— 给 Flask 线程读的。
        #: ⚠ 不能从 Flask 线程直接调 `_window.isVisible()`（碰 Qt widget 有风险），所以缓存一份。
        self._overlay_visible = False

        # 跨线程重载：信号发出去 → 排到主线程的事件循环里执行（这是 Qt 的 AutoConnection）
        self.reloadRequested.connect(self._on_reload_requested)
        self.overlayRequested.connect(self._on_overlay_requested)
        # 窗口显示 / 隐藏（含被关闭拦成隐藏）→ 更新缓存 + 转发给托盘
        self._window.visibilityChanged.connect(self._on_window_visibility)

        self._frame_timer = QTimer(self)
        self._frame_timer.setInterval(FRAME_MS)
        self._frame_timer.timeout.connect(self._push_frame)

        self._reload_timer = QTimer(self)
        self._reload_timer.setInterval(RELOAD_MS)
        self._reload_timer.timeout.connect(self._check_reload)

        self._running = False

    # ------------------------------------------------------------------ 配置
    def _mode(self) -> str:
        return str(self._appearance.get("mode", MODE_TOP))

    def _gains(self) -> tuple[float, float, float]:
        """低 / 中 / 高音的占比倍数（配置里是百分比）。"""
        return self._gains_for(self._appearance)

    @staticmethod
    def _gains_for(appearance: dict) -> tuple[float, float, float]:
        return (int(appearance.get("bass", 100)) / 100.0,
                int(appearance.get("mid", 100)) / 100.0,
                int(appearance.get("treble", 100)) / 100.0)

    # ------------------------------------------------------------------ 生命周期
    def start(self) -> None:
        self._window.show_overlay()
        self._engine.start()
        self._frame_timer.start()
        self._reload_timer.start()
        self._running = True
        self.statusChanged.emit(self.status_text())
        log.info(f"已启动：模式 {self._mode()} · 大小 {self._appearance['size']}% · "
                 f"数量 {self._appearance['count']}")

    def stop(self) -> None:
        self._frame_timer.stop()
        self._reload_timer.stop()
        self._engine.stop()
        self._window.dispose()
        self._running = False

    # ------------------------------------------------------------------ 显示 / 隐藏
    def show_overlay(self) -> None:
        """重新显示浮层（关掉之后从托盘 / 配置页唤回来）。**只能在主线程调。**"""
        self._window.show_overlay()
        self.statusChanged.emit(self.status_text())

    def hide_overlay(self) -> None:
        """隐藏浮层（但它继续在后台跑着 —— 再 show 就回来了）。"""
        self._window.hide()

    def toggle_overlay(self) -> None:
        if self._overlay_visible:
            self.hide_overlay()
        else:
            self.show_overlay()

    def overlay_visible(self) -> bool:
        """浮层当前是否可见 —— **线程安全**（读缓存，不碰 Qt widget）。"""
        return bool(self._overlay_visible)

    def request_overlay(self, visible: bool | None = None) -> None:
        """**线程安全**：请求显示（`True`）/ 隐藏（`False`）/ 切换（`None`）浮层。

        ⚠ 与 `request_reload` 同理：Flask 跑在后台线程，show/hide 只能排到主线程做。
        """
        self.overlayRequested.emit(visible)

    def _on_overlay_requested(self, visible: object) -> None:
        if visible is None:
            self.toggle_overlay()
        elif visible:
            self.show_overlay()
        else:
            self.hide_overlay()

    def _on_window_visibility(self, visible: bool) -> None:
        self._overlay_visible = bool(visible)
        self.overlayVisibilityChanged.emit(self._overlay_visible)
        self.statusChanged.emit(self.status_text())

    # ------------------------------------------------------------------ 帧
    def _push_frame(self) -> None:
        snapshot = self._engine.snapshot()
        self._window.publish(snapshot["bands"],
                             live=bool(snapshot["live"]),
                             source=str(snapshot["source"]))

    # ------------------------------------------------------------------ 热重载
    def _check_reload(self) -> None:
        result = self.store.reload_if_changed()
        if result.error:
            self.toast.emit(f"配置有问题：{result.error}")
            return
        if result.changed:
            self._apply("配置文件变更")

    def reload(self, reason: str = "手动") -> None:
        """从磁盘重读并应用（**只能在主线程调**）。"""
        try:
            self.store.load()
        except Exception as exc:                 # noqa: BLE001
            self.toast.emit(f"重载失败：{exc}")
            return
        self._apply(reason)

    def request_reload(self, reason: str = "配置页保存") -> None:
        """**线程安全**：请主线程重载。

        ⚠ 配置页是 Flask，跑在**后台线程**里 —— 它保存后不能直接碰 Qt / WebEngine
          （`set_appearance` 最终会调 `runJavaScript`），那样会让浮层卡住（表现就是
          "一保存音乐就不动了"）。所以这里只发信号，真正干活排到主线程。
        """
        self.reloadRequested.emit(str(reason))

    def _on_reload_requested(self, reason: str) -> None:
        self.reload(reason or "配置页保存")

    def _apply(self, reason: str) -> None:
        appearance = dict(appearance_of(self.store.data))
        audio = dict(audio_of(self.store.data))
        if int(appearance["count"]) != int(self._appearance.get("count", 0)):
            self._engine.reconfigure(int(appearance["count"]))
        self._engine.apply_settings(source=str(audio["source"]),
                                    delay_ms=int(audio["delay_ms"]),
                                    device=str(audio.get("device", "auto")),
                                    gains=self._gains_for(appearance))
        self._window.configure(appearance=appearance, mode=str(appearance.get("mode", "")))
        self._appearance = appearance
        self._audio = audio
        log.info(f"应用新配置（{reason}）：大小 {appearance['size']}% · "
                 f"数量 {appearance['count']} · 模式 {appearance['mode']} · "
                 f"音频 {audio['source']}/{audio.get('device', 'auto')} · "
                 f"占比 {appearance.get('bass', 100)}/{appearance.get('mid', 100)}/"
                 f"{appearance.get('treble', 100)}")
        self.statusChanged.emit(self.status_text())

    # ------------------------------------------------------------------ 对外
    def latest_spectrum(self) -> dict:
        """给配置页的实时预览：当前这一帧 + 运行状态。"""
        snapshot = self._engine.snapshot()
        return {
            "bands": snapshot["bands"],
            "live": bool(snapshot["live"]),
            "source": str(snapshot["source"]),
            "running": self._running,
            "overlay_visible": self.overlay_visible(),
            "appearance": dict(self._appearance),
        }

    def status_text(self) -> str:
        base = (f"模式 {self._mode()} · 大小 {self._appearance['size']}% · "
                f"数量 {self._appearance['count']}")
        return base if self._overlay_visible else f"{base} · 已隐藏"
