//! 让 Cargo 知道二进制里嵌了 `web/`：目录里任何文件变化（含新增/删除）都重编译。
//!
//! `include_dir!` 依赖 rustc 的 dep-info 只能跟踪**已存在**的文件；新增文件不会自动触发
//! 重编译，所以这里显式声明整个目录。路径相对本 crate 目录：`rust/crates/server` → 仓库根 `web/`。

fn main() {
    println!("cargo:rerun-if-changed=../../../web");
}
