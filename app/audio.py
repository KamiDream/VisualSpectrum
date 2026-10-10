"""VisualSpectrum —— 音频抓取 → FFT → 频段能量。

目标：把**系统正在播放的声音**（而不是麦克风）变成 `count` 个 0~1 的频段电平，
交给浮层去画。整条链路在后台线程里跑，浮层只读"最新一帧"。

三条来源（按可用性依次降级）：

1. **soundcard** —— 跨平台回环捕获（Windows 走 WASAPI loopback，Linux 走
   PulseAudio / PipeWire 的 monitor 设备）。首选，因为它"回环"这件事最简单。
2. **sounddevice** —— 次选（Windows 的 WASAPI loopback / Linux 的 monitor 设备）。
3. **demo** —— 没有可用音频后端 / 抓取失败时，合成一段**会舞动的**频谱，
   保证"画面永远是活的"（也让没有声卡的机器能看效果）。

分析是纯数值活：`band_bins()` / `map_bands()` / `SpectrumProcessor` 都是**纯函数 / 纯对象**，
不依赖 numpy、可以单独测；只有 `_analyze()` 那一步要用 numpy 做 FFT。
"""

from __future__ import annotations

import math
import random
import sys
import threading
import time
from typing import Protocol

from . import log

__all__ = [
    "AudioEngine", "SpectrumProcessor", "band_bins", "map_bands",
    "bands_of", "demo_frame", "list_devices", "region_bounds", "REGION_SPLIT",
]

#: 采样率 / FFT 长度 —— 2048 @ 48kHz ≈ 43ms 一帧，频率分辨率 ≈ 23Hz，够画频谱了。
SAMPLE_RATE = 48000
FFT_SIZE = 2048

#: 频段覆盖范围（Hz）。低于 30Hz 基本是直流/噪声，高于 ~16kHz 人耳听不到。
F_MIN = 30.0
F_MAX = 16000.0

#: demo 模式每秒出多少帧（真机路径跟着音频块走，约 47fps）。
DEMO_FPS = 60.0

#: 低音 / 中音 / 高音的分界（占**频段数**的比例）。
#: 频段是按对数均分的（30Hz~16kHz，约 9.4 个八度），所以 34% 处 ≈ 250Hz（低音上界）、
#: 78% 处 ≈ 4kHz（中音上界）。前端算“占比”读数时用**同一组**比例。
REGION_SPLIT = (0.34, 0.78)


# ==========================================================================
# 纯函数：频段划分与聚合
# ==========================================================================

def band_bins(count: int, sample_rate: int = SAMPLE_RATE, fft_size: int = FFT_SIZE,
              f_min: float = F_MIN, f_max: float = F_MAX) -> list[tuple[int, int]]:
    """把 `rfft` 的输出（长 `fft_size//2 + 1`）切成 `count` 段，返回每段的 `[start, stop)`。

    频段按**对数**均分（音乐是八度的，不是线性的）：低频窄、高频宽。
    保证每段至少 1 个 bin（否则最里面那几段会空），且都在合法范围内。

    ⚠ 纯函数、不碰 numpy —— 因为有静态测试要钉住"段是单调递增、刚好铺满"这些性质。
    """
    if count < 1:
        raise ValueError("count 至少要 1")
    bins = fft_size // 2 + 1
    nyquist = sample_rate / 2.0
    hi_hz = min(float(f_max), nyquist * 0.98)
    lo_hz = max(float(f_min), 20.0)
    if hi_hz <= lo_hz:
        hi_hz = lo_hz * 2.0

    ratio = hi_hz / lo_hz
    out: list[tuple[int, int]] = []
    for i in range(count):
        lo_hz_i = lo_hz * ratio ** (i / count)
        hi_hz_i = lo_hz * ratio ** ((i + 1) / count)
        # 频率 → bin 下标：bin = f / nyquist * (bins - 1)
        lo = int(round(lo_hz_i / nyquist * (bins - 1)))
        hi = int(round(hi_hz_i / nyquist * (bins - 1)))
        lo = max(0, min(lo, bins - 1))
        hi = max(hi, lo + 1)
        hi = min(hi, bins)
        out.append((lo, hi))
    return out


