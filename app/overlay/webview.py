"""浮层的 WebEngine 形态 —— 频段数据推给网页，动画与绘制交给 Canvas。

三条要点（都沿用参考项目里踩过的坑）：

1. **选型必须提前**：`QtWebEngineWidgets` 要在 `QApplication` 之前导入（Qt 的硬性要求），
   所以本模块对它的 import 都是**函数内的懒加载**；真正"提前导入"由 `run.py` 完成。
2. **启动即预热**：程序常驻后台，页面启动时就加载好（隐藏着），弹出（显示）时不用等 Chromium 冷启动。
3. **状态推过去要瘦**：只推 `bands`（一串 0~1 的数）与三项设置，别的都别送进 Chromium 进程。
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .. import log, paths

__all__ = ["available", "chromium_flags", "CHROMIUM_FLAGS", "HIGH_FPS_FLAG", "WebOverlay"]

#: 建议给 Chromium 的参数 —— **只此一处**，`run.py` 引它。
#: `--ignore-gpu-blocklist`：Chromium 常把集显判成"不受支持"而退回 CPU 光栅（SwiftShader）——
#: 我们这层是整屏半透明 + 一堆辉光模糊，走 CPU 就是肉眼可见的掉帧。
#: `--enable-gpu-rasterization`：让光栅化也走 GPU。
#: `--mute-audio`：我们的页面**没有任何声音**，让 Chromium 别去碰音频设备 ——
#:   启动那一瞬间多一个东西抢音频设备，蓝牙耳机上容易把音乐打断。
CHROMIUM_FLAGS = "--ignore-gpu-blocklist --enable-gpu-rasterization --mute-audio"

#: 解锁帧率上限（可选）。加上后 Chromium 不再按 ~60Hz 自我排帧，动画更顺，代价是更费 CPU/GPU。
HIGH_FPS_FLAG = "--disable-frame-rate-limit"


def chromium_flags(*, high_fps: bool = False) -> str:
    """给 Chromium 的完整参数串（`high_fps` 时额外解锁帧率上限）。"""
    return f"{CHROMIUM_FLAGS} {HIGH_FPS_FLAG}" if high_fps else CHROMIUM_FLAGS


def available() -> bool:
    """装了 `PyQt6-WebEngine` 吗。"""
    import importlib.util

    return importlib.util.find_spec("PyQt6.QtWebEngineWidgets") is not None


class WebOverlay:
    """一扇"网页窗"：加载一次，之后只推数据。"""

    def __init__(self, host) -> None:
        from PyQt6.QtCore import Qt
        from PyQt6.QtGui import QColor
        from PyQt6.QtWebEngineCore import QWebEnginePage, QWebEngineSettings
        from PyQt6.QtWebEngineWidgets import QWebEngineView
        from PyQt6.QtWidgets import QVBoxLayout

        self._ready = False
        self._queue: list[str] = []
        self._host = host

        class _Page(QWebEnginePage):
            """把页面里的 console 消息转进日志 —— 浮层出问题时那是唯一的线索。"""

            def javaScriptConsoleMessage(self, level, message, line, source) -> None:  # noqa: N802
                log.debug(f"浮层页面[{level.name} 第{line}行]: {message}")

        self.view = QWebEngineView(host)
        # 透明 + 不吃鼠标事件：这一层只负责画，指针的事与它无关。
        self.view.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground, True)
        self.view.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents, True)
        self.view.setStyleSheet("background: transparent;")
        self.view.setContextMenuPolicy(Qt.ContextMenuPolicy.NoContextMenu)

        page = _Page(self.view)
        page.setBackgroundColor(QColor(0, 0, 0, 0))
        settings = page.settings()
        settings.setAttribute(QWebEngineSettings.WebAttribute.ShowScrollBars, False)
        settings.setAttribute(QWebEngineSettings.WebAttribute.ScrollAnimatorEnabled, False)
        settings.setAttribute(QWebEngineSettings.WebAttribute.FocusOnNavigationEnabled, False)
        self.view.setPage(page)

        layout = QVBoxLayout(host)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(self.view)

        self.view.loadFinished.connect(self._on_load_finished)

    # ------------------------------------------------------------------ 加载
    def load(self) -> bool:
        from PyQt6.QtCore import QUrl

        page_path: Path = paths.web_dir() / "overlay.html"
        if not page_path.is_file():
            log.warn(f"浮层页面不存在：{page_path}")
            return False
        log.debug(f"加载浮层页面：{page_path}")
        self.view.load(QUrl.fromLocalFile(str(page_path)))
        return True

    def _on_load_finished(self, ok: bool) -> None:
        if not ok:
            log.warn("浮层页面加载失败（WebEngine）")
            self._ready = False
            return
        self._ready = True
        pending, self._queue = self._queue, []
        log.debug(f"浮层页面已就绪（补发 {len(pending)} 次调用）")
        for script in pending:
            self._run(script)

    @property
    def ready(self) -> bool:
        return self._ready

    # ------------------------------------------------------------------ 推数据
    def call(self, method: str, *payloads: Any) -> None:
        """调网页侧的 `window.VisualSpectrum.<method>(...)`（页面没好就先排队）。"""
        args = ", ".join(json.dumps(payload, ensure_ascii=True) for payload in payloads)
        # `ensure_ascii=True`：把非 ASCII 全转成 \uXXXX，免得 U+2028/U+2029 在 JS 里变成换行。
        script = f"window.VisualSpectrum && window.VisualSpectrum.{method}({args});"
        if not self._ready:
            self._queue.append(script)
            return
        self._run(script)

    def _run(self, script: str) -> None:
        self.view.page().runJavaScript(script)

    def set_appearance(self, appearance: dict[str, Any]) -> None:
        """推三项设置（大小 / 数量 / 模式）。"""
        self.call("setAppearance", dict(appearance))

    def publish(self, bands: list[float], *, live: bool, source: str) -> None:
        """推一帧频谱（频段保留 3 位小数 —— 再多的精度对画面毫无意义，只是徒增传输量）。"""
        self.call("pushFrame", {"bands": [round(float(b), 3) for b in bands],
                                "live": bool(live), "source": str(source)})

    def dispose(self) -> None:
        try:
            self.view.setParent(None)
            self.view.deleteLater()
        except Exception:                        # noqa: BLE001
            pass
