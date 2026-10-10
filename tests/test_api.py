"""REST 接口：meta / config / validate / reset / spectrum。"""

from __future__ import annotations

import pytest

flask = pytest.importorskip("flask")

from app.config import SIZE_RANGE, ConfigStore  # noqa: E402
from backend.app import create_app  # noqa: E402


@pytest.fixture()
def client(tmp_path):
    store = ConfigStore(path=tmp_path / "config.json")
    store.load_or_default()
    app = create_app(store=store)       # 不给 controller —— 配置页独立运行
    app.config["TESTING"] = True
    with app.test_client() as test_client:
        yield test_client, store


def test_meta(client) -> None:
    test_client, _store = client
    payload = test_client.get("/api/meta").get_json()
    assert payload["ok"] is True
    assert payload["running"] is False
    assert payload["overlay_visible"] is None       # 没控制器 → 不知道 / 没有浮层
    assert payload["mode"] in ("top", "wallpaper")
    assert payload["limits"]["size"] == list(SIZE_RANGE)


def test_get_config(client) -> None:
    test_client, _store = client
    payload = test_client.get("/api/config").get_json()
    assert payload["ok"] is True
    assert payload["config"]["version"] == 1
    assert payload["config"]["appearance"]["count"] > 0


def test_put_config_applies_and_rejects_bad(client) -> None:
    test_client, store = client
    good = {"version": 1, "appearance": {"size": 42, "count": 32, "mode": "wallpaper"}}
    payload = test_client.put("/api/config", json=good).get_json()
    assert payload["ok"] is True
    assert store.data["appearance"]["size"] == 42

    bad = {"version": 1, "appearance": {"size": 500}}
    response = test_client.put("/api/config", json=bad)
    assert response.status_code == 400
    assert response.get_json()["ok"] is False
    assert store.data["appearance"]["size"] == 42     # 坏配置没落盘


def test_validate_endpoint(client) -> None:
    test_client, _store = client
    ok = test_client.post("/api/config/validate", json={"appearance": {"mode": "top"}})
    assert ok.get_json()["ok"] is True
    bad = test_client.post("/api/config/validate", json={"appearance": {"mode": "nope"}})
    assert bad.status_code == 400


def test_reset(client) -> None:
    test_client, store = client
    test_client.put("/api/config", json={"appearance": {"size": 15, "count": 200}})
    payload = test_client.post("/api/config/reset").get_json()
    assert payload["ok"] is True
    assert store.data["appearance"]["size"] == 55


def test_spectrum_without_controller(client) -> None:
    test_client, _store = client
    payload = test_client.get("/api/spectrum").get_json()
    assert payload["ok"] is True
    assert payload["bands"] == []
    assert payload["live"] is False


def test_audio_devices_endpoint(client) -> None:
    test_client, _store = client
    payload = test_client.get("/api/audio/devices").get_json()
    assert payload["ok"] is True
    assert isinstance(payload["devices"], list)
    assert payload["current"] == "auto"


class _FakeController:
    """替身：只记账，验证保存后走的是 `request_reload`（不是直接 `reload`）。"""

    def __init__(self, store):
        self.store = store
        self.requested = []
        self.overlay_requests = []
        self.visible = True

    def request_reload(self, reason=""):
        self.requested.append(reason)

    def reload(self, reason=""):        # 不应该被调到
        raise AssertionError("不应在 Flask 线程里直接 reload（会卡住浮层）")

    def latest_spectrum(self):
        return {"bands": [0.5], "live": False, "source": "fake", "running": True}

    def request_overlay(self, visible=None):
        self.overlay_requests.append(visible)      # 只记账，真正的 show/hide 在主线程

    def overlay_visible(self):
        return self.visible


def test_save_marshals_reload_through_the_controller(tmp_path) -> None:
    store = ConfigStore(path=tmp_path / "config.json")
    store.load_or_default()
    controller = _FakeController(store)
    app = create_app(controller)
    app.config["TESTING"] = True
    with app.test_client() as test_client:
        payload = test_client.put("/api/config",
                                  json={"appearance": {"size": 33}}).get_json()
    assert payload["ok"] is True
    assert payload["applied"] is True
    assert controller.requested == ["配置页保存"]


def test_overlay_endpoint_requests_through_controller(tmp_path) -> None:
    store = ConfigStore(path=tmp_path / "config.json")
    store.load_or_default()
    controller = _FakeController(store)
    app = create_app(controller)
    app.config["TESTING"] = True
    with app.test_client() as test_client:
        shown = test_client.post("/api/overlay", json={"visible": True}).get_json()
        assert shown["ok"] is True and shown["requested"] is True
        toggled = test_client.post("/api/overlay", json={}).get_json()   # 不传 = 切换
        assert toggled["ok"] is True and toggled["requested"] is None
        bad = test_client.post("/api/overlay", json={"visible": "yes"})
        assert bad.status_code == 400
        meta = test_client.get("/api/meta").get_json()
        assert meta["overlay_visible"] is True
    # 全部只走 request_overlay（排队到主线程），没有在 Flask 线程里直接动 Qt
    assert controller.overlay_requests == [True, None]


def test_overlay_endpoint_without_controller(client) -> None:
    test_client, _store = client
    payload = test_client.post("/api/overlay", json={"visible": False}).get_json()
    assert payload["ok"] is True
    assert payload["supported"] is False
