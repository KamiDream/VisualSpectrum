# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller 打包配置（手写）。

    pip install pyinstaller
    pyinstaller VisualSpectrum.spec

要点：

* **只读资源要一起带上**：`web/`（前端）与 `assets/`（图标）—— 它们靠 `app/paths.py`
  的 `resource_dir()` 在打包后从 `sys._MEIPASS` 找；
* `hiddenimports` 里点名 `PyQt6.QtWebEngineWidgets` / `QtWebEngineCore` —— 它们是被
  惰性导入的，PyInstaller 的静态分析扫不到；
* `excludes` 掉一堆用不到的大件（tkinter / 测试框架 / 另一个 Qt 绑定），能把体积压下来；
* 运行时产物（配置 / 日志）落在**用户数据目录**，不在这个包目录里（见 `app/paths.py`）。
"""

from PyInstaller.utils.hooks import collect_data_files, collect_submodules

datas = [("web", "web"), ("assets", "assets")]
datas += collect_data_files("soundcard")

hiddenimports = [
    "PyQt6.QtWebEngineWidgets",
    "PyQt6.QtWebEngineCore",
    "PyQt6.QtNetwork",
] + collect_submodules("soundcard")

excludes = [
    "tkinter", "unittest", "pytest", "PyQt5", "PySide2", "PySide6",
    "matplotlib", "pandas", "scipy",
]

a = Analysis(
    ["run.py"],
    pathex=[],
    binaries=[],
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=[],
    runtime_hooks=[],
    excludes=excludes,
    noarchive=False,
)
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name="VisualSpectrum",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=False,
    disable_windowed_traceback=False,
    icon=None,                      # 想嵌入图标就放一个 assets/icon.ico 再把这里改成它
)
coll = COLLECT(
    exe,
    a.binaries,
    a.datas,
    strip=False,
    upx=False,
    name="VisualSpectrum",
)
