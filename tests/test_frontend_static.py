"""前端静态结构 —— 钉住"浮层与预览共用同一份渲染代码"这类容易走样的约定。

不做真渲染，只查关键文件在不在、引用对不对（快、且不需要浏览器）。
"""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "web"


def _read(relative: str) -> str:
    return (WEB / relative).read_text(encoding="utf-8")


def test_overlay_page_wires_renderer_and_styles() -> None:
    html = _read("overlay.html")
    assert "static/js/overlay.js" in html
    assert "static/css/overlay.css" in html
    assert "spectrum" in html                       # 画布


def test_overlay_and_preview_share_one_renderer() -> None:
    overlay = _read("static/js/overlay.js")
    preview = _read("static/js/lib/preview.js")
    assert "./lib/spectrum.js" in overlay
    assert "./spectrum.js" in preview
    # 共用的那个类必须真的导出了
    assert "export class SpectrumView" in _read("static/js/lib/spectrum.js")


def test_config_page_wires_app() -> None:
    html = _read("templates/index.html")
    assert "/static/js/main.js" in html
    assert "/static/css/base.css" in html
    assert "preview-canvas" in html


def test_config_page_has_the_settings() -> None:
    html = _read("templates/index.html")
    for control in ('id="size"', 'id="count"', 'id="height"', 'id="intensity"',
                    'id="bass"', 'id="mid"', 'id="treble"',
                    'id="rotate"', 'id="link"', 'id="colors"', 'id="add-color"',
                    'id="bg-color"', 'id="bg-opacity"', 'id="crt"', 'id="afterglow"',
                    'id="barrel"', 'id="aberration"',
                    'id="particle-speed"', 'id="particle-life"', 'id="burst"',
                    'id="burst-sensitivity"', 'id="burst-speed"', 'id="burst-life"',
                    'id="high-fps"', 'id="btn-overlay"',
                    'id="delay"', 'id="device"', 'id="mix-readout"',
                    'id="settings-tabs"',
                    'data-tab="display"', 'data-tab="color"', 'data-tab="motion"',
                    'data-tab="bg"', 'data-tab="audio"', 'data-tab="window"',
                    'data-panel="display"', 'data-panel="color"', 'data-panel="motion"',
                    'data-panel="bg"', 'data-panel="audio"', 'data-panel="window"',
                    'data-source="system"', 'data-source="off"',
                    'data-mode="top"', 'data-mode="wallpaper"'):
        assert control in html, control


def test_spectrum_view_uses_cheap_bloom_not_shadowblur() -> None:
    source = _read("static/js/lib/spectrum.js")
    assert ".shadowBlur" not in source             # 刻意不用它（会掉帧）
    assert "globalCompositeOperation" in source     # 用叠加混合做辉光


def test_html_comments_are_balanced() -> None:
    """HTML 注释要成对 —— 多一个 `-->` 就会在页面左上角渲染成 `-->` 文本（曾经发生过）。"""
    for rel in ("overlay.html", "templates/index.html"):
        text = _read(rel)
        assert text.count("<!--") == text.count("-->"), rel
        assert "-->。" not in text, rel
