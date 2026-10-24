#!/usr/bin/env sh
# 构建并安装到当前用户目录（单文件二进制，配置页已嵌入）。
set -eu

cd "$(dirname "$0")"

cargo build --release -p vs-app

BIN_DIR="${HOME}/.local/bin"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_DIR="${DATA_DIR}/applications"
ICON_DIR="${DATA_DIR}/icons/hicolor/scalable/apps"

install -Dm755 target/release/visualspectrum "${BIN_DIR}/visualspectrum"
install -Dm644 visualspectrum.desktop "${APP_DIR}/visualspectrum.desktop"
install -Dm644 ../assets/icon.svg "${ICON_DIR}/visualspectrum.svg"

echo "已安装：${BIN_DIR}/visualspectrum"
echo "配置页：http://127.0.0.1:8770/"
echo "（如果 ${BIN_DIR} 不在 PATH 里，请自行加一下）"
