/**
 * 环形频谱渲染器 —— 浮层与配置页预览**共用这一份**。
 *
 * 画的是：一圈随音乐起伏的频谱条 + CRT 荧光辉光（bloom）+ **荧光余晖拖尾**（条与粒子会拖出光尾）+ 从条端逸出的散射粒子 + 中心一圈套一圈的同心环。
 * 只认调用方给的数据（`pushFrame({bands, live})`），不自己取音频 —— 音频在 Python 那边。
 *
 * 性能上的两个关键取舍：
 *   1. **不用 `shadowBlur`**：它对每个绘制对象都开一次高斯，`count` 上百时必然是掉帧元凶。
 *      辉光改用"把整幅画面降采样到 1/4、模糊一下、再用 `lighter` 叠回来"的廉价 bloom 近似。
 *   2. **发光点用池子**：粒子是复用对象，不是每帧 new —— 免得 GC 抖动把帧率拉出毛刺。
 */

/** 把 `value` 夹到 `[lo, hi]`。 */
function clamp(value, lo, hi) {
  return value < lo ? lo : value > hi ? hi : value
}

/** 线性插值。 */
function lerp(a, b, t) {
  return a + (b - a) * t
}

/** 默认色标（青 → 紫 → 品红），与 Python 侧 `config.DEFAULT_COLORS` 一致。 */
const DEFAULT_COLORS = ['#7EE7FF', '#B37BFF', '#FF6FD8']

/** 我们自己限速的绘制上限（fps）。Chromium 解锁帧率上限后 rAF 可能飙到几千 Hz，
 *  这里把它按到 120，省下 CPU/GPU；60Hz 屏由 vsync 决定，这个上限根本不会触发。 */
const MAX_DRAW_FPS = 120
const DRAW_INTERVAL_MS = 1000 / MAX_DRAW_FPS

/** 余晖（荧光拖尾）强度（0~100）→ 衰减速率（每秒）。越大拖尾越长；0 = 不留余晖（每帧全清）。 */
function afterglowRate(level) {
  const v = clamp(Number(level ?? 60), 0, 100)
  if (v <= 0) return Infinity          // exp(-∞·dt) = 0 → fade = 1 → 每帧完全擦掉
  return lerp(24, 2, (v - 1) / 99)     // v=1 拖尾最短, v=100 最长
}

/** 把"每帧系数"换算成"每秒速率"（`rate = -ln(1-k)·60`），好让动画**与帧率无关**：
 *  60fps 与 120fps 下同样的手感。 */
function rateFor(k60) {
  return -Math.log(1 - k60) * 60
}
const ATTACK_RATE = rateFor(0.78)
const RELEASE_RATE = rateFor(0.32)

/** 桶形畸变 / RGB 分色（WebGL 后处理）拉满时的强度。 */
const BARREL_MAX = 0.35
const ABERRATION_MAX = 0.0035

const WARP_VERT = `
attribute vec2 aPos;
varying vec2 vUv;
void main() {
  vUv = aPos * 0.5 + 0.5;
  gl_Position = vec4(aPos, 0.0, 1.0);
}`

// 桶形畸变 + RGB 分色一次做完（同一个后处理 pass）
const WARP_FRAG = `
precision mediump float;
varying vec2 vUv;
uniform sampler2D uScene;
uniform float uBarrel;
uniform float uAberration;
void main() {
  vec2 d = vUv - vec2(0.5);
  float r2 = dot(d, d);
  // 桶形：越靠边采样点越往中心收 → 画面边缘向外鼓
  vec2 uv = vec2(0.5) + d * (1.0 - uBarrel * r2 * 4.0);
  // RGB 分色：R 往外偏、B 往里偏，越靠边越明显
  vec2 off = d * (uAberration * r2 * 8.0);
  vec4 sR = texture2D(uScene, uv + off);
  vec4 sG = texture2D(uScene, uv);
  vec4 sB = texture2D(uScene, uv - off);
  vec4 col = vec4(sR.r, sG.g, sB.b, max(sG.a, max(sR.a, sB.a)));
  if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) col = vec4(0.0);
  gl_FragColor = col;
}`

/** 编译一个着色器（出错就抛）。 */
function compileShader(gl, type, source) {
  const shader = gl.createShader(type)
  gl.shaderSource(shader, source)
  gl.compileShader(shader)
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(gl.getShaderInfoLog(shader) || 'shader 编译失败')
  }
  return shader
}