def map_bands(magnitudes: list[float], ranges: list[tuple[int, int]]) -> list[float]:
    """把每个频段里的幅度取平均。纯 list 运算，不依赖 numpy。"""
    out: list[float] = []
    for lo, hi in ranges:
        segment = magnitudes[lo:hi]
        out.append(sum(segment) / len(segment) if segment else 0.0)
    return out


def bands_of(samples, *, sample_rate: int = SAMPLE_RATE, fft_size: int = FFT_SIZE,
             ranges: list[tuple[int, int]]) -> list[float]:
    """对一帧采样做 FFT 并聚合成频段幅度（需要 numpy）。

    `samples` 可以是任意 array-like（numpy 数组 / list）。返回 `len(ranges)` 个线性幅度。
    """
    import numpy as np

    x = np.asarray(samples, dtype=np.float32)
    if x.ndim > 1:                     # (frames, channels) → 平均成单路（比"只取第一路"稳）
        x = x.mean(axis=1)
    if x.size < fft_size:
        x = np.pad(x, (0, fft_size - x.size))
    elif x.size > fft_size:
        x = x[-fft_size:]

    window = np.hanning(fft_size)
    spectrum = np.abs(np.fft.rfft(x * window, n=fft_size))
    return map_bands(spectrum.tolist(), ranges)


# ==========================================================================
# 平滑 + 自动增益
# ==========================================================================

def region_bounds(count: int) -> list[tuple[int, int, int]]:
    """把 `count` 个频段切成低/中/高三块，返回 `[(lo, hi, 区域号)]`。

    区域号 0=低音 / 1=中音 / 2=高音，用来取对应的增益。块一定非空、刚好铺满。
    段数太少（<3）就归成一块（只出现在测试里）。
    """
    if count < 3:
        return [(0, count, 0)]
    lo_split, hi_split = REGION_SPLIT
    b1 = min(count - 2, max(1, round(count * lo_split)))
    b2 = min(count - 1, max(b1 + 1, round(count * hi_split)))
    return [(0, b1, 0), (b1, b2, 1), (b2, count, 2)]


class SpectrumProcessor:
    """把原始幅度变成"好看、稳定"的 0~1 电平。

    四步（顺序有意）：

    1. **分区域自动增益**：低音 / 中音 / 高音**各自**跟一条"快速上冲、缓慢回落"的参考线。
       ⚠ 这一步是"低音不再打满"的关键：以前用**全局**峰值做参考线，而低音通常就是全局最响
       的那一路，于是低音永远顶在 1.0、毫无起伏；分区域之后，每块只看自己的相对强弱。
       参考线**回落得慢**（`ref_falloff`），于是"一下强、一下弱"才会显出起伏
       （回落太快的话, 参考线跟着掉, 电平反而一直卡在 1）。
    2. **占比增益**：低/中/高各自的倍数（`gains`）—— 就是配置里的"幅度占比"。
    3. **伽马**：`x ** gamma`，把低电平也托起来一点（不然小声音只剩底面）。
    4. **攻击 / 释放平滑**：上升快（`attack`）、回落慢（`release`），像真的 VU 表。

    ⚠ 纯对象、不碰 numpy —— 可以直接喂一串 list 测它。
    """

    def __init__(self, count: int, *, attack: float = 0.75, release: float = 0.30,
                 gamma: float = 0.75, ref_falloff: float = 0.05,
                 gains: tuple[float, float, float] = (1.0, 1.0, 1.0)) -> None:
        self.count = count
        self.attack = attack
        self.release = release
        self.gamma = gamma
        #: 参考线回落速度（每帧向当前峰值靠拢多少）—— 越小越"长记忆"，起伏越明显
        self.ref_falloff = ref_falloff
        self.gains = list(gains)
        self.regions = region_bounds(count)
        self.global_ref = 1e-6
        self.refs = [1e-6, 1e-6, 1e-6]
        self.values = [0.0] * count

    def set_gains(self, gains: tuple[float, float, float]) -> None:
        """更新低 / 中 / 高的占比倍数（线程间只换一个 list，安全）。"""
        self.gains = list(gains)

    def reset(self, count: int | None = None) -> None:
        if count is not None:
            self.count = count
            self.regions = region_bounds(count)
        self.global_ref = 1e-6
        self.refs = [1e-6, 1e-6, 1e-6]
        self.values = [0.0] * self.count

    def process(self, bands: list[float]) -> list[float]:
        """`bands` = 原始线性幅度；返回等长的 0~1 电平（就地更新内部状态）。"""
        if len(bands) != self.count:
            self.reset(len(bands))

        falloff = self.ref_falloff
        peak_all = max(bands) if bands else 0.0
        self.global_ref = (peak_all if peak_all > self.global_ref
                           else self.global_ref + (peak_all - self.global_ref) * falloff)
        #: 整块区域都安静时的"参考线地板"—— 免得分母趋近 0 把噪声放大成满格
        floor = max(self.global_ref * 0.05, 1e-6)

        out = [0.0] * self.count
        for lo, hi, region in self.regions:
            segment = bands[lo:hi]
            peak = max(segment) if segment else 0.0
            ref = self.refs[region]
            ref = peak if peak > ref else ref + (peak - ref) * falloff
            ref = max(ref, floor)
            self.refs[region] = ref

            gain = self.gains[region] / ref
            for i in range(lo, hi):
                x = bands[i] * gain
                if x < 0.0:
                    x = 0.0
                elif x > 1.0:
                    x = 1.0
                x = x ** self.gamma
                prev = self.values[i]
                k = self.attack if x > prev else self.release
                y = prev + (x - prev) * k
                self.values[i] = y
                out[i] = y
        return out


