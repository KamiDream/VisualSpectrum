#!/usr/bin/env python3
"""VisualSpectrum —— 启动入口。

    conda run -n VSp python run.py              # 启动（托盘 + 浮层 + 配置页）
    conda run -n VSp python run.py --selfcheck  # 只跑环境自检后退出
    conda run -n VSp python run.py --no-tray    # 不起托盘

启动顺序：

    1. 环境自检（音频后端 / WebEngine / 配置）
    2. 读配置（没有就写一份默认的；坏了就用默认值并说明）
    3. **单实例检查**（已经在跑 → 请它打开配置页，自己退出）
    4. 起浮层与音频引擎
    5. 起配置页（Flask，可选）
    6. 进 Qt 事件循环

⚠ 浮层渲染依赖 **PyQt6-WebEngine**（整套辉光 / 粒子 / 音乐元素就是为 Canvas 写的）。
   没装会明确报错并给出安装命令，而不是悄悄退化成"什么都没有"。
"""

from __future__ import annotations

import argparse
import os
import signal
import sys
import threading
from pathlib import Path

_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(_ROOT))


def _fix_stdio() -> None:
    """Windows 控制台默认 GBK，打印 `✓` / `⚠` 会 UnicodeEncodeError —— 显式改 UTF-8。"""
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:
            continue
        try:
            reconfigure(encoding="utf-8", errors="replace", line_buffering=True)
        except Exception:                        # noqa: BLE001
            pass


def _parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="VisualSpectrum —— 桌面环形频谱可视化（壁纸 / 顶层两种模式）",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="示例:\n"
               "  python run.py --selfcheck      只检查环境\n"
               "  python run.py --no-tray        不起托盘\n"
               "  python run.py --config ~/vs.json\n",
    )
    parser.add_argument("--config", type=Path, default=None,
                        help="配置文件路径（默认在用户数据目录，见 --selfcheck 的输出）")
    parser.add_argument("--no-tray", action="store_true", help="不显示系统托盘图标")
    parser.add_argument("--no-config-server", action="store_true",
                        help="不起配置页（只跑浮层与音频引擎）")
    parser.add_argument("--selfcheck", action="store_true", help="只做环境自检后退出")
    parser.add_argument("--debug", action="store_true",
                        help="打开调试输出（音频来源 / 配置热重载 / 页面 console 都打出来）")
    parser.add_argument("--high-fps", action="store_true",
                        help="解锁 Chromium 帧率上限（动画更顺，代价是更费 CPU/GPU）")
    parser.add_argument("--log-file", default=None, type=Path, metavar="路径",
                        help="日志文件路径（默认：源码运行时写 ./logs/visualspectrum.log）")
    parser.add_argument("--port", type=int, default=int(os.environ.get("VISUALSPECTRUM_PORT", "8770")),
                        help="配置页端口（默认 8770）")
    return parser.parse_args(argv)


def _print(text: str = "") -> None:
    print(text, flush=True)


def _prepare_webengine(*, high_fps: bool = False) -> bool:
    """`QtWebEngine` 的**前置动作** —— 必须在 `QApplication` 之前做完。

    QtWebEngine 有两条硬性约束：`QtWebEngineWidgets` 要在 `QApplication` 前导入；
    `AA_ShareOpenGLContexts` 也要在建 app 之前设上。
    顺手打开 GPU 光栅化（Chromium 常把集显判成"不受支持"而退回 CPU 光栅，那样整屏辉光会掉帧）。
    """
    from app import log
    from app.overlay.webview import chromium_flags

    flags = chromium_flags(high_fps=high_fps)
    pushed = False
    if not os.environ.get("QTWEBENGINE_CHROMIUM_FLAGS"):
        os.environ["QTWEBENGINE_CHROMIUM_FLAGS"] = flags
        pushed = True
    log.debug(f"Chromium 参数: {os.environ['QTWEBENGINE_CHROMIUM_FLAGS']}"
              f"（{'默认' if pushed else '用户自己设的，不动它'}）")
    try:
        from PyQt6.QtCore import Qt
        from PyQt6.QtWidgets import QApplication

        QApplication.setAttribute(Qt.ApplicationAttribute.AA_ShareOpenGLContexts, True)
        import PyQt6.QtWebEngineWidgets  # noqa: F401 - 只为"提前导入"这个副作用
        return True
    except Exception as exc:                     # noqa: BLE001
        _print(f"  ✗ WebEngine 起不来（{exc}）")
        _print("    浮层需要它来画辉光 / 粒子。装上即可：pip install PyQt6-WebEngine")
        return False


