/**
 * 配置页入口。
 *
 * 就三件事：
 *   1. 拉 `/api/meta` + `/api/config`，把三项设置（大小 / 数量 / 模式）填进控件；
 *   2. 控件一变就更新内存里的配置、标"有未保存改动"、并**立刻反映到实时预览**上；
 *   3. 「保存并生效」把整份配置 PUT 回后端（后端会当场让桌面浮层用上）。
 *
 * 没有构建步骤、没有任何依赖：浏览器直接吃 ES 模块，断网也能用。
 */

import { $, clamp, esc } from './lib/dom.js'
import { api } from './lib/api.js'
import { mountPreview } from './lib/preview.js'
import { backgroundCss, crtOn } from './lib/spectrum.js'

const state = {
  config: null,
  meta: null,
  dirty: false,
  busy: false,
  saved: null,       // 与磁盘一致的那份（拿来算 dirty）
}

const els = {
  size: $('#size'),
  sizeVal: $('#size-val'),
  count: $('#count'),
  countVal: $('#count-val'),
  height: $('#height'),
  heightVal: $('#height-val'),
  intensity: $('#intensity'),
  intensityVal: $('#intensity-val'),
  bass: $('#bass'),
  bassVal: $('#bass-val'),
  mid: $('#mid'),
  midVal: $('#mid-val'),
  treble: $('#treble'),
  trebleVal: $('#treble-val'),
  mixReadout: $('#mix-readout'),
  rotate: $('#rotate'),
  link: $('#link'),
  sources: [...document.querySelectorAll('[data-source]')],
  delay: $('#delay'),
  delayVal: $('#delay-val'),
  device: $('#device'),
  deviceNote: $('#device-note'),
  colors: $('#colors'),
  addColor: $('#add-color'),
  settingsTabs: $('#settings-tabs'),
  bgColor: $('#bg-color'),
  bgOpacity: $('#bg-opacity'),
  bgOpacityVal: $('#bg-opacity-val'),
  crt: $('#crt'),
  afterglow: $('#afterglow'),
  afterglowVal: $('#afterglow-val'),
  barrel: $('#barrel'),
  barrelVal: $('#barrel-val'),
  aberration: $('#aberration'),
  aberrationVal: $('#aberration-val'),
  particleSpeed: $('#particle-speed'),
  particleSpeedVal: $('#particle-speed-val'),
  particleLife: $('#particle-life'),
  particleLifeVal: $('#particle-life-val'),
  burst: $('#burst'),
  burstSensitivity: $('#burst-sensitivity'),
  burstSensitivityVal: $('#burst-sensitivity-val'),
  burstSpeed: $('#burst-speed'),
  burstSpeedVal: $('#burst-speed-val'),
  burstLife: $('#burst-life'),
  burstLifeVal: $('#burst-life-val'),
  highFps: $('#high-fps'),
  previewStage: $('.preview-stage'),
  modes: [...document.querySelectorAll('[data-mode]')],
  save: $('#btn-save'),
  reload: $('#btn-reload'),
  reset: $('#btn-reset'),
  status: $('#status'),
  previewStatus: $('#preview-status'),
  canvas: $('#preview-canvas'),
}

let preview = null

/* ------------------------------------------------------------------ 设置 */

function applyMeta() {
  if (!state.meta) return
  const [sizeLo, sizeHi] = state.meta.limits.size
  const [countLo, countHi] = state.meta.limits.count
  const [heightLo, heightHi] = state.meta.limits.height ?? [10, 90]
  const [intensityLo, intensityHi] = state.meta.limits.intensity ?? [20, 400]
  const [delayLo, delayHi] = state.meta.limits.delay ?? [0, 10000]
  els.size.min = sizeLo
  els.size.max = sizeHi
  els.count.min = countLo
  els.count.max = countHi
  els.count.step = 1
  els.height.min = heightLo
  els.height.max = heightHi
  els.intensity.min = intensityLo
  els.intensity.max = intensityHi
  els.delay.min = delayLo
  els.delay.max = delayHi
}

