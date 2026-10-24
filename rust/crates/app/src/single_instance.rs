//! 单实例 + "再点一次图标就显示" 的本地 IPC。
//!
//! 为什么需要它：可视化是常驻后台的，跑两份会叠出两圈频谱、白烧一份 CPU；
//! 而窗口关掉后进程仍在（窗口销毁、音频保留），用户总得有个手段把它叫回来。
//! 最稳的办法就是本地 Unix 套接字：第二次启动发现自己不是“第一个”，
//! 就把 `show` 发给第一个再退出。
//!
//! 用文件系统套接字而不是 D-Bus：不引入额外服务，行为简单：**连得上 = 已有一个在跑**。

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// `acquire()` 的两种结果。
pub enum Acquire {
    /// 本进程是唯一实例（句柄要一直活到进程结束）。
    Primary(SingleInstance),
    /// 已经有一个在跑，并且已经请求它显示可视化 —— 本进程应当直接退出。
    AlreadyRunning,
}

pub struct SingleInstance {
    /// 套接字文件；创建失败时为 `None`（此时只是没有"再点一次唤醒"，不影响主程序）。
    socket: Option<PathBuf>,
    show_requested: Arc<AtomicBool>,
}

impl SingleInstance {
    pub fn acquire() -> Acquire {
        let socket = socket_path();
        // 能连上 = 第一个实例还活着：发一条 show 就走人。
        if let Ok(mut stream) = UnixStream::connect(&socket) {
            let _ = stream.write_all(b"show\n");
            let _ = stream.flush();
            return Acquire::AlreadyRunning;
        }
        // 连不上：多半是上次崩溃留下的死文件，删掉重建。
        let _ = std::fs::remove_file(&socket);

        let listener = match bind(&socket) {
            Ok(listener) => listener,
            Err(err) => {
                eprintln!("单实例套接字创建失败（不影响运行）：{err}");
                return Acquire::Primary(Self {
                    socket: None,
                    show_requested: Arc::new(AtomicBool::new(false)),
                });
            }
        };

        let show_requested = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&show_requested);
        let _ = std::thread::Builder::new()
            .name("vs-ipc".to_string())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut buffer = [0_u8; 64];
                    let Ok(n) = (&stream).read(&mut buffer) else {
                        continue;
                    };
                    let message = String::from_utf8_lossy(&buffer[..n]);
                    if message.trim() == "show" {
                        flag.store(true, Ordering::SeqCst);
                    }
                }
            });

        Acquire::Primary(Self {
            socket: Some(socket),
            show_requested,
        })
    }

    /// 取走一次"请显示"的请求（取完自动清零）。
    pub fn take_show_request(&self) -> bool {
        self.show_requested.swap(false, Ordering::SeqCst)
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if let Some(socket) = &self.socket {
            let _ = std::fs::remove_file(socket);
        }
    }
}

fn bind(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    UnixListener::bind(path)
}

/// `$XDG_RUNTIME_DIR/visualspectrum.sock`，退而求其次放到配置目录旁边。
fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir).join("visualspectrum.sock");
        }
    }
    let config = vs_core::default_config_path();
    config
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"))
        .join("instance.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_is_absolute() {
        assert!(socket_path().is_absolute());
    }

    #[test]
    fn request_flag_roundtrip() {
        let instance = SingleInstance {
            socket: None,
            show_requested: Arc::new(AtomicBool::new(false)),
        };
        assert!(!instance.take_show_request());
        instance.show_requested.store(true, Ordering::SeqCst);
        assert!(instance.take_show_request());
        assert!(!instance.take_show_request());
    }
}
