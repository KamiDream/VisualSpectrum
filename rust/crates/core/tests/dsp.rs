//! DSP 纯逻辑的单元测试。

use vs_core::{
    band_bins, demo_frame, map_bands, region_bounds, DemoBlob, Rng, SpectrumProcessor, FFT_SIZE,
    SAMPLE_RATE,
};

#[test]
fn band_bins_are_contiguous_and_cover() {
    let ranges = band_bins(96, SAMPLE_RATE, FFT_SIZE, 30.0, 16_000.0);
    assert_eq!(ranges.len(), 96);
    let bins = FFT_SIZE / 2 + 1;
    let mut prev_lo = 0;
    for &(lo, hi) in &ranges {
        assert!(lo < hi, "每段至少 1 个 bin");
        assert!(hi <= bins, "hi 不能超过 bin 数");
        assert!(lo >= prev_lo, "lo 应当单调不减");
        prev_lo = lo;
    }
}

#[test]
fn band_bins_logarithmic_high_bands_are_wider() {
    let ranges = band_bins(64, SAMPLE_RATE, FFT_SIZE, 30.0, 16_000.0);
    let first = ranges[0].1 - ranges[0].0;
    let last = ranges[63].1 - ranges[63].0;
    assert!(
        last > first,
        "对数分频下高频段应更宽，实际 first={first} last={last}"
    );
}

#[test]
fn map_bands_averages_each_range() {
    let magnitudes = [1.0, 3.0, 5.0, 7.0];
    assert_eq!(map_bands(&magnitudes, &[(0, 2), (2, 4)]), vec![2.0, 6.0]);
}

#[test]
fn map_bands_tolerates_out_of_range() {
    let magnitudes = [2.0];
    assert_eq!(map_bands(&magnitudes, &[(0, 4)]), vec![2.0]);
    assert_eq!(map_bands(&magnitudes, &[(5, 9)]), vec![0.0]);
}

#[test]
fn region_bounds_split_low_mid_high() {
    assert_eq!(
        region_bounds(96),
        vec![(0, 33, 0), (33, 75, 1), (75, 96, 2)]
    );
    assert_eq!(region_bounds(2), vec![(0, 2, 0)]);
}

#[test]
fn processor_outputs_unit_range() {
    let mut processor = SpectrumProcessor::new(96, [1.0, 1.0, 1.0]);
    let out = processor.process(&[0.2; 96]);
    assert_eq!(out.len(), 96);
    assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
}

#[test]
fn processor_release_falls_slowly() {
    let mut processor = SpectrumProcessor::new(8, [1.0, 1.0, 1.0]);
    processor.process(&[1.0; 8]);
    let after = processor.process(&[0.0; 8]);
    // release = 0.30，一帧只回落 30%，不会直接掉到 0。
    assert!(
        after.iter().all(|v| *v > 0.0),
        "释放不应一帧归零：{after:?}"
    );
}

#[test]
fn processor_adapts_to_count_change() {
    let mut processor = SpectrumProcessor::new(8, [1.0, 1.0, 1.0]);
    let out = processor.process(&[0.1; 20]);
    assert_eq!(out.len(), 20);
}

#[test]
fn demo_frame_is_deterministic_and_bounded() {
    let blobs = [DemoBlob {
        phase: 0.1,
        speed: 0.04,
        width: 0.06,
    }];
    let mut first = Rng::new(42);
    let mut second = Rng::new(42);
    let a = demo_frame(0.5, 64, &blobs, &mut first);
    let b = demo_frame(0.5, 64, &blobs, &mut second);
    assert_eq!(a, b, "同种子应当完全一致");
    assert_eq!(a.len(), 64);
    assert!(a.iter().all(|v| v.is_finite() && *v >= 0.0));
}

#[test]
fn demo_frame_moves_with_time() {
    let blobs = [DemoBlob {
        phase: 0.1,
        speed: 0.4,
        width: 0.05,
    }];
    let mut rng = Rng::new(7);
    let early = demo_frame(0.0, 64, &blobs, &mut rng);
    let mut rng2 = Rng::new(7);
    let later = demo_frame(0.5, 64, &blobs, &mut rng2);
    assert_ne!(early, later, "时间不同，画面应当不同");
}