function renderForm() {
  const appearance = state.config.appearance
  els.size.value = appearance.size
  els.sizeVal.textContent = `${appearance.size}%`
  els.count.value = appearance.count
  els.countVal.textContent = `${appearance.count}`
  els.height.value = appearance.height
  els.heightVal.textContent = `${appearance.height}%`
  els.intensity.value = appearance.intensity
  els.intensityVal.textContent = `${appearance.intensity}%`
  for (const key of ['bass', 'mid', 'treble']) {
    els[key].value = appearance[key]
    els[`${key}Val`].textContent = `${appearance[key]}%`
  }
  els.rotate.checked = appearance.rotate !== false
  els.link.checked = appearance.link !== false
  for (const button of els.modes) {
    button.classList.toggle('on', button.dataset.mode === appearance.mode)
  }
  if (preview) preview.setAppearance(appearance)

  const audio = state.config.audio
  for (const button of els.sources) {
    button.classList.toggle('on', button.dataset.source === audio.source)
  }
  els.delay.value = audio.delay_ms
  els.delayVal.textContent = `${audio.delay_ms}ms`
  setDeviceValue(audio.device)
  renderColors()
  els.bgColor.value = appearance.bg_color ?? '#000000'
  els.bgOpacity.value = appearance.bg_opacity ?? 0
  els.bgOpacityVal.textContent = `${appearance.bg_opacity ?? 0}%`
  els.crt.checked = appearance.crt !== false
  els.crt.disabled = (appearance.bg_opacity ?? 0) <= 0   // 没背景就没“屏幕”可贴
  els.afterglow.value = appearance.afterglow ?? 60
  els.afterglowVal.textContent = `${appearance.afterglow ?? 60}%`
  els.barrel.value = appearance.barrel ?? 15
  els.barrelVal.textContent = `${appearance.barrel ?? 15}%`
  els.aberration.value = appearance.aberration ?? 30
  els.aberrationVal.textContent = `${appearance.aberration ?? 30}%`
  applyPreviewBackground()
  els.particleSpeed.value = appearance.particle_speed ?? 100
  els.particleSpeedVal.textContent = `${appearance.particle_speed ?? 100}%`
  els.particleLife.value = appearance.particle_life ?? 100
  els.particleLifeVal.textContent = `${appearance.particle_life ?? 100}%`
  els.burst.checked = appearance.burst !== false
  els.burstSensitivity.value = appearance.burst_sensitivity ?? 55
  els.burstSensitivityVal.textContent = `${appearance.burst_sensitivity ?? 55}%`
  els.burstSpeed.value = appearance.burst_speed ?? 100
  els.burstSpeedVal.textContent = `${appearance.burst_speed ?? 100}%`
  els.burstLife.value = appearance.burst_life ?? 100
  els.burstLifeVal.textContent = `${appearance.burst_life ?? 100}%`
  updateBurstDisabled()
  els.highFps.checked = Boolean(state.config.render?.high_fps)
}

/** 鼓点开关关掉时，它下面那三个滑杆都禁用。 */
function updateBurstDisabled() {
  const off = !els.burst.checked
  els.burstSensitivity.disabled = off
  els.burstSpeed.disabled = off
  els.burstLife.disabled = off
}

/** 把背景（颜色 + 不透明度）应用到预览区（浮层那边由 overlay.js 自己设 `--vs-bg`）。 */
function applyPreviewBackground() {
  if (!els.previewStage || !state.config) return
  const appearance = state.config.appearance
  const bg = backgroundCss(appearance)
  if (bg) els.previewStage.style.setProperty('--vs-bg', bg)
  else els.previewStage.style.removeProperty('--vs-bg')
  els.previewStage.classList.toggle('crt-on', crtOn(appearance))   // CRT 覆盖层只在有背景时可开
}

/** 画颜色色标编辑器（一列 <input type="color">，可加 / 可删）。 */
function renderColors() {
  const colors = state.config.appearance.colors
  els.colors.innerHTML = colors.map((color, index) => `
    <span class="swatch">
      <input type="color" value="${esc(color)}" data-index="${index}"
             aria-label="第 ${index + 1} 个颜色">
      ${colors.length > 1
        ? `<button type="button" class="swatch-del" data-index="${index}" title="删掉这一色">×</button>`
        : ''}
    </span>`).join('')
}