# ==========================================================================
# demo 源：没有音频后端时也能看见画面
# ==========================================================================

def demo_frame(t: float, count: int, blobs: list[tuple[float, float, float]],
               rng: random.Random) -> list[float]:
    """合成一帧"像音乐"的幅度谱。

    * 低频一个跟着节拍鼓动的强峰（像底鼓）；
    * 几团在频轴上缓慢漂移的能量（像旋律 / 和声）；
    * 一点噪声当底噪。

    `blobs` 里每项是 `(起始相位, 漂移速度, 宽度)`。
    """
    out: list[float] = []
    for i in range(count):
        x = i / max(1, count - 1)
        value = 0.05 + rng.random() * 0.05
        # 底鼓：低频（x≈0）上一记 2Hz 的脉冲
        value += 0.9 * math.exp(-((x - 0.02) / 0.07) ** 2) * (0.5 + 0.5 * math.sin(2 * math.pi * 2.0 * t))
        # 漂移的能量团
        for phase, speed, width in blobs:
            center = (phase + speed * t) % 1.0
            delta = x - center
            value += 0.55 * math.exp(-(delta / width) ** 2)
        out.append(value)
    return out


# ==========================================================================
# 音频来源
# ==========================================================================

class _Source(Protocol):
    name: str
    samplerate: int

    def read(self, frames: int): ...          # -> 一帧采样（array-like）
    def close(self) -> None: ...


def _device_int(device, attr: str, fallback: int) -> int:
    """从音频设备对象上读一个正整数属性（拿不到就返回 `fallback`）。

    用于拿到设备**自己**的采样率 / 通道数 —— 尽量按设备的格式打开，
    而不是硬塞一个我们想要的格式（见 `_SoundCardSource`）。
    """
    value = getattr(device, attr, None)
    if isinstance(value, bool):
        return fallback
    if isinstance(value, (int, float)) and value > 0:
        return int(value)
    return fallback


def _pick_speaker(sc, device: str | None):
    """按名字挑一个扬声器；`device` 为空就用默认的。"""
    if device:
        for candidate in sc.all_speakers():
            if str(candidate.name) == device:
                return candidate
        raise RuntimeError(f"找不到扬声器 {device!r}（可能已拔出/改名）")
    return sc.default_speaker()


def _pick_output_index(sd, device: str | None) -> int:
    """Windows：按名字挑一个**输出**设备的下标（WASAPI 回环抓它）；空则默认输出。"""
    if device:
        for index, info in enumerate(sd.query_devices()):
            if str(info.get("name")) == device and info.get("max_output_channels", 0) > 0:
                return index
        raise RuntimeError(f"找不到输出设备 {device!r}")
    return int(sd.default.device[1])


