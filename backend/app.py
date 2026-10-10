"""VisualSpectrum —— 配置页的 Flask 应用组装。

职责划分：

    前端（web/ 下的原生 HTML/CSS/ES 模块）只负责显示与交互，全部通过 /api/* 拿数据；
    后端（backend/）只负责配置这一件事，以及托管静态资源。

两个刻意的设计：

* **无构建步骤**：`web/` 下的文件由 Flask 直接托管，浏览器按 `import` 关系加载 ——
  改完刷新即可，也完全离线可用；
* **与应用同进程**：`create_app(controller)` 把桌面端的 `ConfigStore` 与 controller 传进来，
  于是"保存配置"可以直接让浮层生效，实时预览也能拿到**真实**的频谱数据。
  不给 controller 也能跑（`create_app()`）：那样配置页就是个独立的编辑器，接口照常可用。
"""

from __future__ import annotations

from dataclasses import dataclass

from flask import Flask, jsonify, render_template, request

from app import __version__, paths
from app.config import ConfigStore

__all__ = ["AppContext", "create_app"]


@dataclass
class AppContext:
    """配置页要用到的那点东西（刻意很薄：只有 store 与可选的 controller）。"""

    store: ConfigStore
    controller: object | None = None


def create_app(controller=None, *, store: ConfigStore | None = None) -> Flask:
    """组装 Flask 应用。`controller` 可以不给（见模块注释）。"""
    app = Flask(__name__,
                template_folder=str(paths.templates_dir()),
                static_folder=str(paths.static_dir()),
                static_url_path="/static")
    app.json.ensure_ascii = False                  # 中文别被转成 \uXXXX
    app.config["SEND_FILE_MAX_AGE_DEFAULT"] = 0    # 前端没有构建产物，别缓存

    if controller is not None and getattr(controller, "store", None) is not None:
        app.extensions["visualspectrum"] = AppContext(store=controller.store, controller=controller)
    else:
        app.extensions["visualspectrum"] = AppContext(store=store or ConfigStore(), controller=None)

    from .api.config_api import bp as config_bp
    app.register_blueprint(config_bp)

    @app.get("/")
    def index():
        if not (paths.templates_dir() / "index.html").is_file():
            return _placeholder(), 200
        return render_template("index.html")

    @app.errorhandler(404)
    def not_found(_error):
        if request.path.startswith("/api/"):
            return jsonify({"ok": False, "error": f"接口不存在：{request.path}"}), 404
        return _placeholder(), 404

    @app.errorhandler(500)
    def server_error(error):                      # pragma: no cover
        return jsonify({"ok": False, "error": f"服务器内部错误：{error}"}), 500

    app.config["VISUALSPECTRUM_VERSION"] = __version__
    return app


def _placeholder() -> str:
    """前端资源缺失时的兜底页面 —— 比一个白屏 + 404 有用得多。"""
    return (
        "<!doctype html><meta charset=\"utf-8\"><title>VisualSpectrum 配置页</title>"
        "<body style=\"font:14px/1.7 system-ui;max-width:720px;margin:60px auto;padding:0 16px\">"
        "<h1>VisualSpectrum 配置页</h1>"
        "<p>后端接口是通的（可以试试 <code>/api/meta</code>、<code>/api/config</code>），"
        "但前端文件还没放好：</p>"
        f"<pre>{paths.templates_dir()}</pre>"
        "<p>把 <code>web/templates/index.html</code> 与 <code>web/static/</code> 放好即可。</p>"
        "</body>"
    )
