"""系统托盘 —— 打开配置 / 重载 / 退出。

⚠ **托盘不是必然存在的**：Wayland 下要不要显示托盘取决于合成器有没有实现
StatusNotifierItem 宿主。没有宿主时 Qt 会直接告诉我们"没有托盘"，这时**不能报错退出** ——
程序本身照常工作（浮层不依赖托盘），只是少了一个入口，于是把提示打到终端里。

图标在运行期用 `QPainter` 画（一圈跳动的频谱条），不依赖任何图片素材 ——
"没有图标素材就起不来"这种事不该发生。
"""

from __future__ import annotations

import math

from PyQt6.QtCore import QObject, QPointF, Qt, pyqtSignal
from PyQt6.QtGui import QAction, QColor, QIcon, QPainter, QPen, QPixmap
from PyQt6.QtWidgets import QMenu, QSystemTrayIcon

from . import APP_NAME

__all__ = ["Tray", "is_available", "make_icon"]


def make_icon(size: int = 64) -> QIcon:
    """画一个"环形频谱"应用图标（透明底 + 一圈青色竖条）。"""
    pixmap = QPixmap(size, size)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing, True)

    center = QPointF(size / 2, size / 2)
    radius = size * 0.30
    bars = 24
    pen = QPen()
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    pen.setWidthF(max(1.5, size * 0.05))
    for i in range(bars):
        angle = 2 * math.pi * i / bars
        # 让高度呈"音乐频谱"的起伏
        length = radius * (0.35 + 0.75 * (0.5 + 0.5 * math.sin(angle * 3.0)))
        pen.setColor(QColor.fromHsvF((190 + 30 * (length / radius)) / 360.0, 0.75, 1.0))
        painter.setPen(pen)
        x0 = center.x() + math.cos(angle) * radius
        y0 = center.y() + math.sin(angle) * radius
        x1 = center.x() + math.cos(angle) * (radius + length)
        y1 = center.y() + math.sin(angle) * (radius + length)
        painter.drawLine(QPointF(x0, y0), QPointF(x1, y1))

    painter.setPen(Qt.PenStyle.NoPen)
    painter.setBrush(QColor("#7EE7FF"))
    painter.drawEllipse(center, size * 0.05, size * 0.05)
    painter.end()
    return QIcon(pixmap)


def is_available() -> bool:
    return QSystemTrayIcon.isSystemTrayAvailable()


class Tray(QObject):
    """托盘图标 + 菜单。信号与 `run.py` 接起来。"""

    openConfigRequested = pyqtSignal()
    reloadRequested = pyqtSignal()
    quitRequested = pyqtSignal()

    def __init__(self, *, status: str = "") -> None:
        super().__init__()
        self._icon = QSystemTrayIcon(make_icon(), self)
        self._icon.setToolTip(f"{APP_NAME}  ·  {status}" if status else APP_NAME)

        menu = QMenu()
        act_open = QAction("打开配置页", menu)
        act_open.triggered.connect(self.openConfigRequested.emit)
        act_reload = QAction("重新载入配置", menu)
        act_reload.triggered.connect(self.reloadRequested.emit)
        act_quit = QAction("退出", menu)
        act_quit.triggered.connect(self.quitRequested.emit)
        menu.addAction(act_open)
        menu.addAction(act_reload)
        menu.addSeparator()
        menu.addAction(act_quit)
        self._menu = menu

        self._icon.setContextMenu(menu)
        self._icon.activated.connect(self._on_activated)

    def _on_activated(self, reason) -> None:
        if reason == QSystemTrayIcon.ActivationReason.Trigger:
            self.openConfigRequested.emit()

    def show(self) -> bool:
        """能显示就显示；当前环境没有托盘宿主则返回 `False`（调用方打提示）。"""
        if not is_available():
            return False
        self._icon.show()
        return True

    def set_status(self, text: str) -> None:
        self._icon.setToolTip(f"{APP_NAME}  ·  {text}" if text else APP_NAME)

    def notify(self, title: str, message: str) -> None:
        if self._icon.isVisible():
            self._icon.showMessage(title, message, make_icon())

    def hide(self) -> None:
        self._icon.hide()