def main(argv: list[str] | None = None) -> int:
    _fix_stdio()
    args = _parse_args(argv)
    if args.debug:
        from app import log
        log.enable()

    from app import __version__, log, paths, selfcheck
    from app.config import ConfigStore

    log_file = log.init_file(args.log_file or _default_log_file())
    if log_file:
        log.info(f"日志文件：{log_file}")

    store = ConfigStore(path=args.config) if args.config else ConfigStore()
    if args.config is None:
        store.load_or_default()

    # ---------------------------------------------------------------- 自检
    checks = selfcheck.run_checks(config_error=store.last_error)
    _print(f"  VisualSpectrum {__version__}  ·  {paths.session_type()}  ·  Python "
           f"{sys.version_info.major}.{sys.version_info.minor}")
    report = selfcheck.format_report(checks)
    _print(report)
    _print()
    log.debug("自检报告：\n" + report)

    if args.selfcheck:
        return 0 if selfcheck.summary(checks)["ok"] else 1

    # ---------------------------------------------------------------- 浮层渲染必需 WebEngine
    # 高刷：配置里的 `render.high_fps` 或 CLI `--high-fps`。
    # ⚠ Chromium 参数是**启动时读一次**的，所以改了这一项要重启才生效。
    from app.config import render_of
    high_fps = bool(render_of(store.data).get("high_fps")) or args.high_fps
    if not _prepare_webengine(high_fps=high_fps):
        return 1
    _print("  浮层渲染：QtWebEngine（Canvas）"
           + (" · 已解锁帧率上限（高刷）" if high_fps else ""))

    # ---------------------------------------------------------------- Qt
    from PyQt6.QtGui import QSurfaceFormat
    from PyQt6.QtWidgets import QApplication

    # ⚠ **必须在创建任何窗口之前**把默认表面格式设成带 alpha 的。
    #    浮层是"整屏透明、只画一圈频谱"，表面没有 alpha 的话没画到的地方不会透出去。
    surface = QSurfaceFormat()
    surface.setAlphaBufferSize(8)
    QSurfaceFormat.setDefaultFormat(surface)

    app = QApplication(sys.argv[:1])
    app.setApplicationName("VisualSpectrum")
    app.setQuitOnLastWindowClosed(False)          # 浮层关掉不等于退出程序

    from app.tray import make_icon
    app.setWindowIcon(make_icon())

    # ---------------------------------------------------------------- 单实例
    from app.single_instance import SingleInstance

    #: 配置页**实际**监听的端口；`ready` = 配置页曾经起来过（之后靠 `_ensure_config_server` 保活）
    config_port = {"port": args.port, "ready": False}
    #: 给"第二个实例请求打开配置页"那个回调用的控制器占位（真正的 controller 稍后才建）
    runtime: dict = {"controller": None}
    tray_ref: list = []

    def _open_config_page(*, notify: bool = False) -> None:
        """打开配置页 —— 顺手确保它还在监听（线程挂了就地重启）；唤起不了就提示手动访问。"""
        port = _ensure_config_server(runtime["controller"], config_port)
        if port is None:
            _print("  ! 配置页起不来 —— 请查看日志")
            return
        if not _open_config(port):
            _print(f"  ! 没能唤起浏览器 —— 请手动打开 http://127.0.0.1:{port}/")
        if notify and tray_ref:
            tray_ref[0].notify("VisualSpectrum", "已经在运行 —— 已为你打开配置页")

    def _second_launch(_payload: bytes) -> None:
        _print("  · 又有人启动了 VisualSpectrum —— 同时只允许一个实例")
        _open_config_page(notify=True)

    single = SingleInstance()
    if not single.acquire(on_message=_second_launch):
        _print("  ! VisualSpectrum 已经在运行 —— 这次不再起第二个。")
        # 兜底：本地套接字那条路万一没打通，就自己直接再开一次配置页
        #（第一个实例的服务器就监听在这个默认端口上）。
        if _port_listening("127.0.0.1", args.port) and not _open_config(args.port):
            _print(f"  ! 没能唤起浏览器 —— 请手动打开 http://127.0.0.1:{args.port}/")
        return 0

    # ---------------------------------------------------------------- 浮层 + 音频
    from app.controller import OverlayController

    controller = OverlayController(store)
    runtime["controller"] = controller
    controller.start()

    # ---------------------------------------------------------------- 托盘
    from app import tray as tray_mod

    tray = tray_mod.Tray(status=controller.status_text())
    tray_ref.append(tray)
    tray.openConfigRequested.connect(lambda: _open_config_page())
    tray.showOverlayRequested.connect(controller.show_overlay)      # 点图标 = 只显示（不隐藏）
    tray.toggleOverlayRequested.connect(controller.toggle_overlay)  # 菜单勾选框 = 切换
    tray.reloadRequested.connect(lambda: controller.reload(reason="托盘"))
    tray.quitRequested.connect(app.quit)
    controller.statusChanged.connect(tray.set_status)
    controller.overlayVisibilityChanged.connect(tray.set_overlay_visible)
    tray.set_overlay_visible(controller.overlay_visible())

    def _on_toast(message: str) -> None:
        _print(f"  ! {message}")
        tray.notify("VisualSpectrum", message)

    controller.toast.connect(_on_toast)
    if args.no_tray or not tray.show():
        reason = "（--no-tray）" if args.no_tray else "（当前环境没有托盘宿主）"
        _print(f"  ! 未显示系统托盘 {reason} —— 浮层照常工作。")
    else:
        _print("  ✓ 托盘已就绪（打开配置页 / 重载 / 退出）")

    # ---------------------------------------------------------------- 配置页
    if not args.no_config_server:
        chosen = _start_config_server(controller, port=args.port)
        if chosen is not None:
            config_port["port"] = chosen
            config_port["ready"] = True

    # ---------------------------------------------------------------- 收尾
    def _bye(*_args) -> None:
        _print("\n  正在退出…")
        controller.stop()
        app.quit()

    app.aboutToQuit.connect(controller.stop)

    from PyQt6.QtCore import QTimer
    signal.signal(signal.SIGINT, lambda *_: _bye())
    if hasattr(signal, "SIGTERM"):
        signal.signal(signal.SIGTERM, lambda *_: _bye())
    idle = QTimer()
    idle.start(200)
    idle.timeout.connect(lambda: None)

    _print(f"  {controller.status_text()}")
    _print("  Ctrl+C 退出\n")
    return app.exec()


