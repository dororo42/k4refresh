#!/bin/sh
# KUAL 菜单扩展：k4refresh（v0.1.4 重影优先模式）
# 安装位置（Kindle USB 根目录）:
#   /mnt/us/extensions/k4refresh/config.xml           ← KUAL 菜单注册
#   /mnt/us/extensions/k4refresh/bin/k4r.sh           ← 本脚本
#   /mnt/us/extensions/k4refresh/bin/k4refresh-cli    ← 从构建产物复制（静态版首选）
#
# 脚本逻辑全部是只读/刷新调用 + 写用户分区模式文件，不改任何系统文件；
# 卸载 = 删除整个目录（/mnt/us/extensions/k4refresh）。
#
# ghost 档位经 mode.conf 传给 KOReader 插件（每 N 页 slow 收尾清残影），
# 插件在下次开书时读取生效；KOReader 菜单内切换则即时生效。

BIN=/mnt/us/extensions/k4refresh/bin/k4refresh-cli
MODE_CONF_DIR=/mnt/us/k4refresh
MODE_CONF=$MODE_CONF_DIR/mode.conf

msg() {
  # KUAL 环境下用 eips 打一行状态（2 行起，4 秒），脚本退出码不受影响
  # 注：K4 真机实测 eips 屏底提示不可见，保留用于其他兼容机型
  eips 0 4 "$1" >/dev/null 2>&1
}

# KUAL 与 KOReader 是两个进程：模式经文件传递，
# KOReader 插件在下次开书（onReaderReady）时读取生效。
set_mode() {
  mkdir -p "$MODE_CONF_DIR" 2>/dev/null
  printf '%s\n' "$1" > "$MODE_CONF" && sync
}

case "$1" in
  flash)
    "$BIN" flash && msg "K4R: full refresh done" || msg "K4R: flash FAILED"
    ;;
  ghostoff)
    set_mode "off" && msg "K4R: ghost off" || msg "K4R: mode write FAILED"
    ;;
  ghost4)
    set_mode "ghost 4" && msg "K4R: ghost every 4 pages" || msg "K4R: mode write FAILED"
    ;;
  ghost6)
    set_mode "ghost 6" && msg "K4R: ghost every 6 pages" || msg "K4R: mode write FAILED"
    ;;
  ghost8)
    set_mode "ghost 8" && msg "K4R: ghost every 8 pages" || msg "K4R: mode write FAILED"
    ;;
  info)
    "$BIN" info | head -n 7 > /tmp/k4r_info.txt
    i=6; while IFS= read -r line; do i=$((i+1)); eips 0 $i "$line" >/dev/null 2>&1; done < /tmp/k4r_info.txt
    ;;
  bench)
    "$BIN" bench --fx partial,fast,slow --n 20 --out /mnt/us/k4refresh/bench.csv \
      && msg "K4R: bench done -> bench.csv" || msg "K4R: bench FAILED"
    ;;
  *)
    msg "K4R: usage {flash|ghost4|ghost6|ghost8|ghostoff|info|bench}"
    ;;
esac
exit 0
