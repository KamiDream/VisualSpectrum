"""VisualSpectrum —— 桌面环形频谱可视化（壁纸 / 顶层两种模式）。

它是一个常驻后台的小工具：抓取**系统正在播放的声音**（回环捕获），做一次 FFT，
把频段能量画成一圈**随着音乐起伏的环形频谱**——带 CRT 显示器那种荧光辉光，
还有从频谱条顶端逸出的散射粒子。

设置很少（见 [`app/config.py`](app/config.py)）：

* **大小** `appearance.size` —— 环占屏幕短边的百分比；
* **数量** `appearance.count` —— 环上频谱块的个数；
* **条高** `appearance.height` —— 频谱条最长伸到环半径的百分之多少；
* **强度 / 灵敏度** `appearance.intensity` —— 整体电平放大倍数；
* **连线** `appearance.link` —— 是否把各条顶端的中心点用细线相连；
* **颜色** `appearance.colors` —— 按频率低→高的一段色标（每个频率一个颜色）；
* **旋转** `appearance.rotate` —— 整圈是否缓慢自转；
* **模式** `appearance.mode` —— `top`（浮在最上层、鼠标穿透）还是
  `wallpaper`（贴到桌面壁纸层、被所有窗口盖住）。

渲染走 `QtWebEngine`（HTML5 Canvas）—— 因为整套视觉（辉光 / 粒子 / 音乐元素）
本来就是为 Canvas 写的，配置页的实时预览与真实浮层**共用同一份渲染代码**。
"""

from __future__ import annotations

#: 程序版本（自检 / `/api/meta` / 打包都会报它）。
__version__ = "0.1.0"

#: 程序名（路径、托盘、窗口标题都用它）。
APP_NAME = "VisualSpectrum"

__all__ = ["__version__", "APP_NAME"]