/** `#RRGGBB` → `[r, g, b]`（解析不了就退成青色）。 */
function parseHex(hex) {
  const match = /^#([0-9a-fA-F]{6})$/.exec(String(hex ?? '').trim())
  if (!match) return [126, 231, 255]
  const n = parseInt(match[1], 16)
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}

/** 往白色提 `t` —— 亮度用（比 HSL 好控）。 */
function mixWhite(rgb, t) {
  return [lerp(rgb[0], 255, t), lerp(rgb[1], 255, t), lerp(rgb[2], 255, t)]
}

/** 生成 `rgba(r, g, b, a)` 字符串。 */
function rgba(rgb, alpha) {
  return `rgba(${Math.round(rgb[0])}, ${Math.round(rgb[1])}, ${Math.round(rgb[2])}, ${alpha})`
}

/**
 * 把背景设置（颜色 + 不透明度）算成 CSS 的 `rgba(...)`；不透明度为 0 时返回 `null`
 * （= 不要背景，桌面直接透出来）。配置页预览与浮层都用它。
 */
export function backgroundCss(appearance) {
  const opacity = clamp(Number(appearance?.bg_opacity ?? 0), 0, 100)
  if (opacity <= 0) return null
  const [r, g, b] = parseHex(appearance?.bg_color ?? '#000000')
  return `rgba(${r}, ${g}, ${b}, ${opacity / 100})`
}

/**
 * 要不要显示 CRT 覆盖层（扫描线 / 暗角）：**只有存在背景时**才谈得上 ——
 * 没背景（不透明度 0）时它就是给整屏加一层灰，反而破坏了"全透明"。
 */
export function crtOn(appearance) {
  return backgroundCss(appearance) !== null && appearance?.crt !== false
}

/**
 * 没有真实数据时的演示波形（配置页独立打开、或引擎还没起来时用）。
 * 与 Python 侧 `demo_frame()` 的观感一致：低频鼓点 + 几团漂移的能量。
 */
function demoBands(t, count) {
  const blobs = [
    [0.10, 0.05, 0.05],
    [0.34, -0.07, 0.06],
    [0.62, 0.04, 0.05],
    [0.85, -0.03, 0.04],
  ]
  const out = new Array(count)
  for (let i = 0; i < count; i += 1) {
    const x = count > 1 ? i / (count - 1) : 0
    let v = 0.05 + 0.04 * Math.sin(i * 12.9898 + t * 3.7)
    v += 0.9 * Math.exp(-(((x - 0.02) / 0.07) ** 2)) * (0.5 + 0.5 * Math.sin(2 * Math.PI * 2.0 * t))
    for (const [phase, speed, width] of blobs) {
      const center = ((phase + speed * t) % 1 + 1) % 1
      v += 0.55 * Math.exp(-(((x - center) / width) ** 2))
    }
    out[i] = clamp(v, 0, 1)
  }
  return out
}

export class SpectrumView {
  /**
   * @param {HTMLCanvasElement} canvas
   */
  constructor(canvas) {
    this.canvas = canvas
    this.ctx = canvas.getContext('2d')

    /** 设置（大小 / 数量 / 条高 / 强度 / 连线 / 旋转 / 颜色 / 模式）。 */
    this.appearance = {
      size: 55, count: 96, height: 48, intensity: 100, link: true, rotate: true,
      afterglow: 60,
      barrel: 15, aberration: 30,
      particle_speed: 100, particle_life: 100,
      burst: true, burst_sensitivity: 55, burst_speed: 100, burst_life: 100,
      colors: DEFAULT_COLORS.slice(), mode: 'top',
    }

    /** 目标频段（来自 Python / 轮询）与当前显示值（逐帧插值平滑）。 */
    this.target = []
    this.shown = []

    /** 发光点池。 */
    this.particles = []
    this.maxParticles = 700

    /** 整个环的自转角（`rotate: false` 时冻结不动）。 */
    this.rotation = 0

    this.running = false
    this._raf = 0
    this._last = 0
    this._demoTime = 0
    this._live = false
    this._source = ''

    /** 廉价 bloom 用的离屏画布（1/4 分辨率）。 */
    this._bloom = document.createElement('canvas')
    this._bloomCtx = this._bloom.getContext('2d')

    /** 余晖（荧光拖尾）用的离屏画布（全分辨率）：不清屏，让上一帧慢慢淡出。 */
    this._trail = document.createElement('canvas')
    this._trailCtx = this._trail.getContext('2d')

    /** 场景画布（全分辨率）：开了桶形/色散后处理时，先把画面画到这里，再由 WebGL 变形贴回可见画布。 */
    this._scene = document.createElement('canvas')
    this._sceneCtx = this._scene.getContext('2d')

    /** WebGL 后处理（懒创建；起不来就退回直接贴图）。 */
    this._warp = null
    this._gl = null
    this._glFailed = false

    this._onResize = () => this.resize()
    this._observer = typeof ResizeObserver !== 'undefined'
      ? new ResizeObserver(() => this.resize())
      : null
  }

