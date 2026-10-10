/**
 * 配置页的实时预览：把浮层那套渲染器（`spectrum.js`）挂到配置页的一张 Canvas 上，
 * 并轮询 `/api/spectrum` 拿**真实**的频段数据（拿不到就退回合成的演示波形）。
 *
 * 这样"配置页看到的"和"桌面上浮层画的"是同一份渲染代码、同一份数据 —— 所见即所得。
 */

import { SpectrumView } from './spectrum.js'
import { api } from './api.js'

const POLL_MS = 60

//: 低 / 中 / 高音的分界（占频段数比例）—— 与 Python 侧 `audio.REGION_SPLIT` 一致。
const REGION_SPLIT = [0.34, 0.78]

/** 把一帧频段按低/中/高算成占比（三数之和 ≈ 1）。 */
function regionMix(bands) {
  const n = bands?.length ?? 0
  if (!n) return null
  const b1 = Math.max(1, Math.round(n * REGION_SPLIT[0]))
  const b2 = Math.min(n - 1, Math.max(b1 + 1, Math.round(n * REGION_SPLIT[1])))
  const avg = (lo, hi) => {
    if (hi <= lo) return 0
    let sum = 0
    for (let i = lo; i < hi; i += 1) sum += bands[i]
    return sum / (hi - lo)
  }
  const bass = avg(0, b1)
  const mid = avg(b1, b2)
  const treble = avg(b2, n)
  const total = bass + mid + treble
  if (total <= 1e-6) return { bass: 0, mid: 0, treble: 0 }
  return { bass: bass / total, mid: mid / total, treble: treble / total }
}

export function mountPreview(canvas, { onStatus } = {}) {
  const view = new SpectrumView(canvas)
  view.start()

  let stopped = false
  let timer = 0

  async function poll() {
    if (stopped) return
    try {
      const payload = await api.spectrum()
      view.pushFrame(payload)
      onStatus?.({
        live: Boolean(payload.live),
        source: String(payload.source ?? ''),
        running: Boolean(payload.running),
        mix: regionMix(payload.bands),
      })
    } catch (error) {
      onStatus?.({ live: false, source: `预览连不上后端：${error.message}`, running: false })
    }
    timer = window.setTimeout(poll, POLL_MS)
  }
  void poll()

  return {
    setAppearance: (appearance) => view.setAppearance(appearance),
    dispose() {
      stopped = true
      window.clearTimeout(timer)
      view.stop()
    },
  }
}
