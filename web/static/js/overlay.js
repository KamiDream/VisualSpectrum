/**
 * 浮层页面脚本 —— 把 Python 推来的数据喂给共用的渲染器 `lib/spectrum.js`。
 *
 * 这一层刻意做得很薄：不取音频、不做几何、不管动画 —— 只负责"收数据 → 转交渲染器"，
 * 以及把 CRT 的 CSS 特效（扫描线 / 暗角）挂上。
 */

import { SpectrumView, backgroundCss, crtOn } from './lib/spectrum.js'

const stage = document.getElementById('stage')
const canvas = document.getElementById('spectrum')

const view = new SpectrumView(canvas)

//: CRT 的扫描线 / 暗角是**静态覆盖层**（CSS 画的），挂在 stage 上，不占渲染成本。
const crt = document.createElement('div')
crt.className = 'crt'
stage.appendChild(crt)

function ensureRunning() {
  if (!view.running) view.start()
}

window.VisualSpectrum = {
  /** Python 侧判断"页面好了没"就看这个。 */
  ready: true,

  /** 三项设置 + 背景：背景色 / 不透明度写进 `--vs-bg`（见 overlay.css）。 */
  setAppearance(appearance) {
    view.setAppearance(appearance)
    const bg = backgroundCss(appearance)
    const root = document.documentElement
    if (bg) root.style.setProperty('--vs-bg', bg)
    else root.style.removeProperty('--vs-bg')
    // CRT 覆盖层：**有背景且开关开着**才显示（否则会看着像“背景色”）
    root.classList.toggle('crt-on', crtOn(appearance))
    ensureRunning()
  },

  /** 每一帧频段：`{bands: [...], live: bool, source: 名字}`。 */
  pushFrame(frame) {
    ensureRunning()
    view.pushFrame(frame)
  },

  /** 显示浮层（拿掉 idle）。 */
  open() {
    document.documentElement.classList.remove('idle')
    ensureRunning()
  },

  /** 隐藏浮层：**停掉渲染**（没有动画就没有合成，Chromium 退闲）。 */
  suspend() {
    document.documentElement.classList.add('idle')
    view.stop()
  },
}

// 页面热重载（开发时）或刚载入时先跑起来；真正显隐由 open()/suspend() 控制。
ensureRunning()
