"""配置页的 REST 接口。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/meta` | 版本、平台、会话、自检结果、配置路径、运行状态 |
| GET | `/api/config` | 取当前配置（**已归一化**）+ 指纹 |
| PUT | `/api/config` | 全量保存；成功后**立刻让浮层生效**（不用重启） |
| POST | `/api/config/validate` | 只校验不保存（前端边改边提示） |
| POST | `/api/config/reset` | 恢复默认配置 |
| GET | `/api/config/export` | 下载配置 JSON |
| GET | `/api/spectrum` | **实时预览**：当前这一帧频段 + 运行状态 |
| GET | `/api/audio/devices` | 可捕获的设备（回环 / monitor），给配置页的下拉用 |
| POST | `/api/reload` | 让桌面端重新读配置 |

两个要点：

* **所有写操作都走 `ConfigStore`**（校验 → 归一化 → 原子写），不存在"绕过校验存进坏配置"；
* 保存成功后主动调 `controller.reload() / _apply()`，让正在运行的浮层**当场**用上新配置。
"""

from __future__ import annotations

from flask import Blueprint, Response, current_app, jsonify, request

from app import __version__, audio, paths, selfcheck
from app.config import (
    AUDIO_SOURCES, BG_OPACITY_RANGE, COUNT_RANGE, DELAY_RANGE, HEIGHT_RANGE, INTENSITY_RANGE,
    MODES, SIZE_RANGE,
    ConfigError, appearance_of, audio_of, default_config, validate,
)

bp = Blueprint("config_api", __name__, url_prefix="/api")


# --------------------------------------------------------------------------
# 上下文
# --------------------------------------------------------------------------

def _context():
    return current_app.extensions["visualspectrum"]


def _ok(**payload):
    return jsonify({"ok": True, **payload})


def _fail(message: str, status: int = 400):
    return jsonify({"ok": False, "error": message}), status


# --------------------------------------------------------------------------
# meta
# --------------------------------------------------------------------------

@bp.get("/meta")
def meta():
    """配置页首屏要的一切：环境状态 + 自检 + 配置位置 + 可调范围。"""
    ctx = _context()
    checks = selfcheck.run_checks(config_error=ctx.store.last_error)

    spectrum = None
    if ctx.controller is not None and hasattr(ctx.controller, "latest_spectrum"):
        spectrum = ctx.controller.latest_spectrum()

    return _ok(
        version=__version__,
        platform=paths.describe(),
        config_path=str(ctx.store.path),
        revision=ctx.store.revision,
        fingerprint=ctx.store.fingerprint(),
        running=ctx.controller is not None,
        mode=str(appearance_of(ctx.store.data).get("mode", "top")),
        live=bool(spectrum["live"]) if spectrum else False,
        overlay_visible=(bool(ctx.controller.overlay_visible())
                         if ctx.controller is not None
                         and hasattr(ctx.controller, "overlay_visible") else None),
        source=str(spectrum["source"]) if spectrum else "（配置页独立运行）",
        limits={"size": list(SIZE_RANGE), "count": list(COUNT_RANGE),
                "height": list(HEIGHT_RANGE), "intensity": list(INTENSITY_RANGE),
                "bg_opacity": list(BG_OPACITY_RANGE),
                "modes": list(MODES),
                "audio_sources": list(AUDIO_SOURCES), "delay": list(DELAY_RANGE)},
        selfcheck=selfcheck.summary(checks),
    )


# --------------------------------------------------------------------------
# 频谱（实时预览用）
# --------------------------------------------------------------------------

@bp.get("/spectrum")
def spectrum():
    """给配置页预览的**真实**频谱帧。

    配置页独立运行（没接控制器）时返回空帧 + `live=false`，前端自己退回合成的演示波形。
    """
    ctx = _context()
    if ctx.controller is None or not hasattr(ctx.controller, "latest_spectrum"):
        return _ok(bands=[], live=False, source="（配置页独立运行）", running=False)
    return _ok(**ctx.controller.latest_spectrum())


# --------------------------------------------------------------------------
# 浮层显示 / 隐藏
# --------------------------------------------------------------------------