def _pick_monitor_index(sd, device: str | None) -> int:
    """非 Windows：按名字挑设备；空则挑第一个 monitor / loopback 输入。"""
    if device:
        for index, info in enumerate(sd.query_devices()):
            if str(info.get("name")) == device:
                return index
        raise RuntimeError(f"找不到设备 {device!r}")
    for index, info in enumerate(sd.query_devices()):
        name = str(info.get("name", "")).lower()
        if info.get("max_input_channels", 0) > 0 and ("monitor" in name or "loopback" in name):
            return index
    raise RuntimeError("没有找到 monitor / loopback 输入设备")


class _SoundCardSource:
    """`soundcard` 回环：抓某个扬声器在放什么（默认那个放什么）。

    ⚠ **尽量按设备自己的格式打开**（采样率 / 通道数）：强行指定一个不匹配的格式
      （尤其是把立体声设备当单声道）会让某些设备（**蓝牙耳机**最典型）在打开的那一瞬间
      重新协商格式 —— 表现就是"一启动音乐就停了/切到通话模式"。
    """

    def __init__(self, sample_rate: int, block: int, device: str | None = None) -> None:
        import soundcard as sc

        speaker = _pick_speaker(sc, device)
        rate = _device_int(speaker, "samplerate", sample_rate)
        channels = max(1, min(2, _device_int(speaker, "channels", 2)))
        mic = sc.get_microphone(id=str(speaker.name), include_loopback=True)
        if mic is None:
            raise RuntimeError(f"找不到 {speaker.name} 的回环设备")
        self._recorder = mic.recorder(samplerate=rate, channels=channels, blocksize=block)
        self._recorder.__enter__()
        self.samplerate = rate
        self.name = f"soundcard · {speaker.name}（回环 {rate}Hz/{channels}ch）"

    def read(self, frames: int):
        return self._recorder.record(numframes=frames)

    def close(self) -> None:
        try:
            self._recorder.__exit__(None, None, None)
        except Exception:                        # noqa: BLE001
            pass


class _SoundDeviceSource:
    """`sounddevice` 回环：Windows 走 WASAPI loopback，其它平台挑 monitor 设备。"""

    def __init__(self, sample_rate: int, block: int, device: str | None = None) -> None:
        import sounddevice as sd

        extra = None
        if sys.platform == "win32":
            extra = sd.WasapiSettings(loopback=True)   # type: ignore[attr-defined]
            index = _pick_output_index(sd, device)
            label = device or "默认扬声器"
        else:
            index = _pick_monitor_index(sd, device)
            label = str(sd.query_devices(index)["name"])

        # 用**设备自己的**默认采样率（拿不到才退回传入值）—— 避免重新协商格式
        try:
            rate = int(sd.query_devices(index)["default_samplerate"]) or sample_rate
        except Exception:                            # noqa: BLE001
            rate = sample_rate
        channels = 2                                 # 立体声；FFT 前会平均成单路

        self._stream = sd.RawInputStream(
            samplerate=rate, blocksize=block, channels=channels, dtype="float32",
            device=index, extra_settings=extra)
        self._stream.start()
        import numpy as np
        self._np = np
        self.samplerate = rate
        self.name = f"sounddevice · {label}（回环 {rate}Hz/{channels}ch）"

    def read(self, frames: int):
        raw, _overflowed = self._stream.read(frames)
        return self._np.frombuffer(raw, dtype="float32")

    def close(self) -> None:
        try:
            self._stream.stop()
            self._stream.close()
        except Exception:                        # noqa: BLE001
            pass


def list_devices() -> list[dict[str, str]]:
    """枚举可捕获的设备（回环 / monitor），给配置页的下拉用。

    每项：`{"key": "soundcard:<名字>", "label": "..."}` —— 存进配置的就是 `key`。
    一个后端都不可用就返回空列表（配置页会提示"自动"）。
    """
    out: list[dict[str, str]] = []
    seen: set[str] = set()

    def _add(key: str, label: str) -> None:
        if key in seen:
            return
        seen.add(key)
        out.append({"key": key, "label": label})

    try:
        import soundcard as sc
        for speaker in sc.all_speakers():
            name = str(speaker.name)
            _add(f"soundcard:{name}", f"{name}（回环 · soundcard）")
    except Exception:                            # noqa: BLE001
        pass

    try:
        import sounddevice as sd
        if sys.platform == "win32":
            for info in sd.query_devices():
                if info.get("max_output_channels", 0) > 0:
                    name = str(info.get("name"))
                    _add(f"sounddevice:{name}", f"{name}（回环 · WASAPI）")
        else:
            for info in sd.query_devices():
                name = str(info.get("name", ""))
                if info.get("max_input_channels", 0) > 0 and (
                        "monitor" in name.lower() or "loopback" in name.lower()):
                    _add(f"sounddevice:{name}", f"{name}（monitor）")
    except Exception:                            # noqa: BLE001
        pass

    return out


