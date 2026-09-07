# k4refresh — Kindle 4 E-Ink 刷新策略控制器

Rust 编写的 Kindle 4（legacy einkfb）刷屏优化层：**翻页走 `fx_update_fast` 快速档 + 每 N 页 `fx_update_slow` 收尾清残影**，外加确定性手动全刷、区域裁剪与 ioctl 计时 benchmark。

- 📄 完整技术方案（环境约束/策略参数/部署/回滚/测试表）：[K4Refresh技术方案.md](K4Refresh技术方案.md)
- 🔧 构建：`./build.sh`（musl 静态 CLI）/ `cargo build --release --target armv7-unknown-linux-gnueabihf`（.so 动态）
- 🤖 CI：push 自动构建并上传 ARM 产物（Actions → build → artifacts）
- 📦 部署：产物落 `/mnt/us/k4refresh/`，KUAL 菜单入口 `extensions/k4refresh/`，Lua 桥 `lua/K4Refresh.lua`
- ↩️ 回滚：删除上述三个路径即恢复原生刷新行为（无系统分区写入，无驻留组件）

```bash
# 设备端冒烟
./k4refresh-cli info     # 600x800, bpp=8
./k4refresh-cli flash    # 强制 slow 全刷
./k4refresh-cli bench --n 20 --out /mnt/us/bench.csv
```

状态：v0.1.0 —— 9/9 单测通过，x86 交叉编译验证通过（ARM EABI5 产物），设备端实测待用户执行（方案 §7 提供完整测试表）。
