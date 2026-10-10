"""环境自检 —— `run.py` 与 `/api/meta` 共用同一份逻辑。

要回答的是"这东西这台机器上跑得起来吗、缺什么"：

* 音频后端（soundcard / sounddevice / numpy）—— 决定了是抓真声音还是走 demo；
* WebEngine —— 浮层渲染靠它；
* 配置文件 —— 有没有、坏没坏。
"""

from __future__ import annotations

import importlib.util

from . import paths

__all__ = ["Check", "run_checks", "format_report", "summary"]


class Check:
    """一项体检结论。`ok` 为 `None` 表示"提示"，不算失败。"""

    def __init__(self, name: str, ok: bool | None, detail: str = "") -> None:
        self.name = name
        self.ok = ok
        self.detail = detail

    def render(self) -> str:
        mark = {True: "✓", False: "✗", None: "·"}[self.ok]
        text = f"{mark} {self.name}"
        return f"{text}：{self.detail}" if self.detail else text


def _has(module: str) -> bool:
    try:
        return importlib.util.find_spec(module) is not None
    except Exception:                            # noqa: BLE001
        return False


def run_checks(*, config_error: str | None = None) -> list[Check]:
    """跑一遍体检。顺序 = 展示顺序。"""
    checks: list[Check] = []

    checks.append(Check(f"平台 {paths.session_type()}", True,
                        f"Python {paths.describe()['python']}"))

    # 音频：真正决定"是真频谱还是 demo"
    have_numpy = _has("numpy")
    have_card = _has("soundcard")
    have_sd = _has("sounddevice")
    real_possible = have_numpy and (have_card or have_sd)
    audio_detail = (f"soundcard={'有' if have_card else '无'} · "
                    f"sounddevice={'有' if have_sd else '无'} · "
                    f"numpy={'有' if have_numpy else '无'}")
    if real_possible:
        checks.append(Check("音频回环", True, audio_detail))
    else:
        missing = []
        if not have_numpy:
            missing.append("numpy")
        if not (have_card or have_sd):
            missing.append("soundcard 或 sounddevice")
        checks.append(Check("音频回环（回落到 demo 合成频谱）", None,
                            f"缺 {' / '.join(missing)}；{audio_detail}"))

    # 渲染
    have_webengine = _has("PyQt6.QtWebEngineWidgets")
    checks.append(Check("浮层渲染（QtWebEngine）", have_webengine,
                        "可用" if have_webengine else "缺 PyQt6-WebEngine，浮层起不来"))

    # 配置
    store_path = paths.config_path()
    if config_error:
        checks.append(Check("配置文件", False, config_error))
    elif store_path.is_file():
        checks.append(Check("配置文件", True, str(store_path)))
    else:
        checks.append(Check("配置文件", None, f"{store_path}（启动时会生成一份默认的）"))

    return checks


def format_report(checks: list[Check]) -> str:
    return "\n".join("  " + check.render() for check in checks)


def summary(checks: list[Check]) -> dict:
    """给 `/api/meta` 用的小结。"""
    return {
        "ok": all(check.ok is not False for check in checks),
        "failed": [check.name for check in checks if check.ok is False],
        "notes": [check.name for check in checks if check.ok is None],
        "items": [{"name": c.name, "ok": c.ok, "detail": c.detail} for c in checks],
    }
