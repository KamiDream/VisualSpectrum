# VisualSpectrum · 环形频谱可视化

把**系统正在播放的声音**变成桌面上一圈随音乐起伏的**环形频谱**：荧光辉光、余晖拖尾、从条顶端逸出的散射粒子、中心一圈套一圈的同心环（不填充）+ X 形油光。

**纯 Rust 原生渲染**（winit + wgpu），不再依赖 Python / QtWebEngine。浏览器只用来打开本地配置页。

---

## 功能

- 环形频谱：条数 / 大小 / 条高 / 强度 / 低中高占比 / 顶端连线 / 自转（含速度）
- 视觉：bloom 辉光、CRT（桶形畸变 + RGB 分色 + 余晖）、中心同心环与 X 形油光、每频率自定义色标
- 粒子：散射粒子 + 鼓点全屏爆发（速度 / 寿命 / 灵敏度）
- 音频：优先抓**系统默认输出的 monitor**（PulseAudio / PipeWire，经 `parec`），设备切换自动跟随；失败退 cpal / demo 波形
- 窗口：无边框、鼠标穿透、按模式给置顶/置底提示
- 常驻：单实例（再启动 = 显示窗口）、关闭窗口 = 销毁窗口但保留音频、托盘图标（SNI）
- 配置：本地 HTTP 配置页 + 配置文件热重载；保存后立即生效，无需重启

## 环境要求（Linux）

| 依赖 | 说明 |
|---|---|
| Vulkan 驱动 | Intel 核显请装 `vulkan-intel`；没有硬件驱动时自动退回软件渲染（会自动降低内部渲染分辨率） |
| `parec` / `pactl` | PulseAudio / PipeWire 工具（Arch: `libpulse` 或 `pulseaudio-utils`），用于抓系统回环 |
| 状态栏（可选） | 支持 StatusNotifierItem 的状态栏（waybar / quickshell 等）才能看到托盘图标 |
| Wayland / X11 | 置顶/置底在 Wayland 下取决于合成器；鼠标穿透与无边框两端都可用 |

## 构建与运行

```bash
cd rust
cargo build --release -p vs-app
./target/release/visualspectrum
```

开发时用 `cargo run -p vs-app` 即可。

启动后：

- 可视化窗口出现（无边框、鼠标穿透）；
- 配置页：**http://127.0.0.1:8770/**（日志里也会打印）；
- 配置文件：`~/.config/visualspectrum/config.json`，编辑后 0.5 秒内自动生效。

## 打包 / 安装

发布版是**单个可执行文件**（`web/` 配置页在编译时已嵌入，不需要另带资源）：

```bash
cd rust
cargo build --release -p vs-app
ls -lh target/release/visualspectrum     # ≈ 15 MB
```

装到当前用户目录：

```bash
install -Dm755 target/release/visualspectrum ~/.local/bin/visualspectrum
# 可选：桌面菜单入口 + 图标
install -Dm644 visualspectrum.desktop ~/.local/share/applications/visualspectrum.desktop
install -Dm644 ../assets/icon.svg ~/.local/share/icons/hicolor/scalable/apps/visualspectrum.svg
```

或者直接 `sh install.sh`（做了上面三件事）。

说明：

- 运行时依赖：Vulkan 驱动（Intel 核显装 `vulkan-intel`）、`parec`/`pactl`、D-Bus（托盘）、支持 SNI 的状态栏（可选）；
- 配置页已嵌入。想**实时修改**前端文件时，用 `VISUALSPECTRUM_WEB=/path/to/web` 指向磁盘目录即可（磁盘优先，读不到才用嵌入副本）。

### 改了 `web/` 之后

- **开发时**：不用重编译。从仓库根运行程序（或设 `VISUALSPECTRUM_WEB`）会优先读磁盘上的 `web/`，改完刷新浏览器即可；
- **打包时**：重新执行 `cargo build --release -p vs-app`（或 `sh install.sh`）。`vs-server` 里有 `build.rs` 跟踪整个 `web/` 目录，**新增 / 修改 / 删除**前端文件都会触发重编译，不必手动 `clean`。

## 托盘与单实例

- 左键点托盘图标 = **显示可视化**（永远显示，不隐藏）；
- 右键菜单：显示可视化 / 打开配置页 / 退出；
- 关闭可视化窗口 = 隐藏（窗口销毁，进程与音频仍在）；再次运行程序、点托盘、或调用 `POST /api/overlay` 都能唤回；
- 同一份配置只允许一个实例。

## 目录

```
rust/
├── crates/core     vs-core    配置 schema / 校验 / 原子读写 / 热重载；FFT 与频段映射
├── crates/audio    vs-audio   parec(monitor) / cpal 捕获 → FFT → 频段
├── crates/scene    vs-scene   几何 / 色标 / 粒子 / 鼓点 / 自转 / 顶端连线
├── crates/server   vs-server  axum：/api/* + 配置页静态托管
└── crates/app      vs-app     winit 窗口 + wgpu 渲染 + 托盘 + 单实例
web/               配置页（原生 ES 模块，无构建步骤）
assets/            应用图标
```

## 配置接口

`/api/*` 与配置 JSON 字段一一对应：

- `GET /api/meta`、`GET|PUT /api/config`、`POST /api/config/validate|reset`、`GET /api/config/export`
- `POST /api/reload`、`GET /api/audio/devices`、`GET /api/spectrum`
- `POST /api/overlay`（`{"visible": true|false}`，不给 body 则切换）

所有响应都是 `{ok: true, ...}` 或 `{ok: false, error}`。

## 开发

```bash
cd rust
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

调试键（窗口有键盘焦点时）：`B` bloom、`C` CRT、`R` 自转、`Esc` 退出。

环境变量：

- `VISUALSPECTRUM_WEB`：配置页目录（默认自动向上查找 `web/`）
- `VISUALSPECTRUM_SCALE`：内部渲染倍数（默认软件渲染 0.5、硬件 1.0）
