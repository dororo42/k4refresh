# k4refresh — Kindle 4 E-Ink 刷新策略控制器

Rust 编写的 Kindle 4（legacy einkfb）刷屏优化层：**翻页保持 KOReader 原生 partial，KOReader 插件按 ghost 档位每 N 页自动 `fx_update_slow` 收尾清残影**，外加确定性手动全刷、区域裁剪与 ioctl 计时 benchmark。

- ⚠️ **状态：v0.1.4，插件（重影优先模式）已实现，真机感知验收进行中**——部署前请阅读 [k4refresh技术方案.md](k4refresh技术方案.md) §7 测试表与 §9 风险清单，`info` 冒烟失败立即停用。
- 📦 下载：[Releases](https://github.com/dororo42/k4refresh/releases) 页 zip 包（含 CLI / .so / Lua 桥 / KOReader 插件 / KUAL 扩展 / SHA256SUMS）；或 Actions → build → artifacts。
- 🤖 CI：push 自动构建 ARM 产物；打 `v*` tag 自动发布 Release。

---

## 1. zip 包内容（Release）

```text
k4refresh-cli            ← 静态 CLI（musl，首选，零依赖）
k4refresh-cli-dynamic    ← 动态 CLI（gnueabihf，备选）
libk4refresh.so          ← 共享库（KOReader LuaJIT FFI 加载，gnueabihf）
k4refresh.koplugin/      ← KOReader 插件：重影优先模式（每 N 页自动 slow 收尾）
lua/k4refresh.lua        ← KOReader 桥接脚本（控制台手动用）
kual/config.xml          ← KUAL 菜单注册
kual/bin/k4r.sh          ← KUAL 菜单脚本
kual/bin/k4refresh-cli   ← KUAL 用的 CLI 副本
INSTALL.txt              ← 部署命令速查（与本节 §3 相同）
SHA256SUMS.txt           ← 完整性校验
```

校验：`sha256sum -c SHA256SUMS.txt`。

## 2. 构建（自建时）

```bash
./build.sh              # musl 静态 CLI（首选）
cargo build --release --target armv7-unknown-linux-gnueabihf   # .so 动态库（需 bootlin 交叉 gcc，见技术方案 §5.1）
cargo test              # 策略单测（host）
```

## 3. 部署（照抄即可；前置：越狱 + KUAL；SSH 通道二选一）

SSH 通道（二选一，推荐 A）：
- **A. KOReader 自带 SSH 服务器**（免装 usbnet，攻击面最小）：KOReader → 齿轮 → 网络 → 无线 → 开启 SSH 服务器，默认端口 `2222`，用户 `root`，密码在 KOReader 设置里可见/修改
- B. USBNetwork：usbnet 包安装后 `;debugOn` → `~usbNetwork` → `;debugOff` 启用

```bash
# 解压 zip 后，在 zip 目录内执行（IP/端口按你的 SSH 通道调整；scp 加 -P 2222）
ssh -P 2222 root@192.168.2.x "mkdir -p /mnt/us/k4refresh /mnt/us/extensions/k4refresh/bin /mnt/us/koreader/plugins"
scp -P 2222 k4refresh-cli libk4refresh.so root@192.168.2.x:/mnt/us/k4refresh/
scp -P 2222 k4refresh-cli                 root@192.168.2.x:/mnt/us/extensions/k4refresh/bin/
scp -P 2222 kual/config.xml               root@192.168.2.x:/mnt/us/extensions/k4refresh/
scp -P 2222 kual/bin/k4r.sh               root@192.168.2.x:/mnt/us/extensions/k4refresh/bin/
scp -P 2222 lua/k4refresh.lua             root@192.168.2.x:/mnt/us/koreader/   # 文件名小写
scp -P 2222 -r k4refresh.koplugin         root@192.168.2.x:/mnt/us/koreader/plugins/
ssh -P 2222 root@192.168.2.x "chmod +x /mnt/us/k4refresh/* /mnt/us/extensions/k4refresh/bin/* && sync"
```

> 部署/更新 KUAL 扩展后需**重启 Kindle**（KUAL 只在启动时扫描 extensions/）。

## 4. 使用

### 4.1 KOReader 插件（重影优先模式，主入口）

重启 KOReader 后主菜单出现 **K4Refresh (ghost clearing)**：

| 菜单项 | 动作 |
|---|---|
| Full Refresh (slow) | 立即整屏 slow 全刷，清残影 |
| Ghost clearing: Off / Every 4 / 6 / 8 pages | 设定每 N 页自动 slow 收尾（即时生效，并写入 mode.conf 与 KUAL/CLI 共享） |

行为：翻页保持原生 partial 不变；每 N 次翻页自动触发一次 slow 全刷（闪烁一次、残影清零）；章节/跳页等大步长跳转（|Δ页|>1）立即收尾。FFI/.so 不可用时插件自动回退静态 CLI，收尾动作不中断。

### 4.2 KUAL 菜单

KUAL → K4Refresh（写 mode.conf，**下次开书后**生效；KOReader 内菜单则即时生效）：

| 菜单项 | 动作 |
|---|---|
| Full Refresh (slow) | 立即整屏 slow 全刷，清残影 |
| Ghost: Off | mode.conf 写 `off`：关闭自动收尾 |
| Ghost: Every 4 / 6 / 8 Pages | mode.conf 写 `ghost N`：每 N 页收尾 |
| Screen Info | 屏显分辨率/位深（600x800, bpp=8 为正常） |
| Benchmark (CSV) | 跑 bench，结果落 `/mnt/us/k4refresh/bench.csv` |

### 4.3 命令行（SSH）

```bash
/mnt/us/k4refresh/k4refresh-cli --version                  # 打印版本
/mnt/us/k4refresh/k4refresh-cli info                       # 冒烟第 1 条：600x800, bpp=8
/mnt/us/k4refresh/k4refresh-cli flash                      # 强制 slow 全刷
/mnt/us/k4refresh/k4refresh-cli refresh --mode fast --interval 6
/mnt/us/k4refresh/k4refresh-cli bench --fx partial,fast,slow --n 20 --label T --out /mnt/us/bench.csv
/mnt/us/k4refresh/k4refresh-cli bench --seq fast,fast,fast,slow --n 20 --label C --out /mnt/us/bench_seq.csv
#                                                          ↑ 组合序列计时：模拟"3 快 1 收尾"真实翻页周期
/mnt/us/k4refresh/k4refresh-cli bench --fx slow --n 3 --label A --delay-ms 3000 --out /dev/null
#                                                       ↑ 组内每次刷新间隔 3 秒，肉眼逐次辨认标记
#
# bench 说明（v0.1.2+）：
#   - 每次刷新的图案左上角带白底计数标记（A1/A2/A3…），方便真机肉眼计数
#   - --delay-ms N（v0.1.3+）：相邻刷新间暂停 N 毫秒；不加则连发，人眼跟不上
#   - 结束时自动恢复进入前的画面（不留棋盘格）
#   - 完成消息为「N 行写入 xxx（M 次 ioctl 失败）」，M>0 才需要关注 CSV error 列
```

### 4.4 KOReader 内（Lua 桥，手动模式）

KOReader 菜单 → 更多工具 → Lua 调试台：

```lua
local K4R = require("k4refresh")   -- 文件名小写
K4R.init()                          -- 幂等；自动读取 KUAL 写入的 mode.conf
K4R.flash()                         -- 立即全刷
K4R.set_mode(0, 6)                  -- fast 模式，6 页一收尾
K4R.set_mode(1)                     -- 回 conservative
```

可把手势/按键绑定到上述调用（详见技术方案 §6.4）。

## 5. 卸载（100% 恢复原生）

```bash
ssh root@192.168.15.244
rm -rf /mnt/us/extensions/k4refresh   # KUAL 菜单
rm -f  /mnt/us/koreader/k4refresh.lua # Lua 桥
rm -rf /mnt/us/k4refresh              # 库、CLI、mode.conf、bench.csv
sync
```

无系统分区写入、无驻留组件，删除即净。

## 6. 排障

| 现象 | 处理 |
|---|---|
| `info` 打不开 /dev/fb0 | 未越狱或未开 USBNet；需 root shell |
| `info` 分辨率非 600x800 | 立即停用并卸载，反馈 `/proc/version` |
| KOReader 内 `require("k4refresh")` 报错 | 确认文件已拷到 `/mnt/us/koreader/` 且大小写为小写 |
| KUAL 菜单点了没反应 | 确认 `extensions/k4refresh/bin/` 下有 `k4r.sh` 和 `k4refresh-cli` 且有执行权限 |
| 残影明显 | interval 调大（如 8）或切 conservative |

更多：技术方案 §7 完整测试表、§8 备份与回滚、§9 风险清单。

## 7. 已知边界

- 仅适用 Kindle 4（Non-Touch，FW 4.1.4，legacy einkfb）；mxcfb 机型不适用。
- v0.1.4 插件接管的是"收尾调度"：翻页本身仍是 KOReader 原生 partial；插件每 N 页触发一次 slow 收尾。直接改写 KOReader 刷新后端（让翻页走 fast 档）未实现——真机实测三种 fx 在真实翻页下视觉不可分辨，该路径的收益存疑，暂缓。
- 插件 flash 走 libk4refresh.so（FFI）；.so 加载失败自动回退静态 CLI，两者都不在时收尾静默失败（不阻塞阅读）。
- KUAL 改档在下次开书后生效；KOReader 菜单改档即时生效。
- fast/conservative 库内模式只影响经本库（CLI refresh / Lua 桥 `K4R.refresh`）发起的刷新；无翻页路径消费者（v0.1.x 历史接口，保留仅为 ABI 兼容）。
- Windows 主机不可编译本 crate（`libc::ioctl` 仅 POSIX）；测试/构建用 Linux 或 CI。