def _open_source(sample_rate: int, block: int, device: str = "auto") -> _Source | None:
    """打开捕获源。

    * `device == "auto"`（默认）：依次尝试 soundcard → sounddevice，抓**默认**输出设备；
    * 否则按 `"<backend>:<名字>"` 打开**指定的**那一个；**打不开就退回 auto**（而不是直接 demo）。
    """
    errors: list[str] = []

    key = (device or "auto").strip()
    if key and key != "auto":
        backend, _, name = key.partition(":")
        try:
            if backend == "soundcard":
                return _SoundCardSource(sample_rate, block, device=name)
            if backend == "sounddevice":
                return _SoundDeviceSource(sample_rate, block, device=name)
        except Exception as exc:                 # noqa: BLE001
            errors.append(f"指定设备 {key}：{exc}")
            log.warn(f"指定设备打不开（{key}：{exc}）—— 退回自动捕获")

    for module, factory in (("soundcard", _SoundCardSource), ("sounddevice", _SoundDeviceSource)):
        try:
            import importlib.util
            if importlib.util.find_spec(module) is None:
                continue
        except Exception:                        # noqa: BLE001
            continue
        try:
            return factory(sample_rate, block)
        except Exception as exc:                 # noqa: BLE001 - 任何一种失败都往下退
            errors.append(f"{module}: {exc}")

    if errors:
        log.warn("回环音频打不开，改用 demo 模式 —— " + "；".join(errors))
    else:
        log.warn("没装回环音频库（soundcard / sounddevice），改用 demo 模式")
    return None


# ==========================================================================
# 引擎
# ==========================================================================