/** 把设备下拉选到 `value`；列表里没有就先补一个选项（设备可能刚插上/被改过）。 */
function setDeviceValue(value) {
  const wanted = value || 'auto'
  const exists = [...els.device.options].some((option) => option.value === wanted)
  if (!exists && wanted !== 'auto') {
    const option = document.createElement('option')
    option.value = wanted
    option.textContent = `${wanted}（不在当前设备列表里）`
    els.device.appendChild(option)
  }
  els.device.value = wanted
}

/** 拉一次可捕获设备列表填进下拉（每次打开配置页都重拉）。 */
async function loadDevices() {
  try {
    const payload = await api.devices()
    const devices = payload.devices ?? []
    const rows = ['<option value="auto">自动（默认输出设备）</option>']
    for (const item of devices) {
      rows.push(`<option value="${esc(item.key)}">${esc(item.label)}</option>`)
    }
    els.device.innerHTML = rows.join('')
    els.deviceNote.textContent = devices.length ? '' : '（没枚举到可捕获设备）'
    setDeviceValue(state.config?.audio?.device ?? payload.current ?? 'auto')
  } catch (error) {
    els.deviceNote.textContent = `设备列表拿不到：${error.message}`
  }
}

function renderStatus() {
  if (!state.meta) return
  const bits = [
    state.meta.running ? '浮层运行中' : '浮层未运行（仅配置）',
    state.meta.platform.session,
    state.dirty ? '有未保存改动' : '已保存',
  ]
  els.status.textContent = bits.join(' · ')
  els.save.disabled = !state.dirty || state.busy
  els.save.textContent = state.busy ? '保存中…' : '保存并生效'
  document.title = state.dirty ? 'VisualSpectrum 配置 *' : 'VisualSpectrum 配置'
}

function markEdited() {
  state.dirty = JSON.stringify(state.config) !== JSON.stringify(state.saved)
  renderForm()
  renderStatus()
}

/* ------------------------------------------------------------------ 动作 */

async function save() {
  state.busy = true
  renderStatus()
  try {
    const result = await api.save(state.config)
    state.config = result.config
    state.saved = result.config
    state.dirty = false
    showToast(result.note || '已保存', 'info')
    await refreshMeta()
  } catch (error) {
    showToast(error.message, 'error', 6000)
  } finally {
    state.busy = false
    renderForm()
    renderStatus()
  }
}

async function reload() {
  const payload = await api.config()
  state.config = payload.config
  state.saved = payload.config
  state.dirty = false
  renderForm()
  renderStatus()
  showToast('已重新载入磁盘上的配置', 'info')
}

async function reset() {
  const result = await api.reset()
  state.config = result.config
  state.saved = result.config
  state.dirty = false
  renderForm()
  renderStatus()
  showToast('已恢复默认配置', 'info')
  await refreshMeta()
}

async function refreshMeta() {
  try {
    state.meta = await api.meta()
    applyMeta()
    renderStatus()
  } catch (error) {
    /* meta 拿不到不影响设置面板 */
  }
}

/* ------------------------------------------------------------------ Toast */

const toastHost = $('#toasts')
function showToast(message, kind = 'info', ttl = 3500) {
  const node = document.createElement('div')
  node.className = `toast ${kind}`
  node.textContent = message
  toastHost.appendChild(node)
  window.setTimeout(() => node.remove(), ttl)
}

/* ------------------------------------------------------------------ 事件 */

