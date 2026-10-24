//! 系统托盘（freedesktop StatusNotifierItem）。
//!
//! 行为按用户要求定死：
//! * **左键点击 = 显示可视化**（永远显示，不做“切换/隐藏”）；
//! * 右键菜单：显示可视化 / 打开配置页 / 退出。
//!
//! niri 本身没有托盘栏，需要状态栏（waybar 等）支持 SNI 才会显示图标；
//! 没有宿主时注册会失败，这里只打印日志，不影响本体运行。
//!
//! 图标是现画的 32×32 ARGB 像素（青→紫→品红的同心环 + 中心点），不依赖外部图片资源。

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
use ksni::{Category, Icon, MenuItem};

pub type TrayHandle = Handle<VisualSpectrumTray>;

/// 托盘回调与主线程共享的两个开关。
struct TrayShared {
    overlay_visible: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
}

pub struct VisualSpectrumTray {
    shared: TrayShared,
}

impl ksni::Tray for VisualSpectrumTray {
    fn id(&self) -> String {
        "visualspectrum".to_string()
    }

    fn title(&self) -> String {
        "VisualSpectrum".to_string()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![render_icon(32)]
    }

    /// 左键点击：**只显示**，不隐藏。
    fn activate(&mut self, _x: i32, _y: i32) {
        self.shared.overlay_visible.store(true, Ordering::Relaxed);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "显示可视化".to_string(),
                activate: Box::new(|tray: &mut Self| {
                    tray.shared.overlay_visible.store(true, Ordering::Relaxed);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "打开配置页".to_string(),
                activate: Box::new(|_| open_config_page()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "退出".to_string(),
                activate: Box::new(|tray: &mut Self| {
                    tray.shared.quit.store(true, Ordering::Relaxed);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// 注册托盘。`None` = 当前桌面没有 SNI 宿主（不是错误，只影响图标可见性）。
pub fn spawn(overlay_visible: Arc<AtomicBool>, quit: Arc<AtomicBool>) -> Option<TrayHandle> {
    let tray = VisualSpectrumTray {
        shared: TrayShared {
            overlay_visible,
            quit,
        },
    };
    // `assume_sni_available(true)`：状态栏晚于程序启动时也能在它上线后出现。
    match tray.assume_sni_available(true).spawn() {
        Ok(handle) => {
            eprintln!("托盘图标已注册");
            Some(handle)
        }
        Err(err) => {
            eprintln!("托盘不可用（{err}）—— 需要支持 StatusNotifierItem 的状态栏");
            None
        }
    }
}

fn open_config_page() {
    let url = format!("http://127.0.0.1:{}/", vs_server::DEFAULT_PORT);
    match Command::new("xdg-open").arg(&url).spawn() {
        Ok(_) => eprintln!("正在打开配置页：{url}"),
        Err(err) => eprintln!("打开配置页失败：{err}（手动访问 {url}）"),
    }
}

// ==========================================================================
// 图标
// ==========================================================================

fn render_icon(size: u32) -> Icon {
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let half = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - half;
            let dy = y as f32 + 0.5 - half;
            let d = (dx * dx + dy * dy).sqrt();

            // 三圈同心环 + 中心点（和可视化中心一个意思）。
            let outer = band(d, half * 0.78, 1.3);
            let middle = band(d, half * 0.52, 1.15) * 0.85;
            let inner = band(d, half * 0.30, 1.0) * 0.7;
            let dot = 1.0 - smoothstep(2.4, 3.6, d);
            let alpha = outer.max(middle).max(inner).max(dot);
            let (r, g, b) = gradient((d / half).clamp(0.0, 1.0));

            // StatusNotifierItem 的 IconPixmap：ARGB32，network byte order（A 在最前）。
            data.extend_from_slice(&[(alpha * 255.0).round() as u8, r, g, b]);
        }
    }
    Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

fn band(distance: f32, radius: f32, width: f32) -> f32 {
    let t = 1.0 - ((distance - radius).abs() / width).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 注意参数顺序是「内半径, 外半径」：distance 小于 `outer` 时开始出现。
fn smoothstep(inner: f32, outer: f32, value: f32) -> f32 {
    let t = ((value - inner) / (outer - inner)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 青 → 紫 → 品红，和默认色标一致。
fn gradient(t: f32) -> (u8, u8, u8) {
    const STOPS: [(f32, f32, f32); 3] = [
        (126.0, 231.0, 255.0),
        (179.0, 123.0, 255.0),
        (255.0, 111.0, 216.0),
    ];
    let pos = (t * 2.0).clamp(0.0, 2.0);
    let index = (pos.floor() as usize).min(1);
    let frac = pos - index as f32;
    let a = STOPS[index];
    let b = STOPS[index + 1];
    let mix = |x: f32, y: f32| (x + (y - x) * frac).round().clamp(0.0, 255.0) as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_argb_pixels() {
        let icon = render_icon(32);
        assert_eq!(icon.width, 32);
        assert_eq!(icon.height, 32);
        assert_eq!(icon.data.len(), 32 * 32 * 4);
        // 中心点不透明，四角透明。
        let center = ((16 * 32) + 16) * 4;
        assert!(icon.data[center] > 200, "中心应该有实心点");
        assert_eq!(icon.data[0], 0, "左上角应当透明");
    }
}