class AudioEngine:
    """后台线程：抓音频 → 分析 → 存下"最新一帧"，供浮层随时取。

    用法：`engine.start()` 起线程，`engine.snapshot()` 取当前频段，
    配置改了数量就调 `engine.reconfigure(count)`，退出时 `engine.stop()`。
    """

    def __init__(self, count: int, *, sample_rate: int = SAMPLE_RATE,
                 fft_size: int = FFT_SIZE, f_min: float = F_MIN, f_max: float = F_MAX,
                 source: str = "system", delay_ms: int = 0, device: str = "auto",
                 gains: tuple[float, float, float] = (1.0, 1.0, 1.0)) -> None:
        self.sample_rate = sample_rate
        self.fft_size = fft_size
        self.f_min = f_min
        self.f_max = f_max
        #: 音频来源（`system` / `off`）、启动延迟（毫秒）、捕获设备（`auto` 或 `backend:名字`）
        self.source_setting = source if source in ("system", "off") else "system"
        self.delay_ms = max(0, int(delay_ms))
        self.device_setting = (device or "auto").strip() or "auto"
        self._count = count
        self._ranges = band_bins(count, sample_rate, fft_size, f_min, f_max)
        self._processor = SpectrumProcessor(count, gains=gains)

        self._bands = [0.0] * count
        self._live = False
        self._source_name = "demo"
        self._lock = threading.Lock()

        self._thread: threading.Thread | None = None
        self._started = False
        self._stop = threading.Event()
        #: demo 模式用的漂移能量团（固定一条，看起来才连贯）
        rng = random.Random(20261010)
        self._demo_blobs = [
            (rng.random(), 0.03 + rng.random() * 0.05, 0.05 + rng.random() * 0.05)
            for _ in range(5)
        ]

    # ------------------------------------------------------------------ 生命周期
    def start(self) -> None:
        if self._thread is not None and self._thread.is_alive():
            return
        self._stop.clear()
        self._started = True
        self._thread = threading.Thread(target=self._run, name="visualspectrum-audio",
                                        daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._started = False
        thread = self._thread
        if thread is not None:
            thread.join(timeout=1.5)
        self._thread = None

    # ------------------------------------------------------------------ 配置
    def apply_settings(self, *, source: str | None = None, delay_ms: int | None = None,
                       device: str | None = None,
                       gains: tuple[float, float, float] | None = None) -> None:
        """来源 / 延迟 / 设备 / 低中高占比改了。**来源或设备改了要重开捕获**（正在跑的时候）。"""
        changed = False
        if source is not None and source in ("system", "off") and source != self.source_setting:
            self.source_setting = source
            changed = True
        if delay_ms is not None:
            self.delay_ms = max(0, int(delay_ms))
        if device is not None:
            wanted = (device or "auto").strip() or "auto"
            if wanted != self.device_setting:
                self.device_setting = wanted
                changed = True
        if gains is not None:
            self._processor.set_gains(gains)      # 占比：只换增益，不用重开捕获
        if changed and self._started:
            log.info(f"音频设置变了（来源 {self.source_setting} · 设备 {self.device_setting}）—— 重开捕获")
            self.stop()
            self.start()

    def reconfigure(self, count: int) -> None:
        """数量改了 —— 换一套频段划分并清空状态（下一帧就按新数量出）。"""
        if count == self._count:
            return
        with self._lock:
            self._count = count
            self._ranges = band_bins(count, self.sample_rate, self.fft_size, self.f_min, self.f_max)
            self._processor.reset(count)
            self._bands = [0.0] * count

    # ------------------------------------------------------------------ 取数
    def snapshot(self) -> dict[str, object]:
        """当前这一帧：`{"bands": [...], "live": bool, "source": 名字}`。"""
        with self._lock:
            return {"bands": list(self._bands), "live": self._live, "source": self._source_name}

    # ------------------------------------------------------------------ 线程
    def _run(self) -> None:
        # 1) 配置里关掉了捕获 → 直接 demo（一根音频设备都不碰）
        if self.source_setting == "off":
            self._source_name = "demo（已关闭捕获）"
            self._live = False
            log.info("音频来源：demo（配置里关闭了系统声音捕获）")
            self._run_demo()
            return

        # 2) 启动延迟：错开"启动那一瞬间"对音频设备的抢占
        delay = max(0, int(self.delay_ms)) / 1000.0
        if delay > 0:
            log.debug(f"等 {int(self.delay_ms)}ms 后再打开音频设备")
            if self._stop.wait(delay):
                return

        # 3) 打开捕获源
        source: _Source | None = None
        try:
            source = _open_source(self.sample_rate, self.fft_size, self.device_setting)
        except Exception as exc:                 # noqa: BLE001
            log.warn(f"音频后端初始化时异常，改用 demo 模式：{exc}")
            source = None

        if source is not None:
            # 设备自己的采样率可能与默认不同 —— 跟着它走，否则频段划分会错位
            rate = int(getattr(source, "samplerate", self.sample_rate) or self.sample_rate)
            if rate != self.sample_rate:
                with self._lock:
                    self.sample_rate = rate
                    self._ranges = band_bins(self._count, rate, self.fft_size,
                                             self.f_min, self.f_max)
            self._source_name = source.name
            self._live = True
            log.info(f"音频来源：{source.name}")
            try:
                self._run_real(source)
            except Exception as exc:             # noqa: BLE001 - 跑着跑着断了也要能退 demo
                log.warn(f"音频抓取中断（{exc}）—— 改用 demo 模式")
            finally:
                source.close()

        # 4) 真源不可用 / 断掉 → demo（保证画面一直是活的）
        if not self._stop.is_set():
            self._source_name = "demo（合成）"
            self._live = False
            log.info("音频来源：demo（合成频谱）")
            self._run_demo()

    def _run_real(self, source: _Source) -> None:
        while not self._stop.is_set():
            block = source.read(self.fft_size)
            with self._lock:
                ranges = self._ranges
            levels = self._processor.process(bands_of(block, sample_rate=self.sample_rate,
                                                      fft_size=self.fft_size, ranges=ranges))
            self._store(levels)

    def _run_demo(self) -> None:
        rng = random.Random()
        start = time.monotonic()
        interval = 1.0 / DEMO_FPS
        while not self._stop.is_set():
            t = time.monotonic() - start
            with self._lock:
                count = self._count
            raw = demo_frame(t, count, self._demo_blobs, rng)
            self._store(self._processor.process(raw))
            self._stop.wait(interval)

    def _store(self, levels: list[float]) -> None:
        with self._lock:
            self._bands = levels
