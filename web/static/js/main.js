/**
 * 配置页入口。
 *
 * 就三件事：
 *   1. 拉 `/api/meta` + `/api/config`，把三项设置（大小 / 数量 / 模式）填进控件；
 *   2. 控件一变就更新内存里的配置、标"有未保存改动"；
 *   3. 「保存并生效」把整份配置 PUT 回后端（后端会当场让桌面浮层用上）。
 *
 * 没有构建步骤、没有任何依赖：浏览器直接吃 ES 模块，断网也能用。
 */

import { $, clamp, esc } from './lib/dom.js'
import { api } from './lib/api.js'

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
  rotate: $('#rotate'),
  rotateSpeed: $('#rotate-speed'),
  rotateSpeedVal: $('#rotate-speed-val'),
  link: $('#link'),
  sources: [...document.querySelectorAll('[data-source]')],
  delay: $('#delay'),
  delayVal: $('#delay-val'),
  device: $('#device'),
  deviceNote: $('#device-note'),
  colors: $('#colors'),
  centerGlow: $('#center-glow'),
  centerGlowVal: $('#center-glow-val'),
  addColor: $('#add-color'),
  settingsTabs: $('#settings-tabs'),
  pageTitle: $('#page-title'),
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
  modes: [...document.querySelectorAll('[data-mode]')],
  save: $('#btn-save'),
  reload: $('#btn-reload'),
  reset: $('#btn-reset'),
  status: $('#status'),
}

/* ------------------------------------------------------------------ 设置 */

