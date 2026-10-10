"""音频分析：频段划分 / 聚合 / 平滑 / demo 波形。

不依赖 numpy 的部分一定会跑；需要 numpy 的那条（`bands_of`）在没有 numpy 时跳过。
"""

from __future__ import annotations

import random

import pytest

from app.audio import (
    FFT_SIZE,
    SAMPLE_RATE,
    SpectrumProcessor,
    band_bins,
    bands_of,
    demo_frame,
    map_bands,
    region_bounds,
)

# -------------------------------------------------------------------------- band_bins

def test_band_bins_count_matches() -> None:
    for count in (16, 48, 96, 256):
        assert len(band_bins(count)) == count


def test_band_bins_are_monotonic_and_cover() -> None:
    ranges = band_bins(64)
    bins = FFT_SIZE // 2 + 1
    prev_hi = 0
    for lo, hi in ranges:
        assert 0 <= lo < hi <= bins
        assert lo >= prev_hi - 1            # 允许相邻段重叠 1 个 bin（对数划分难免）
        prev_hi = hi
    # 第一段从最低频开始（跳过 DC —— F_MIN 略高于 0 是刻意的）
    assert ranges[0][0] <= 1
    # 最后一段落在 F_MAX（16kHz）附近，而不是铺到奈奎斯特
    assert 600 <= ranges[-1][1] < bins


def test_band_bins_each_has_at_least_one() -> None:
    for lo, hi in band_bins(256):
        assert hi - lo >= 1


def test_band_bins_rejects_zero() -> None:
    with pytest.raises(ValueError):
        band_bins(0)


# -------------------------------------------------------------------------- map_bands

def test_map_bands_averages() -> None:
    magnitudes = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
    assert map_bands(magnitudes, [(0, 2), (2, 4), (4, 6)]) == [1.5, 3.5, 5.5]


def test_map_bands_empty_segment() -> None:
    assert map_bands([], [(0, 1)]) == [0.0]


# -------------------------------------------------------------------------- SpectrumProcessor

def test_processor_normalizes_and_bounds() -> None:
    proc = SpectrumProcessor(4)
    out = proc.process([0.0, 0.5, 1.0, 2.0])
    assert len(out) == 4
    assert all(0.0 <= value <= 1.0 for value in out)
    assert out[3] > out[0]                  # 大的更大


def test_processor_reference_adapts() -> None:
    proc = SpectrumProcessor(3)
    # 第一次给很小的值，参考线也会跟上（自动增益）
    small = proc.process([0.001, 0.002, 0.001])
    big = proc.process([1.0, 2.0, 1.0])
    assert max(big) > 0.5                    # 不管绝对幅度多少，都能填满
    assert max(small) >= 0.0


def test_processor_release_is_slower_than_attack() -> None:
    proc = SpectrumProcessor(2)
    proc.process([1.0, 1.0])                 # 冲上去
    up = proc.values[0]
    proc.process([0.0, 0.0])                 # 回落一帧
    assert proc.values[0] > 0.0
    assert proc.values[0] > up * 0.5         # 回得很慢（释放）


def test_processor_reset_on_count_change() -> None:
    proc = SpectrumProcessor(4)
    proc.process([1.0, 1.0, 1.0, 1.0])
    proc.process([0.0, 0.0, 0.0, 0.0, 0.0])  # 长度变了 → 重置成 5
    assert proc.count == 5
    assert len(proc.values) == 5


# -------------------------------------------------------------------------- 低/中/高分区

def test_region_bounds_cover_and_are_nonempty() -> None:
    for count in (4, 8, 96, 512):
        regions = region_bounds(count)
        assert [region for _, _, region in regions] == [0, 1, 2]
        assert regions[0][0] == 0 and regions[-1][1] == count
        for lo, hi, _ in regions:
            assert hi > lo                      # 每块非空


def test_gains_scale_the_region() -> None:
    proc = SpectrumProcessor(9)
    for _ in range(6):
        proc.process([1.0] * 9)
    base = proc.process([1.0] * 9)
    proc.set_gains((0.5, 1.0, 1.0))            # 低音占比减半
    lowered = proc.process([1.0] * 9)
    assert lowered[0] < base[0]                 # 低音块确实被压下来了
    assert lowered[-1] >= base[-1] - 1e-9       # 高音块不受影响


def test_bass_has_dynamics_not_pegged() -> None:
    """低音不再"永远打满"：分区域归一化后，低音强弱交替时电平真的会起伏。"""
    proc = SpectrumProcessor(12)

    def frame(strong: bool) -> list[float]:
        bands = [0.05] * 12
        for i in range(0, 4):                   # 低音区（前 4 个频段）
            bands[i] = 1.0 if strong else 0.15
        return bands

    for _ in range(4):
        proc.process(frame(True))
    loud = proc.process(frame(True))[0]
    for _ in range(3):
        proc.process(frame(False))
    quiet = proc.process(frame(False))[0]
    assert loud > 0.9
    assert quiet < loud - 0.15                  # 有起伏, 不是一直顶在 1.0


# -------------------------------------------------------------------------- demo_frame

def test_demo_frame_bounds() -> None:
    rng = random.Random(1)
    blobs = [(0.1, 0.05, 0.05), (0.6, -0.04, 0.06)]
    frame = demo_frame(0.3, 96, blobs, rng)
    assert len(frame) == 96
    assert all(value >= 0.0 for value in frame)
    assert max(frame) > 0.3                  # 低频鼓点应当有明显能量


# -------------------------------------------------------------------------- bands_of（需 numpy）

def test_bands_of_sine_peaks_in_right_band() -> None:
    np = pytest.importorskip("numpy")
    count = 64
    ranges = band_bins(count, sample_rate=SAMPLE_RATE, fft_size=FFT_SIZE)
    t = np.arange(FFT_SIZE) / SAMPLE_RATE
    tone = np.sin(2 * np.pi * 1000.0 * t).astype(np.float32)   # 1kHz 正弦
    levels = bands_of(tone, sample_rate=SAMPLE_RATE, fft_size=FFT_SIZE, ranges=ranges)
    assert len(levels) == count
    assert levels.index(max(levels)) > count // 3             # 峰值不该在最低频那头