def _default_log_file() -> Path:
    """日志默认写哪儿：源码运行 → 仓库 `logs/`；打包后 → 用户数据目录。"""
    from app import paths

    if paths.is_frozen():
        return paths.log_path()
    return paths.resource_dir() / "logs" / "visualspectrum.log"


def _port_free(host: str, port: int) -> bool:
    """端口是不是空的（先自己试绑一下 —— 比等 werkzeug 抛英文报错友好得多）。"""
    import socket

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            probe.bind((host, port))
        except OSError:
            return False
    return True


def _port_listening(host: str, port: int) -> bool:
    """端口是不是**已经有东西在监听**（用来判断配置页还活着没，好决定要不要重启它）。"""
    import socket

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.35)
        try:
            return probe.connect_ex((host, port)) == 0
        except OSError:
            return False


def _ensure_config_server(controller, state: dict) -> int | None:
    """确保配置页在监听：端口没人听了就（重新）起一个。返回监听端口（起不来则 `None`）。

    为什么不只"起一次"就完事：配置页跑在一个后台线程里，容器/桌面环境可能在某个时刻
    把它收掉。用户看到的现像就是"关了配置页窗口后就再也打不开，得重启程序"。
    这里每次要打开前先探一下端口，没人听就补一个，于是**永远能打开**。
    """
    host = "127.0.0.1"
    if state.get("ready") and _port_listening(host, int(state["port"])):
        return int(state["port"])
    chosen = _start_config_server(controller, port=int(state.get("port", 8770)))
    if chosen is not None:
        state["port"] = chosen
        state["ready"] = True
    return chosen


def _start_config_server(controller, *, port: int) -> int | None:
    """在后台线程里起配置页（Flask）。起不来就只提示，不影响浮层。

    返回**实际**监听的端口（默认端口被占时会往后挪）；起不来则返回 `None`。
    """
    try:
        from backend.app import create_app
    except Exception as exc:                     # noqa: BLE001
        _print(f"  ! 配置页没起来（{exc}）—— 只跑浮层。")
        return None

    host = "127.0.0.1"
    chosen = port
    while chosen < port + 10 and not _port_free(host, chosen):
        chosen += 1
    if chosen >= port + 10:
        _print(f"  ! 配置页端口 {port}~{port + 9} 都被占了 —— 跳过配置页。")
        return None

    def _serve() -> None:
        app = create_app(controller)
        app.run(host=host, port=chosen, debug=False, use_reloader=False, threaded=True)

    threading.Thread(target=_serve, name="visualspectrum-config", daemon=True).start()
    _print(f"  ✓ 配置页: http://{host}:{chosen}/   （改完保存即生效，不必重启）")
    from app import log
    log.debug(f"配置页监听 {host}:{chosen}")
    return chosen


def _open_config(port: int) -> bool:
    """用系统浏览器打开配置页。返回**是否成功唤起**（失败调用方应提示手动访问 URL）。

    唤起浏览器这条链在 Linux 上不一定可靠（`webbrowser` 可能挑到一个不可用的后端），
    所以这里两道：先 `webbrowser`，不行再用系统命令兜底；每次都把 URL 记进日志。
    """
    url = f"http://127.0.0.1:{port}/"
    from app import log
    log.info(f"打开配置页：{url}")
    if _open_with_webbrowser(url):
        return True
    return _open_with_platform(url)


def _open_with_webbrowser(url: str) -> bool:
    """先用 Python 标准的 `webbrowser` —— **每次开新标签**，避开复用已退出控制器句柄的毛病。"""
    try:
        import webbrowser

        try:
            return bool(webbrowser.open_new_tab(url))
        except Exception:                            # noqa: BLE001 - 退回老 API 再试一次
            return bool(webbrowser.open(url))
    except Exception:                                # noqa: BLE001
        return False


def _open_with_platform(url: str) -> bool:
    """`webbrowser` 失灵时的兜底：手边常见的系统"打开 URL"命令。"""
    import subprocess
    import sys

    if sys.platform.startswith("win"):
        candidates = [["cmd", "/c", "start", "", url]]
    elif sys.platform == "darwin":
        candidates = [["open", url]]
    else:
        candidates = [["xdg-open", url], ["gio", "open", url], ["sensible-browser", url]]
    for cmd in candidates:
        try:
            subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             start_new_session=True)
            return True
        except (OSError, ValueError):
            continue
    return False


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(0)