@bp.post("/overlay")
def overlay_control():
    """显示 / 隐藏桌面浮层 —— 也就是"把可视化关了之后怎么再唤回来"那条路。

    body 可选：`{"visible": true}` 显示、`{"visible": false}` 隐藏；不给 body 就**切换**。
    ⚠ 与保存一样，只发信号（`request_overlay`），真正的 show/hide 排到主线程做 ——
      在 Flask 线程里直接 show/hide Qt 窗口有风险。
    """
    ctx = _context()
    payload = request.get_json(silent=True) or {}
    visible = payload.get("visible", None)
    if visible is not None and not isinstance(visible, bool):
        return _fail("visible 需要是 true / false（不传则切换）")
    if ctx.controller is None:
        return _ok(supported=False, note="配置页独立运行，没有浮层可控制")
    request_overlay = getattr(ctx.controller, "request_overlay", None)
    if request_overlay is None:
        return _fail("当前版本不支持远程控制浮层", 501)
    request_overlay(visible)
    return _ok(supported=True, requested=visible)


# --------------------------------------------------------------------------
# 音频设备
# --------------------------------------------------------------------------

@bp.get("/audio/devices")
def audio_devices():
    """可捕获的设备（回环 / monitor）—— 配置页的下拉用。

    枚举放到**请求里现做**（而不是缓存）：音频设备随时可能插拔/改名。
    一个后端都不可用就返回空列表，前端保留「自动」那一项。
    """
    ctx = _context()
    current = str(audio_of(ctx.store.data).get("device", "auto"))
    try:
        devices = audio.list_devices()
    except Exception as exc:                      # noqa: BLE001 - 枚举失败不该 500
        return _ok(devices=[], current=current, error=str(exc))
    return _ok(devices=devices, current=current)


# --------------------------------------------------------------------------
# 配置
# --------------------------------------------------------------------------

@bp.get("/config")
def get_config():
    ctx = _context()
    return _ok(config=ctx.store.data,
               fingerprint=ctx.store.fingerprint(),
               revision=ctx.store.revision,
               path=str(ctx.store.path),
               last_error=ctx.store.last_error)


@bp.put("/config")
def put_config():
    """全量保存。校验失败**不动磁盘**；成功则立刻让浮层生效。"""
    ctx = _context()
    payload = request.get_json(silent=True)
    if payload is None:
        return _fail("请求体不是合法 JSON")

    try:
        saved = ctx.store.save(payload)
    except ConfigError as exc:
        return _fail(str(exc))

    applied, note = _apply(ctx)
    return _ok(config=saved, revision=ctx.store.revision,
               fingerprint=ctx.store.fingerprint(), applied=applied, note=note)


@bp.post("/config/validate")
def validate_config():
    """只校验不保存 —— 前端可以边改边喊，拿到的报错与真存时**完全一致**。"""
    payload = request.get_json(silent=True)
    if payload is None:
        return _fail("请求体不是合法 JSON")
    try:
        clean = validate(payload)
    except ConfigError as exc:
        return _fail(str(exc))
    return _ok(config=clean)


@bp.post("/config/reset")
def reset_config():
    ctx = _context()
    try:
        saved = ctx.store.save(default_config())
    except ConfigError as exc:                    # 默认配置理论上一定合法
        return _fail(str(exc), 500)
    applied, note = _apply(ctx)
    return _ok(config=saved, revision=ctx.store.revision, applied=applied, note=note)


@bp.get("/config/export")
def export_config():
    """导出配置（浏览器会当成下载）。"""
    import json
    ctx = _context()
    body = json.dumps(ctx.store.data, ensure_ascii=False, indent=2) + "\n"
    return Response(body, mimetype="application/json",
                    headers={"Content-Disposition":
                             'attachment; filename="visualspectrum-config.json"'})


@bp.post("/reload")
def reload_config():
    ctx = _context()
    applied, note = _apply(ctx)
    return _ok(applied=applied, note=note, revision=ctx.store.revision)


# --------------------------------------------------------------------------
# 应用
# --------------------------------------------------------------------------

def _apply(ctx) -> tuple[bool, str]:
    """保存后让桌面端当场用上新配置。没有桌面端就只回一句说明。

    ⚠ **不能直接调 `controller.reload()`**：这段代码跑在 Flask 的**后台线程**里，
      而重载过程会碰 Qt / WebEngine 对象（只能在主线程碰）。所以走 `request_reload()`
      —— 它只发个信号，真正的重载由主线程执行（这就是以前"一保存音乐就不动了"的根因）。
    """
    controller = ctx.controller
    if controller is None:
        return False, "配置页是独立运行的，没有浮层在跑 —— 已保存，下次启动生效"
    try:
        request = getattr(controller, "request_reload", None)
        if request is not None:
            request("配置页保存")
        else:                                      # 兜底：对象没有那个方法时直接重载
            controller.reload("配置页保存")
    except Exception as exc:                      # noqa: BLE001
        return False, f"已保存，但让浮层生效时出错：{exc}"
    return True, "已保存并生效"