function applyMeta() {
  if (!state.meta) return
  const [sizeLo, sizeHi] = state.meta.limits.size
  const [countLo, countHi] = state.meta.limits.count
  const [heightLo, heightHi] = state.meta.limits.height ?? [10, 90]
  const [intensityLo, intensityHi] = state.meta.limits.intensity ?? [20, 400]
  const [delayLo, delayHi] = state.meta.limits.delay ?? [0, 10000]
  const [rotateSpeedLo, rotateSpeedHi] = state.meta.limits.rotate_speed ?? [10, 300]
  const [centerGlowLo, centerGlowHi] = state.meta.limits.center_glow ?? [0, 200]
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
  els.rotateSpeed.min = rotateSpeedLo
  els.rotateSpeed.max = rotateSpeedHi
  els.centerGlow.min = centerGlowLo
  els.centerGlow.max = centerGlowHi
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
  els.rotateSpeed.value = appearance.rotate_speed ?? 100
  els.rotateSpeedVal.textContent = `${appearance.rotate_speed ?? 100}%`
  els.link.checked = appearance.link !== false
  els.centerGlow.value = appearance.center_glow ?? 60
  els.centerGlowVal.textContent = `${appearance.center_glow ?? 60}%`
  for (const button of els.modes) {
    button.classList.toggle('on', button.dataset.mode === appearance.mode)
  }

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

/** 按滑杆自己的 min/max 夹值。
 *  范围的唯一来源是后端 `/api/meta` 的 limits（由 `applyMeta()` 写进 DOM）；
 *  以前在事件里另写一份 10~100 / 10~90 / 20~400，结果滑到中段数值就不动了。 */
function clampInput(input) {
  const lo = Number(input.min)
  const hi = Number(input.max)
  return clamp(
    Number(input.value),
    Number.isFinite(lo) ? lo : 0,
    Number.isFinite(hi) ? hi : 100,
  )
}

/** 鼓点开关关掉时，它下面那三个滑杆都禁用。 */
function updateBurstDisabled() {
  const off = !els.burst.checked
  els.burstSensitivity.disabled = off
  els.burstSpeed.disabled = off
  els.burstLife.disabled = off
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
  //: 设置卡的侧边栏导航：点哪块就只显示哪块，并同步卡片顶部那张「居中标题」。
  const tabButtons = [...els.settingsTabs.querySelectorAll('button[data-tab]')]
  const tabPanels = [...document.querySelectorAll('.tab-panel')]
  const showTab = (tab) => {
    for (const button of tabButtons) button.classList.toggle('on', button.dataset.tab === tab)
    for (const panel of tabPanels) panel.hidden = panel.dataset.panel !== tab
    const label = tabButtons.find((b) => b.dataset.tab === tab)
      ?.querySelector('.nav-txt b')?.textContent?.trim()
    if (label && els.pageTitle) els.pageTitle.textContent = label
  }
  // 初始标题与默认选中的那一栏对上（HTML 里默认为「显示设置」）
  showTab(tabButtons.find((button) => button.classList.contains('on'))?.dataset.tab ?? 'display')
  els.settingsTabs.addEventListener('click', (event) => {
    const button = event.target.closest('button[data-tab]')
    if (button) showTab(button.dataset.tab)
  })

  els.size.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.size = clampInput(els.size)
    els.sizeVal.textContent = `${state.config.appearance.size}%`
    state.dirty = true
    renderStatus()
  })

  els.count.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.count = clampInput(els.count)
    els.countVal.textContent = `${state.config.appearance.count}`
    state.dirty = true
    renderStatus()
  })

  els.height.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.height = clampInput(els.height)
    els.heightVal.textContent = `${state.config.appearance.height}%`
    state.dirty = true
    renderStatus()
  })

  els.rotate.addEventListener('change', () => {
    if (!state.config) return
    state.config.appearance.rotate = els.rotate.checked
    state.dirty = true
    renderStatus()
  })

  els.rotateSpeed.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.rotate_speed = clampInput(els.rotateSpeed)
    els.rotateSpeedVal.textContent = `${state.config.appearance.rotate_speed}%`
    state.dirty = true
    renderStatus()
  })

  els.centerGlow.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.center_glow = clampInput(els.centerGlow)
    els.centerGlowVal.textContent = `${state.config.appearance.center_glow}%`
    state.dirty = true
    renderStatus()
  })

  els.intensity.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.intensity = clampInput(els.intensity)
    els.intensityVal.textContent = `${state.config.appearance.intensity}%`
    state.dirty = true
    renderStatus()
  })

  //: 低/中/高占比在音频侧生效，保存后才看得出来。
  for (const key of ['bass', 'mid', 'treble']) {
    const input = els[key]
    const output = els[`${key}Val`]
    input.addEventListener('input', () => {
      if (!state.config) return
      state.config.appearance[key] = clampInput(input)
      output.textContent = `${state.config.appearance[key]}%`
      state.dirty = true
      renderStatus()
    })
  }

  els.link.addEventListener('change', () => {
    if (!state.config) return
    state.config.appearance.link = els.link.checked
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
    state.config.audio.delay_ms = clampInput(els.delay)
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
    state.dirty = true
    renderStatus()
  })

  els.bgColor.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.bg_color = els.bgColor.value.toUpperCase()
    state.dirty = true
    renderStatus()
  })

  els.bgOpacity.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.bg_opacity = clampInput(els.bgOpacity)
    els.bgOpacityVal.textContent = `${state.config.appearance.bg_opacity}%`
    els.crt.disabled = state.config.appearance.bg_opacity <= 0
    state.dirty = true
    renderStatus()
  })

  els.crt.addEventListener('change', () => {
    if (!state.config) return
    state.config.appearance.crt = els.crt.checked
    state.dirty = true
    renderStatus()
  })

  els.afterglow.addEventListener('input', () => {
    if (!state.config) return
    state.config.appearance.afterglow = clampInput(els.afterglow)
    els.afterglowVal.textContent = `${state.config.appearance.afterglow}%`
    state.dirty = true
    renderStatus()
  })

  //: 粒子（散射 / 鼓点）与 CRT 参数都走滑杆自己的范围。
  const bindRange = (input, output, key) => {
    input.addEventListener('input', () => {
      if (!state.config) return
      state.config.appearance[key] = clampInput(input)
      output.textContent = `${state.config.appearance[key]}%`
      state.dirty = true
      renderStatus()
    })
  }
  bindRange(els.particleSpeed, els.particleSpeedVal, 'particle_speed')
  bindRange(els.particleLife, els.particleLifeVal, 'particle_life')
  bindRange(els.burstSpeed, els.burstSpeedVal, 'burst_speed')
  bindRange(els.burstLife, els.burstLifeVal, 'burst_life')
  bindRange(els.burstSensitivity, els.burstSensitivityVal, 'burst_sensitivity')
  bindRange(els.barrel, els.barrelVal, 'barrel')
  bindRange(els.aberration, els.aberrationVal, 'aberration')

  els.burst.addEventListener('change', () => {
    if (!state.config) return
    state.config.appearance.burst = els.burst.checked
    updateBurstDisabled()
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
    els.status.textContent = `连不上后端：${error.message}`
    return
  }

  applyMeta()
  renderForm()
  renderStatus()
  await loadDevices()

  if (state.meta.platform) {
    document.body.dataset.session = state.meta.platform.session
  }
  const failing = state.meta.selfcheck?.failed ?? []
  if (failing.length) {
    showToast(`自检有未通过项：${failing.join('、')}`, 'error', 8000)
  }
}

void main()