function wire() {
  //: 设置卡的菜单栏（tabs）：点哪块就只显示哪块。
  const tabButtons = [...els.settingsTabs.querySelectorAll('button[data-tab]')]
  const tabPanels = [...document.querySelectorAll('.tab-panel')]
  const showTab = (tab) => {
    for (const button of tabButtons) button.classList.toggle('on', button.dataset.tab === tab)
    for (const panel of tabPanels) panel.hidden = panel.dataset.panel !== tab
  }
  els.settingsTabs.addEventListener('click', (event) => {
    const button = event.target.closest('button[data-tab]')
    if (button) showTab(button.dataset.tab)
  })

  els.size.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.size = clamp(Number(els.size.value), 10, 100)
    els.sizeVal.textContent = `${state.config.appearance.size}%`
    if (preview) preview.setAppearance(state.config.appearance)
    state.dirty = true
    renderStatus()
  })

  els.count.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.count = clamp(Number(els.count.value), 1, 512)
    els.countVal.textContent = `${state.config.appearance.count}`
    if (preview) preview.setAppearance(state.config.appearance)
    state.dirty = true
    renderStatus()
  })

  els.height.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.height = clamp(Number(els.height.value), 10, 90)
    els.heightVal.textContent = `${state.config.appearance.height}%`
    if (preview) preview.setAppearance(state.config.appearance)
    state.dirty = true
    renderStatus()
  })

  els.rotate.addEventListener('change', () => {
    if (!state.config) return
    state.config.appearance.rotate = els.rotate.checked
    if (preview) preview.setAppearance(state.config.appearance)
    state.dirty = true
    renderStatus()
  })

  els.intensity.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.intensity = clamp(Number(els.intensity.value), 20, 400)
    els.intensityVal.textContent = `${state.config.appearance.intensity}%`
    if (preview) preview.setAppearance(state.config.appearance)
    state.dirty = true
    renderStatus()
  })

  //: 低/中/高占比是**在 Python 侧**生效的（各自归一化后再乘），所以保存后才看得出来 ——
  //: 这里不调 preview.setAppearance。
  for (const key of ['bass', 'mid', 'treble']) {
    const input = els[key]
    const output = els[`${key}Val`]
    input.addEventListener('input', () => {
      if (!state.config) return
      state.config.appearance[key] = clamp(Number(input.value), 0, 400)
      output.textContent = `${state.config.appearance[key]}%`
      state.dirty = true
      renderStatus()
    })
  }

  els.link.addEventListener('change', () => {
    if (!state.config) return
    state.config.appearance.link = els.link.checked
    if (preview) preview.setAppearance(state.config.appearance)
    state.dirty = true
    renderStatus()
  })

  for (const button of els.sources) {
    button.addEventListener('click', () => {
      if (!state.config) return
      state.config.audio.source = button.dataset.source
      for (const other of els.sources) other.classList.toggle('on', other === button)
      state.dirty = true
      renderStatus()
    })
  }

  els.delay.addEventListener('input', () => {
    if (!state.config) return
    state.config.audio.delay_ms = clamp(Number(els.delay.value), 0, 10000)
    els.delayVal.textContent = `${state.config.audio.delay_ms}ms`
    state.dirty = true
    renderStatus()
  })

  els.device.addEventListener('change', () => {
    if (!state.config) return
    state.config.audio.device = els.device.value || 'auto'
    state.dirty = true
    renderStatus()
  })

  //: 颜色改动**不重画**色块 —— 否则拖色盘时焦点会丢；只改配置与预览。
  els.colors.addEventListener('input', (event) => {
    const input = event.target.closest('input[type="color"]')
    if (!input || !state.config) return
    state.config.appearance.colors[Number(input.dataset.index)] = input.value.toUpperCase()
    if (preview) preview.setAppearance(state.config.appearance)
    state.dirty = true
    renderStatus()
  })

  els.colors.addEventListener('click', (event) => {
    const del = event.target.closest('.swatch-del')
    if (!del || !state.config) return
    const colors = state.config.appearance.colors
    if (colors.length <= 1) return
    colors.splice(Number(del.dataset.index), 1)
    renderColors()
    if (preview) preview.setAppearance(state.config.appearance)
    state.dirty = true
    renderStatus()
  })

  els.addColor.addEventListener('click', () => {
    if (!state.config) return
    const colors = state.config.appearance.colors
    if (colors.length >= 8) {
      showToast('最多 8 个颜色', 'error')
      return
    }
    colors.push(colors[colors.length - 1] ?? '#7EE7FF')
    renderColors()
    if (preview) preview.setAppearance(state.config.appearance)
    state.dirty = true
    renderStatus()
  })

  els.bgColor.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.bg_color = els.bgColor.value.toUpperCase()
    applyPreviewBackground()
    state.dirty = true
    renderStatus()
  })

  els.bgOpacity.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.bg_opacity = clamp(Number(els.bgOpacity.value), 0, 100)
    els.bgOpacityVal.textContent = `${state.config.appearance.bg_opacity}%`
    els.crt.disabled = state.config.appearance.bg_opacity <= 0
    applyPreviewBackground()
    state.dirty = true
    renderStatus()
  })

  els.crt.addEventListener('change', () => {
    if (!state.config) return
    state.config.appearance.crt = els.crt.checked
    applyPreviewBackground()
    state.dirty = true
    renderStatus()
  })

  els.afterglow.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.afterglow = clamp(Number(els.afterglow.value), 0, 100)
    els.afterglowVal.textContent = `${state.config.appearance.afterglow}%`
    if (preview) preview.setAppearance(state.config.appearance)   // 余晖是画布效果，预览可直接看
    state.dirty = true
    renderStatus()
  })

  //: 粒子（散射 / 鼓点）也都是画布效果 —— 改完预览直接看得到。
  const bindParticle = (input, output, key, lo, hi) => {
    input.addEventListener('input', () => {
      if (!state.config) return
      state.config.appearance[key] = clamp(Number(input.value), lo, hi)
      output.textContent = `${state.config.appearance[key]}%`
      if (preview) preview.setAppearance(state.config.appearance)
      state.dirty = true
      renderStatus()
    })
  }
  bindParticle(els.particleSpeed, els.particleSpeedVal, 'particle_speed', 20, 400)
  bindParticle(els.particleLife, els.particleLifeVal, 'particle_life', 20, 400)
  bindParticle(els.burstSpeed, els.burstSpeedVal, 'burst_speed', 20, 400)
  bindParticle(els.burstLife, els.burstLifeVal, 'burst_life', 20, 400)
  bindParticle(els.burstSensitivity, els.burstSensitivityVal, 'burst_sensitivity', 0, 100)
  // 桶形畸变 / RGB 分色也是画布效果（WebGL 后处理）—— 预览直接看得到。
  bindParticle(els.barrel, els.barrelVal, 'barrel', 0, 100)
  bindParticle(els.aberration, els.aberrationVal, 'aberration', 0, 100)

  els.burst.addEventListener('change', () => {
    if (!state.config) return
    state.config.appearance.burst = els.burst.checked
    updateBurstDisabled()
    if (preview) preview.setAppearance(state.config.appearance)
    state.dirty = true
    renderStatus()
  })

  els.highFps.addEventListener('change', () => {
    if (!state.config) return
    state.config.render = state.config.render ?? {}
    state.config.render.high_fps = els.highFps.checked
    state.dirty = true
    renderStatus()
    showToast('高刷新率改动要重启才生效', 'info')
  })

  for (const button of els.modes) {
    button.addEventListener('click', () => {
      if (!state.config) return
      state.config.appearance.mode = button.dataset.mode
      for (const other of els.modes) other.classList.toggle('on', other === button)
      state.dirty = true
      renderStatus()
    })
  }

  els.save.addEventListener('click', () => {
    if (!state.config) return
    void save()
  })
  els.reload.addEventListener('click', () => void reload().catch((e) => showToast(e.message, 'error')))
  els.reset.addEventListener('click', () => void reset().catch((e) => showToast(e.message, 'error')))
}

