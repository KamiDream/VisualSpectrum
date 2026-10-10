"""一次性冒烟脚本（名字以 _ 开头，pytest 不会收集）：离屏建一扇浮层窗口，推几帧后退出。

    QT_QPA_PLATFORM=offscreen conda run -n Menu python tests/_smoke_overlay.py

只验证"窗口 + WebEngine + 推数据"这条线接得通，不验证画面好看与否。
"""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

os.environ.setdefault("VISUALSPECTRUM_HOME", tempfile.mkdtemp())
os.environ["QT_QPA_PLATFORM"] = "offscreen"
os.environ["QT_QPA_PLATFORMTHEME"] = ""
os.environ["QTWEBENGINE_DISABLE_SANDBOX"] = "1"
os.environ["QTWEBENGINE_CHROMIUM_FLAGS"] = (
    "--no-sandbox --disable-gpu --single-process --disable-dev-shm-usage")
_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(_ROOT))

from PyQt6.QtCore import QTimer, Qt  # noqa: E402
from PyQt6.QtGui import QSurfaceFormat  # noqa: E402
from PyQt6.QtWidgets import QApplication  # noqa: E402

QApplication.setAttribute(Qt.ApplicationAttribute.AA_ShareOpenGLContexts, True)
import PyQt6.QtWebEngineWidgets  # noqa: E402,F401

surface = QSurfaceFormat()
surface.setAlphaBufferSize(8)
QSurfaceFormat.setDefaultFormat(surface)

from app.overlay import OverlayWindow  # noqa: E402

app = QApplication(sys.argv[:1])
window = OverlayWindow(mode="top", appearance={"size": 55, "count": 64, "mode": "top"})
window.show_overlay()

frames = {"n": 0}


def tick() -> None:
    frames["n"] += 1
    bands = [abs((i % 7) / 7.0) for i in range(64)]
    window.publish(bands, live=True, source="smoke")
    if frames["n"] >= 5:
        print("SMOKE-OK: pushed", frames["n"], "frames; web ready =",
              window._web.ready if window._web else None)
        window.dispose()
        app.quit()


timer = QTimer()
timer.timeout.connect(tick)
timer.start(200)
QTimer.singleShot(15000, app.quit)
sys.exit(app.exec())