  // ------------------------------------------------------------------ 生命周期
  start() {
    if (this.running) return
    this.running = true
    window.addEventListener('resize', this._onResize)
    this._observer?.observe(this.canvas)
    this.resize()
    this._last = performance.now()
    this._raf = requestAnimationFrame((now) => this._tick(now))
  }

  stop() {
    this.running = false
    cancelAnimationFrame(this._raf)
    window.removeEventListener('resize', this._onResize)
    this._observer?.disconnect()
  }

  // ------------------------------------------------------------------ 输入
  setAppearance(appearance) {
    if (!appearance) return
    Object.assign(this.appearance, appearance)
  }

  pushFrame(frame) {
    if (!frame) return
    this.target = Array.isArray(frame.bands) ? frame.bands : []
    this._live = Boolean(frame.live)
    this._source = String(frame.source ?? '')
  }

  // ------------------------------------------------------------------ 尺寸
  resize() {
    const dpr = Math.min(window.devicePixelRatio || 1, 2)
    const width = this.canvas.clientWidth || window.innerWidth
    const height = this.canvas.clientHeight || window.innerHeight
    this.canvas.width = Math.max(1, Math.round(width * dpr))
    this.canvas.height = Math.max(1, Math.round(height * dpr))
    this._dpr = dpr
    this._w = width
    this._h = height
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0)

    const bw = Math.max(1, Math.round(width / 4))
    const bh = Math.max(1, Math.round(height / 4))
    this._bloom.width = bw
    this._bloom.height = bh
    this._bw = bw
    this._bh = bh

    // 余晖层跟主画布同尺寸；改尺寸会**顺便清空**它（反正要重新开始了）
    this._trail.width = this.canvas.width
    this._trail.height = this.canvas.height