/* ------------------------------------------------------------------ 启动 */

async function main() {
  wire()

  try {
    const [meta, config] = await Promise.all([api.meta(), api.config()])
    state.meta = meta
    state.config = config.config
    state.saved = config.config
  } catch (error) {
    $('#preview-status').textContent = `连不上后端：${error.message}`
    $('#preview-status').classList.add('error')
    return
  }

  applyMeta()
  renderForm()
  renderStatus()
  await loadDevices()

  preview = mountPreview(els.canvas, {
    onStatus: ({ live, source, mix }) => {
      els.previewStatus.textContent = live ? `实时音频 · ${source}` : `演示波形 · ${source}`
      els.previewStatus.classList.toggle('demo', !live)
      if (mix) {
        const pct = (value) => `${Math.round(value * 100)}%`
        els.mixReadout.textContent =
          `低 / 中 / 高音占比：${pct(mix.bass)} · ${pct(mix.mid)} · ${pct(mix.treble)}`
      }
    },
  })
  preview.setAppearance(state.config.appearance)

  if (state.meta.platform) {
    document.body.dataset.session = state.meta.platform.session
  }
  const failing = state.meta.selfcheck?.failed ?? []
  if (failing.length) {
    showToast(`自检有未通过项：${failing.join('、')}`, 'error', 8000)
  }
}

void main()
