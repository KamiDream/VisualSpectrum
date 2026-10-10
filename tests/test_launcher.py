"""启动器（`run.py`）里那几个"能反复打开配置页"的小工具。

不真的起浏览器 / Flask —— 只钉住逻辑：端口探活、配置页保活重启、唤起失败时不抛异常。
背景：用户报过"把配置页窗口关了之后就再也打不开，得重启程序"，于是打开动作改成
**每次先探活、没有就补一个服务器，再用浏览器/系统命令兜底唤起**。
"""

from __future__ import annotations

import socket

import run


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def test_port_listening_false_when_nobody_listens() -> None:
    assert run._port_listening("127.0.0.1", _free_port()) is False


def test_port_listening_true_when_listening() -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as server:
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("127.0.0.1", 0))
        server.listen(1)
        port = server.getsockname()[1]
        assert run._port_listening("127.0.0.1", port) is True


def test_ensure_config_server_reuses_live_server(monkeypatch) -> None:
    """端口还有人听 → 直接复用，**不要**再起第二个服务器。"""
    monkeypatch.setattr(run, "_port_listening", lambda host, port: True)

    def _boom(*_args, **_kwargs):
        raise AssertionError("服务器还活着时不该重启")

    monkeypatch.setattr(run, "_start_config_server", _boom)
    state = {"port": 8770, "ready": True}
    assert run._ensure_config_server(object(), state) == 8770


def test_ensure_config_server_restarts_when_down(monkeypatch) -> None:
    """端口没人听（配置页线程挂了）→ 就地重启，并更新端口 / ready。"""
    monkeypatch.setattr(run, "_port_listening", lambda host, port: False)
    monkeypatch.setattr(run, "_start_config_server", lambda controller, *, port: 8888)
    state = {"port": 8770, "ready": True}
    assert run._ensure_config_server(object(), state) == 8888
    assert state == {"port": 8888, "ready": True}


def test_ensure_config_server_reports_failure(monkeypatch) -> None:
    monkeypatch.setattr(run, "_port_listening", lambda host, port: False)
    monkeypatch.setattr(run, "_start_config_server", lambda controller, *, port: None)
    state = {"port": 8770, "ready": False}
    assert run._ensure_config_server(object(), state) is None


def test_open_config_prefers_webbrowser(monkeypatch) -> None:
    monkeypatch.setattr(run, "_open_with_webbrowser", lambda url: True)
    monkeypatch.setattr(run, "_open_with_platform", lambda url: True)
    assert run._open_config(8770) is True


def test_open_config_falls_back_then_gives_up(monkeypatch) -> None:
    monkeypatch.setattr(run, "_open_with_webbrowser", lambda url: False)
    seen = {}

    def _platform(url: str) -> bool:
        seen["url"] = url
        return False

    monkeypatch.setattr(run, "_open_with_platform", _platform)
    assert run._open_config(8770) is False
    assert seen["url"] == "http://127.0.0.1:8770/"    # 兜底也拿到的是同一个 URL


def test_open_config_trues_when_platform_succeeds(monkeypatch) -> None:
    monkeypatch.setattr(run, "_open_with_webbrowser", lambda url: False)
    monkeypatch.setattr(run, "_open_with_platform", lambda url: True)
    assert run._open_config(8770) is True
