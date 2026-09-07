#!/bin/sh
# KUAL 菜单扩展：k4refresh.toml
# 安装位置（Kindle USB 根目录）:
#   /mnt/us/extensions/k4refresh/bin/k4refresh-cli      ← 从构建产物复制
#   /mnt/us/extensions/k4refresh/lib/libk4refresh.so    ← 从构建产物复制
#   /mnt/us/extensions/k4refresh/bin/k4r.sh             ← 本脚本
#   /mnt/us/extensions/k4refresh/config.xml             ← KUAL 菜单注册（见下）
#
# 脚本逻辑全部是只读/刷新调用，不改任何系统文件；卸载 = 删除整个目录。

BIN=/mnt/us/extensions/k4refresh/bin/k4refresh-cli

msg() {
  # KUAL 环境下用 eips 打一行状态（2 行起，4 秒），脚本退出码不受影响
  eips 0 4 "$1" >/dev/null 2>&1
}

case "$1" in
  flash)
    "$BIN" flash && msg "K4R: full refresh done" || msg "K4R: flash FAILED"
    ;;
  fast)
    # fast 模式只设置策略位（对 KOReader 生效需 Lua 侧调用，见文档 §6）
    msg "K4R: mode=fast (lib default)"
    ;;
  conservative)
    msg "K4R: mode=conservative"
    ;;
  info)
    "$BIN" info | head -n 6 > /tmp/k4r_info.txt
    i=6; while IFS= read -r line; do i=$((i+1)); eips 0 $i "$line" >/dev/null 2>&1; done < /tmp/k4r_info.txt
    ;;
  bench)
    "$BIN" bench --fx partial,fast,slow --n 20 --out /mnt/us/k4refresh/bench.csv \
      && msg "K4R: bench done -> bench.csv" || msg "K4R: bench FAILED"
    ;;
  *)
    msg "K4R: usage {flash|info|bench}"
    ;;
esac
exit 0
