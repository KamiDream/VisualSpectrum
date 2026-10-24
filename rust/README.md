# VisualSpectrum · Rust workspace

原生渲染实现（winit 0.30 + wgpu 30）。用户向的说明见仓库根目录的 `README.md`。

## 结构

```
crates/core     vs-core    配置 schema / 校验 / 原子读写 / 热重载；FFT 与频段映射
crates/audio    vs-audio   parec(monitor) / cpal 捕获 → FFT → 频段（失败退 demo）
crates/scene    vs-scene   几何 / 色标 / 粒子 / 鼓点 / 自转 / 顶端连线
crates/server   vs-server  axum：/api/* + 配置页静态托管
crates/app      vs-app     窗口 + wgpu 渲染 + 托盘 + 单实例 + 配置热重载
```

`core` / `audio` / `scene` 都不依赖窗口与 GPU，可在无显示器环境完整单测。

## 命令

```bash
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo run -p vs-app
cargo build --release -p vs-app   # 单文件发布版（web 已嵌入）
sh install.sh                     # 安装到 ~/.local/bin + 桌面入口
```

`web/` 在编译时嵌入（`vs-server/build.rs` 跟踪整个目录）。开发时不需要重编译：从仓库根运行
或设 `VISUALSPECTRUM_WEB` 会优先读磁盘；发布前重新 `cargo build --release` 即可。

## 渲染管线

1. **场景**：频谱条 → 顶端连线 → 散射粒子 → 中心（同心环 / X 油光 / 色标标签），画进离屏纹理；
2. **余晖**：双缓冲 ping-pong，上一帧 × decay 铺底（带量化地板，避免 8-bit 残影卡在低亮度）；
3. **bloom**：1/4 分辨率两趟可分离高斯；
4. **CRT 合成**：桶形畸变 + RGB 分色 + bloom + 背景，写入窗口。

内部渲染分辨率在软件适配器下自动降为 0.5 倍；`high_fps=false` 锁 60fps，`true` 锁 120fps。

## 音频

`auto` 时抓**默认输出的 `.monitor`**（`parec`），每 2 秒检查默认 sink 是否变化并自动重连；
`pulse:<源名>` 可显式指定；没有 pulse 工具时退回 cpal（注意：cpal 在 PipeWire 下通常连的是麦克风）。
