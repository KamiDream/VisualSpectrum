//! 设备枚举与挑选。

use cpal::traits::{DeviceTrait, HostTrait};

use crate::EngineError;

/// 一个可捕获的设备。`key` 形如 `cpal:<设备名>`（存进配置的就是它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub key: String,
    pub label: String,
}

fn is_monitor(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("monitor") || lower.contains("loopback")
}

/// 枚举输入设备，给配置页的下拉用。
///
/// 回环 / monitor 排在前面并标注出来 —— 大多数用户要的就是它。
/// 枚举可捕获的设备。
///
/// **Linux 上优先列 PulseAudio / PipeWire 的源**：只有那里的 `*.monitor` 才是"系统正在播放
/// 的声音"；cpal 走 ALSA 时默认连的是**麦克风**（这就是"音乐在放但频谱不动"的原因）。
/// 拿不到 pulse 工具时退回 cpal。
pub fn list_devices() -> Vec<DeviceInfo> {
    let pulse = pulse_device_infos();
    if !pulse.is_empty() {
        return pulse;
    }
    cpal_device_infos()
}

fn cpal_device_infos() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let mut monitors = Vec::new();
    let mut inputs = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            let Ok(name) = device.name() else { continue };
            let entry = if is_monitor(&name) {
                DeviceInfo {
                    key: format!("cpal:{name}"),
                    label: format!("{name}（回环 · cpal）"),
                }
            } else {
                DeviceInfo {
                    key: format!("cpal:{name}"),
                    label: format!("{name}（输入 · cpal）"),
                }
            };
            if is_monitor(&name) {
                monitors.push(entry);
            } else {
                inputs.push(entry);
            }
        }
    }
    monitors.extend(inputs);
    monitors
}

fn pulse_device_infos() -> Vec<DeviceInfo> {
    if !program_exists("pactl") {
        return Vec::new();
    }
    let mut monitors = Vec::new();
    let mut inputs = Vec::new();
    for name in pulse_sources() {
        let entry = DeviceInfo {
            label: label_for_pulse(&name),
            key: format!("pulse:{name}"),
        };
        if name.ends_with(".monitor") {
            monitors.push(entry);
        } else {
            inputs.push(entry);
        }
    }
    monitors.extend(inputs);
    monitors
}

/// `pactl list short sources` 里的源名字（制表符分隔的第 2 列）。
pub(crate) fn pulse_sources() -> Vec<String> {
    let Ok(output) = std::process::Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1).map(str::to_string))
        .filter(|name| !name.is_empty())
        .collect()
}

/// 默认输出的 monitor 源（`<default-sink>.monitor`）。没有 `parec` 或拿不到 sink 就返回 `None`。
pub(crate) fn default_monitor_source() -> Option<String> {
    if !program_exists("parec") {
        return None;
    }
    if let Ok(output) = std::process::Command::new("pactl")
        .args(["get-default-sink"])
        .output()
    {
        if output.status.success() {
            let sink = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !sink.is_empty() {
                return Some(format!("{sink}.monitor"));
            }
        }
    }
    pulse_sources()
        .into_iter()
        .find(|name| name.ends_with(".monitor"))
}

fn program_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(name))
                .any(|path| path.is_file())
        })
        .unwrap_or(false)
}

fn label_for_pulse(name: &str) -> String {
    if name.ends_with(".monitor") {
        format!("{name}（回环 · pulse）")
    } else {
        format!("{name}（输入 · pulse）")
    }
}

/// 挑设备：指定名字优先；`auto` 则优先回环 / monitor，其次默认输入。
///
/// ⚠ 指定设备**找不到就退回 auto** —— 设备可能刚被拔掉/改名，
/// 不该因此让整条链路起不来。
pub fn pick_device(wanted: &str) -> Result<cpal::Device, EngineError> {
    let host = cpal::default_host();
    let want = wanted.strip_prefix("cpal:").unwrap_or(wanted);

    if wanted != "auto" && !want.is_empty() {
        if let Ok(devices) = host.input_devices() {
            for device in devices {
                if device.name().map(|name| name == want).unwrap_or(false) {
                    return Ok(device);
                }
            }
        }
    }

    let mut fallback: Option<cpal::Device> = None;
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if is_monitor(&device.name().unwrap_or_default()) {
                return Ok(device);
            }
            if fallback.is_none() {
                fallback = Some(device);
            }
        }
    }
    fallback
        .or_else(|| host.default_input_device())
        .ok_or_else(|| EngineError::NoDevice("没有可用的输入设备".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_labels_mark_monitors() {
        assert!(label_for_pulse("alsa_output.x.monitor").contains("回环"));
        assert!(label_for_pulse("alsa_input.x").contains("输入"));
    }
}
