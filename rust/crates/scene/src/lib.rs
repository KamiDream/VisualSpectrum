//! 频谱**画什么**：几何、色标、平滑、粒子池、鼓点判定、自转 —— 一个纯状态机。
//!
//! `vs-app`（wgpu）只消费 [`Scene::bars`] / [`Scene::links`] / [`Scene::particles`] /
//! [`Scene::geometry`]，这些都可以单测 —— 不依赖 GPU / 窗口。

mod scene;

pub use scene::{
    afterglow_rate, geometry, palette, parse_hex, Bar, Geometry, Particle, Scene, Viewport,
};