    // 离屏场景 / WebGL 后处理输出也是同样尺寸
    this._scene.width = this.canvas.width
    this._scene.height = this.canvas.height
    if (this._warp) {
      this._warp.width = this.canvas.width
      this._warp.height = this.canvas.height
    }
  }

  // ------------------------------------------------------------------ 后处理（WebGL）
  /** 要不要走 WebGL 后处理（桶形畸变 / RGB 分色）。
   *  只做 **WebEngine/Chromium** 这一条路：浮层本身就是 WebEngine 里的一个页面，
   *  所以后处理放在 JS 里最省事 —— Qt 侧一行都不用动。
   *  这里**懒创建**上下文；GL 起不来（无 WebGL / 上下文丢失）就返回 false，退回直接贴图。 */
  _postEnabled() {
    if (this._glFailed) return false
    const want = Number(this.appearance.barrel ?? 0) > 0 || Number(this.appearance.aberration ?? 0) > 0
    if (!want) return false
    this._initWarp()
    return !this._glFailed && !!this._gl
  }

  /** 懒创建 WebGL 后处理：一个离屏 canvas + 一个全屏四边形 + 一个 pass（桶形 + 色散）。 */
  _initWarp() {
    if (this._gl || this._glFailed) return
    try {
      const canvas = document.createElement('canvas')
      const gl = canvas.getContext('webgl', {
        alpha: true, premultipliedAlpha: false,
        antialias: false, depth: false, stencil: false,
      }) || canvas.getContext('experimental-webgl')
      if (!gl) throw new Error('WebGL 不可用')

      const program = gl.createProgram()
      gl.attachShader(program, compileShader(gl, gl.VERTEX_SHADER, WARP_VERT))
      gl.attachShader(program, compileShader(gl, gl.FRAGMENT_SHADER, WARP_FRAG))
      gl.linkProgram(program)
      if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
        throw new Error(gl.getProgramInfoLog(program) || 'program 链接失败')
      }
      gl.useProgram(program)

      // 全屏四边形（两个三角形）—— 顶点直接给裁剪空间坐标
      const buf = gl.createBuffer()
      gl.bindBuffer(gl.ARRAY_BUFFER, buf)
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([
        -1, -1, 1, -1, -1, 1,
        -1, 1, 1, -1, 1, 1,
      ]), gl.STATIC_DRAW)
      const aPos = gl.getAttribLocation(program, 'aPos')
      gl.enableVertexAttribArray(aPos)
      gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0)

      // 场景纹理：边缘夹取（别让畸变把画面外绕回来）+ 线性过滤（本来就是模糊的光）
      const tex = gl.createTexture()
      gl.bindTexture(gl.TEXTURE_2D, tex)
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR)
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
      // canvas 的 Y 轴朝下、纹理坐标朝上 → 上传前翻一下，否则整幅画面上下颠倒
      gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, true)

      this._gl = gl
      this._warp = canvas
      this._warpTex = tex
      this._uBarrel = gl.getUniformLocation(program, 'uBarrel')
      this._uAberration = gl.getUniformLocation(program, 'uAberration')
      gl.uniform1i(gl.getUniformLocation(program, 'uScene'), 0)

      canvas.width = this.canvas.width
      canvas.height = this.canvas.height
    } catch (err) {
      // 记一笔但**不抛**：调用方看到 false 就走直接贴图，画面照样出得来
      this._glFailed = true
      this._gl = null
      this._warp = null
    }
  }

  /** 把离屏场景经桶形 + 色散后处理后贴回可见画布。 */
  _presentWarp() {
    const gl = this._gl
    if (!gl || !this._warp) return
    const w = this.canvas.width
    const h = this.canvas.height
    if (this._warp.width !== w || this._warp.height !== h) {
      this._warp.width = w
      this._warp.height = h
    }
    gl.viewport(0, 0, w, h)
    gl.clearColor(0, 0, 0, 0)
    gl.clear(gl.COLOR_BUFFER_BIT)

    gl.activeTexture(gl.TEXTURE0)
    gl.bindTexture(gl.TEXTURE_2D, this._warpTex)
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, this._scene)

    // 0~100 → 0~满量程（BARREL_MAX / ABERRATION_MAX 是调过的"看起来够但不至于糊"的档位）
    gl.uniform1f(this._uBarrel, (clamp(this.appearance.barrel ?? 0, 0, 100) / 100) * BARREL_MAX)
    gl.uniform1f(this._uAberration, (clamp(this.appearance.aberration ?? 0, 0, 100) / 100) * ABERRATION_MAX)
    gl.drawArrays(gl.TRIANGLES, 0, 6)

    // 贴回可见画布的 2D 上下文
    const ctx = this.ctx
    ctx.setTransform(1, 0, 0, 1, 0, 0)
    ctx.globalCompositeOperation = 'source-over'
    ctx.globalAlpha = 1
    ctx.clearRect(0, 0, w, h)
    ctx.drawImage(this._warp, 0, 0)
  }

  // ------------------------------------------------------------------ 主循环
  _tick(now) {
    if (!this.running) return

    // 自限速：低于限速间隔的这次 rAF 直接跳过（不解锁时就由 vsync 决定，这里不触发）；
    // 这样即使 Chromium 解锁了帧率上限（--disable-frame-rate-limit），也不会狂烧 CPU/GPU。
    if (now - this._last < DRAW_INTERVAL_MS - 1.5) {
      this._raf = requestAnimationFrame((next) => this._tick(next))
      return
    }

    const dt = Math.min(0.05, Math.max(0.001, (now - this._last) / 1000))
    this._last = now
    this._demoTime += dt

    const count = clamp(Math.round(this.appearance.count), 4, 512)

    // 目标数据：优先真实数据；没有就用演示波形（画面永远是活的）。
    let target = this.target
    if (!target.length) {
      target = demoBands(this._demoTime, count)
    }
    // 数量对不上就重采样（配置页改数量的一瞬间会短暂不一致）
    if (target.length !== count) {
      target = resample(target, count)
    }

    // 强度 / 灵敏度：整体放大（再夹到 0~1）
    const gain = clamp((this.appearance.intensity ?? 100) / 100, 0.05, 12)

    // 显示值追赶目标值：系数用**时间**算（帧率无关），所以 60fps / 120fps 手感一致
    if (this.shown.length !== count) this.shown = new Array(count).fill(0)
    const attackK = 1 - Math.exp(-ATTACK_RATE * dt)
    const releaseK = 1 - Math.exp(-RELEASE_RATE * dt)
    for (let i = 0; i < count; i += 1) {
      const goal = clamp(target[i] * gain, 0, 1)
      const prev = this.shown[i]
      const k = goal > prev ? attackK : releaseK
      this.shown[i] = prev + (goal - prev) * k
    }

    // 整个环的自转（关掉就定格）—— 速度比之前快，"动起来"更明显
    if (this.appearance.rotate !== false) this.rotation += dt * 0.45

    this._beat(dt)
    this._emit(dt, this.shown)
    this._draw(dt)

    this._raf = requestAnimationFrame((next) => this._tick(next))
  }

  // ------------------------------------------------------------------ 几何
  _geometry() {
    const cx = this._w / 2
    const cy = this._h / 2
    const radiusOuter = (clamp(this.appearance.size, 5, 200) / 100) * Math.min(this._w, this._h) / 2
    // 条高 = 频谱条最长占环半径的百分比；条从内往外长，所以底座半径 = R - 最大条长。
    // 条高：条最长 = 环半径 × h。h ≤ 1 时条尖止于环边（底座在内）；h > 1 时底座收到圆心、
    // 条直接**冲出环外** —— 这就是"不封顶"。
    const h = clamp((this.appearance.height ?? 48) / 100, 0.05, 5)
    const maxLen = radiusOuter * h
    const radiusBase = Math.max(0, radiusOuter * (1 - h))
    return { cx, cy, radiusOuter, radiusBase, maxLen }
  }

  /** 把色标均匀铺在 `n` 份上并插值（同一套规则：1 色 = 纯色，多色 = 渐变）。
   *  按 `n + 色标` 缓存 —— `n` 有好几种用法（条数、中心环的圈数…），只有变了才重算。 */
  _paletteFor(n) {
    const stops = (this.appearance.colors?.length ? this.appearance.colors : DEFAULT_COLORS)
      .map(parseHex)
    const key = `${n}|${this.appearance.colors?.join(',') ?? ''}`
    if (!this._palettes) this._palettes = new Map()
    const hit = this._palettes.get(key)
    if (hit) return hit

    const palette = new Array(n)
    for (let i = 0; i < n; i += 1) {
      const pos = n > 1 ? i / (n - 1) : 0
      const scaled = pos * (stops.length - 1)
      const lo = Math.floor(scaled)
      const hi = Math.min(stops.length - 1, lo + 1)
      const t = scaled - lo
      const a = stops[lo]
      const b = stops[hi]
      palette[i] = [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
    }
    if (this._palettes.size > 6) this._palettes.clear()   // 改色时别一直堆
    this._palettes.set(key, palette)
    return palette
  }

  // ------------------------------------------------------------------ 粒子
  _emit(dt, bands) {
    const count = bands.length
    if (!count) return
    const { cx, cy, radiusBase, maxLen } = this._geometry()
    const palette = this._paletteFor(count)
    const speedScale = clamp((this.appearance.particle_speed ?? 100) / 100, 0.2, 4)
    const lifeScale = clamp((this.appearance.particle_life ?? 100) / 100, 0.2, 4)
    const spawn = Math.min(24, 4 + Math.round(count * 0.06))
    for (let s = 0; s < spawn; s += 1) {
      const index = (Math.random() * count) | 0
      const value = bands[index]
      if (Math.random() > value * value * 0.9 + 0.02) continue
      if (this.particles.length >= this.maxParticles) break
      const angle = (index / count) * Math.PI * 2 - Math.PI / 2 + this.rotation
      const startRadius = radiusBase + maxLen * value
      const speed = (14 + value * 70 + Math.random() * 20) * speedScale
      this.particles.push({
        angle: angle + (Math.random() - 0.5) * 0.05,
        radius: startRadius,
        speed,
        life: 0,
        ttl: (0.7 + Math.random() * 1.1) * lifeScale,
        size: 0.8 + Math.random() * 1.8,
        color: palette[index] ?? [126, 231, 255],
      })
    }
    // 更新：活到寿命尽头就消失（不再设距离上限 —— 速度/寿命都交给用户调）
    const alive = []
    for (const p of this.particles) {
      p.life += dt
      if (p.life >= p.ttl) continue
      p.radius += p.speed * dt
      p.angle += dt * 0.15            // 一点点切向漂移，看着像"散射"
      alive.push(p)
    }
    this.particles = alive
  }

  // ------------------------------------------------------------------ 鼓点
  /** 盯低频能量的"骤升"：够猛就从容心炸出一圈**全屏扩散粒子**。 */
  _beat(dt) {
    if (this.appearance.burst === false) return
    const n = this.shown.length
    if (!n) return

    const lowBands = Math.max(1, Math.round(n * 0.08))
    let bass = 0
    for (let i = 0; i < lowBands; i += 1) bass += this.shown[i]
    bass /= lowBands
    if (this._bassAvg === undefined) this._bassAvg = bass

    const sens = clamp(Number(this.appearance.burst_sensitivity ?? 55), 0, 100) / 100
    const threshold = lerp(1.55, 1.04, sens)     // 越灵敏，门槛越低
    const refractory = lerp(0.36, 0.07, sens)    // 越灵敏，两次之间越短
    this._sinceBeat = (this._sinceBeat ?? 999) + dt

    if (bass > this._bassAvg * threshold && bass > 0.15 && this._sinceBeat > refractory) {
      this._sinceBeat = 0
      this._spawnBurst(Math.round(lerp(28, 96, sens)))
    }
    // 基线缓慢跟随（时间常数 ~0.7s）—— 于是"骤升"才显出来
    this._bassAvg += (bass - this._bassAvg) * Math.min(1, dt * 1.5)
  }

  /** 从中心炸一圈粒子，在寿命内飞到屏幕边缘（"全屏扩散"）。 */
  _spawnBurst(count) {
    const n = this.shown.length || 1
    const palette = this._paletteFor(n)
    const lifeScale = clamp((this.appearance.burst_life ?? 100) / 100, 0.2, 4)
    const speedScale = clamp((this.appearance.burst_speed ?? 100) / 100, 0.2, 4)
    const maxRadius = Math.hypot(this._w, this._h) / 2
    const total = clamp(Math.round(count), 0, 140)
    for (let i = 0; i < total; i += 1) {
      if (this.particles.length >= this.maxParticles) break
      const ttl = (0.6 + Math.random() * 0.9) * lifeScale
      this.particles.push({
        angle: Math.random() * Math.PI * 2,
        radius: Math.random() * 8,                    // 从中心附近出发
        speed: (maxRadius / ttl) * 1.15 * speedScale, // 约在寿命结束时飞到屏边
        life: 0,
        ttl,
        size: 1.4 + Math.random() * 2.4,
        color: palette[(Math.random() * n) | 0] ?? [126, 231, 255],
      })
    }
  }

  // ------------------------------------------------------------------ 绘制
  _draw(dt) {
    // 开了桶形/色散（后处理）就把场景画到**离屏**，再用 WebGL 变形贴回可见画布；
    // 没开就直接画在可见画布上（不多一步全屏拷贝）。
    const post = this._postEnabled()
    const scene = post ? this._scene : this.canvas
    const ctx = post ? this._sceneCtx : this.ctx
    const { cx, cy, radiusBase, maxLen, radiusOuter } = this._geometry()
    const count = this.shown.length
    const palette = this._paletteFor(count)
    const sector = (Math.PI * 2) / Math.max(1, count)
    // 条宽：只占扇区的 42%（细条、留白多）—— 想更细就再调小这个系数。
    const barWidth = Math.max(0.9, radiusBase * sector * 0.42)

    if (!this._tipX || this._tipX.length !== count) {
      this._tipX = new Float32Array(count)
      this._tipY = new Float32Array(count)
    }

    // === 1) 余晖层（trail）：**不清屏**，而是把上一帧擦淡一点 ===
    // 用 `destination-out`（降 alpha）而不是画一层黑 —— 这是**透明**画布上做荧光余晖的正确姿势：
    // 画黑会把透明处填黑；降 alpha 才是"淡出到透明"，桌面/背景才能透出来。
    const g = this._trailCtx
    g.setTransform(this._dpr, 0, 0, this._dpr, 0, 0)
    g.globalCompositeOperation = 'destination-out'
    const fade = 1 - Math.exp(-afterglowRate(this.appearance.afterglow) * dt)
    g.fillStyle = `rgba(0, 0, 0, ${fade})`
    g.fillRect(0, 0, this._w, this._h)
    g.globalCompositeOperation = 'lighter'

    // 2) 频谱条（画到余晖层）
    for (let i = 0; i < count; i += 1) {
      const value = this.shown[i]
      const angle = (i / count) * Math.PI * 2 - Math.PI / 2 + this.rotation
      const cos = Math.cos(angle)
      const sin = Math.sin(angle)
      const len = maxLen * (0.06 + 0.94 * value)

      const x0 = cx + cos * radiusBase
      const y0 = cy + sin * radiusBase
      const x1 = cx + cos * (radiusBase + len)
      const y1 = cy + sin * (radiusBase + len)
      this._tipX[i] = x1
      this._tipY[i] = y1

      const color = palette[i]

      // 外晕（宽、淡）
      g.strokeStyle = rgba(color, 0.22)
      g.lineWidth = barWidth * 1.9
      g.lineCap = 'round'
      g.beginPath()
      g.moveTo(x0, y0)
      g.lineTo(x1, y1)
      g.stroke()

      // 芯（窄、亮）—— 越亮越往白里提
      g.strokeStyle = rgba(mixWhite(color, 0.12 + value * 0.30), 0.95)
      g.lineWidth = barWidth
      g.beginPath()
      g.moveTo(x0, y0)
      g.lineTo(x1, y1)
      g.stroke()

      // 端点的高光（越亮越白）
      if (value > 0.08) {
        g.fillStyle = rgba(mixWhite(color, 0.35 + value * 0.5), 0.35 + value * 0.6)
        g.beginPath()
        g.arc(x1, y1, barWidth * (0.38 + value * 0.30), 0, Math.PI * 2)
        g.fill()
      }
    }

    // 2b) 顶端细线相连：把各条顶端的中心点依次连起来（可关）。
    if (this.appearance.link !== false && count > 2) {
      const mid = palette[Math.floor(count / 2)] ?? [126, 231, 255]
      g.lineJoin = 'round'
      g.strokeStyle = rgba(mixWhite(mid, 0.15), 0.5)
      g.lineWidth = 1.2
      g.beginPath()
      g.moveTo(this._tipX[0], this._tipY[0])
      for (let i = 1; i < count; i += 1) g.lineTo(this._tipX[i], this._tipY[i])
      g.closePath()
      g.stroke()
      g.strokeStyle = rgba(mid, 0.16)                 // 一圈淡淡的外晕（荧光感）
      g.lineWidth = 3.2
      g.stroke()
    }

    // 2c) 散射粒子（也画到余晖层，于是会拖出光尾）
    for (const p of this.particles) {
      const fade = 1 - p.life / p.ttl
      const x = cx + Math.cos(p.angle) * p.radius
      const y = cy + Math.sin(p.angle) * p.radius
      g.fillStyle = rgba(mixWhite(p.color, 0.25), 0.75 * fade)
      g.beginPath()
      g.arc(x, y, p.size * (0.6 + fade), 0, Math.PI * 2)
      g.fill()
    }

    // === 3) 主画布：清空 → 贴余晖层 → 画中心（同心环 + 标签；静止，不参与拖尾）===
    ctx.setTransform(1, 0, 0, 1, 0, 0)
    ctx.globalCompositeOperation = 'source-over'
    ctx.clearRect(0, 0, scene.width, scene.height)
    ctx.drawImage(this._trail, 0, 0)

    ctx.setTransform(this._dpr, 0, 0, this._dpr, 0, 0)
    ctx.globalCompositeOperation = 'lighter'
    this._drawCenter(ctx, cx, cy, Math.max(radiusBase, radiusOuter * 0.2))

    // 4) 廉价 bloom：降采样 → 模糊 → 用 lighter 叠回来
    ctx.setTransform(1, 0, 0, 1, 0, 0)
    ctx.globalCompositeOperation = 'source-over'
    const bctx = this._bloomCtx
    bctx.setTransform(1, 0, 0, 1, 0, 0)
    bctx.globalCompositeOperation = 'copy'
    bctx.filter = 'blur(2px)'
    bctx.drawImage(scene, 0, 0, this._bw, this._bh)
    bctx.filter = 'none'

    ctx.globalCompositeOperation = 'lighter'
    ctx.globalAlpha = 0.85
    ctx.drawImage(this._bloom, 0, 0, scene.width, scene.height)
    ctx.globalAlpha = 1
    ctx.globalCompositeOperation = 'source-over'

    // 后处理（桶形 / RGB 分色）：把离屏场景经 WebGL 变形后贴回可见画布
    if (post) this._presentWarp()
  }

  /** 中心：一圈套一圈的**同心环**（不填充）+ X 形油光 + 标签（用自定义色）；随低频轻微起伏。 */
  _drawCenter(ctx, cx, cy, radiusBase) {
    const bass = this.shown.length ? this.shown.slice(0, 4).reduce((a, b) => a + b, 0) / 4 : 0
    const r = radiusBase * 0.92 * (1 + bass * 0.03)
    const labelR = r * 0.36

    ctx.globalCompositeOperation = 'lighter'

    // 1) 一圈淡淡的光晕（叠加）—— 让它浮在桌面上也有个底
    const halo = ctx.createRadialGradient(cx, cy, r * 0.25, cx, cy, r * 1.3)
    halo.addColorStop(0, `rgba(120, 210, 255, ${0.09 + bass * 0.10})`)
    halo.addColorStop(1, 'rgba(120, 210, 255, 0)')
    ctx.fillStyle = halo
    ctx.beginPath()
    ctx.arc(cx, cy, r * 1.3, 0, Math.PI * 2)
    ctx.fill()

    // 2) 圈套圈：从标签外缘往外一圈圈**同心环**（不填充），颜色跟中心标签**同一套规则**
    //    —— 用自定义色标；一个色 = 同色，多色 = 按圈数铺成渐变。
    const ringCount = 10
    const ringColors = this._paletteFor(ringCount)
    const inner = labelR * 1.12
    const outer = r * 0.99
    ctx.lineWidth = Math.max(1, r * 0.012)
    for (let i = 0; i < ringCount; i += 1) {
      const t = ringCount > 1 ? i / (ringCount - 1) : 0
      ctx.strokeStyle = rgba(mixWhite(ringColors[i], 0.1), 0.55)
      ctx.beginPath()
      ctx.arc(cx, cy, lerp(inner, outer, t), 0, Math.PI * 2)
      ctx.stroke()
    }

    // 3) X 形油光（跟着自转角转）—— 底下不再是实心盘面，就是几道光带掠过这些环
    ctx.save()
    ctx.beginPath()
    ctx.arc(cx, cy, r, 0, Math.PI * 2)
    ctx.clip()
    const spin = this.rotation
    this._sheenBand(ctx, cx, cy, r, spin + Math.PI / 4)
    this._sheenBand(ctx, cx, cy, r, spin + Math.PI * 3 / 4)
    ctx.restore()

    // 4) 中心标签（不变）—— 用**用户自定义的那组颜色**：一个色 = 纯色，多个色 = 一道渐变。+ 中心孔
    ctx.globalCompositeOperation = 'source-over'
    const stops = (this.appearance.colors?.length ? this.appearance.colors : DEFAULT_COLORS)
      .map(parseHex)
    const label = ctx.createLinearGradient(cx - labelR, cy - labelR, cx + labelR, cy + labelR)
    if (stops.length === 1) {
      label.addColorStop(0, rgba(stops[0], 1))
      label.addColorStop(1, rgba(stops[0], 1))
    } else {
      for (let i = 0; i < stops.length; i += 1) {
        label.addColorStop(i / (stops.length - 1), rgba(stops[i], 1))
      }
    }
    ctx.fillStyle = label
    ctx.beginPath()
    ctx.arc(cx, cy, labelR, 0, Math.PI * 2)
    ctx.fill()
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.30)'
    ctx.lineWidth = Math.max(1, r * 0.012)
    ctx.beginPath()
    ctx.arc(cx, cy, labelR, 0, Math.PI * 2)
    ctx.stroke()
    ctx.fillStyle = 'rgba(8, 9, 12, 0.92)'
    ctx.beginPath()
    ctx.arc(cx, cy, r * 0.05, 0, Math.PI * 2)
    ctx.fill()

    // 5) 回到叠加，继续画粒子
    ctx.globalCompositeOperation = 'lighter'
  }

  /** 画一条穿过中心环的亮带：`angle` 是渐变轴方向，亮带与之垂直。
   *  画两条（`angle` 与 `angle + π/2`）就是 X 形反光。 */
  _sheenBand(ctx, cx, cy, r, angle) {
    const dx = Math.cos(angle) * r
    const dy = Math.sin(angle) * r
    const grad = ctx.createLinearGradient(cx + dx, cy + dy, cx - dx, cy - dy)
    grad.addColorStop(0, 'rgba(255, 255, 255, 0)')
    grad.addColorStop(0.44, 'rgba(255, 255, 255, 0.07)')
    grad.addColorStop(0.5, 'rgba(255, 255, 255, 0.18)')
    grad.addColorStop(0.56, 'rgba(255, 255, 255, 0.07)')
    grad.addColorStop(1, 'rgba(255, 255, 255, 0)')
    ctx.fillStyle = grad
    ctx.fillRect(cx - r, cy - r, r * 2, r * 2)
  }
}

/** 把任意长度的频段重采样成 `count` 个（线性插值）。 */
function resample(bands, count) {
  const out = new Array(count)
  const n = bands.length
  for (let i = 0; i < count; i += 1) {
    const pos = count > 1 ? (i / (count - 1)) * (n - 1) : 0
    const lo = Math.floor(pos)
    const hi = Math.min(n - 1, lo + 1)
    out[i] = lerp(bands[lo] ?? 0, bands[hi] ?? 0, pos - lo)
  }
  return out
}
