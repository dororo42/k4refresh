#!/bin/sh
# k4refresh 交叉构建脚本。
# 用法:
#   ./build.sh                       # 默认 musl 静态 target（首选）
#   ./build.sh gnueabihf             # 回退 glibc 动态 target（需外部交叉 gcc）
#   ./build.sh host                  # 仅本机自测（cargo test 用，产物不部署）
set -e

TARGET_MUSL="armv7-unknown-linux-musleabihf"
TARGET_GNU="armv7-unknown-linux-gnueabihf"

case "${1:-musl}" in
  musl)
    rustup target add "$TARGET_MUSL"
    cargo build --release --target "$TARGET_MUSL"
    echo
    echo "产物:"
    ls -lh "target/$TARGET_MUSL/release/k4refresh-cli"
    ls -lh "target/$TARGET_MUSL/release/libk4refresh.so"
    echo
    echo "静态链接自检（预期: not a dynamic executable / statically linked）:"
    file "target/$TARGET_MUSL/release/k4refresh-cli"
    ;;
  gnueabihf)
    rustup target add "$TARGET_GNU"
    cargo build --release --target "$TARGET_GNU"
    ls -lh "target/$TARGET_GNU/release/k4refresh-cli" "target/$TARGET_GNU/release/libk4refresh.so"
    ;;
  host)
    cargo test
    cargo build --release
    ;;
  *)
    echo "unknown target: $1" >&2
    exit 2
    ;;
esac
