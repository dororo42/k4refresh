# k4refresh — Kindle 4 E-Ink 刷新策略控制器

Rust 编写的 Kindle 4（legacy einkfb）刷屏优化层：**翻页保持 KOReader 原生 partial，KOReader 插件按 ghost 档位每 N 页自动 `fx_update_slow` 收尾清残影**，外加确定性手动全刷、区域裁剪与 ioctl 计时 benchmark。

- 🆕 **v0.1.10（2026-11-07）**：**Duokan 对齐量化**——多看固件逆向（报告《Duokan-xKindle-K4固件逆向与借鉴优化报告-2026-11-07.md》）证实其在渲染层对每像素执行 `v&0xF0|v>>4`（16 级灰阶，幂等），把 partial 刷新的累积灰阶污染锁死在 16 级内。新增：CLI `fxquant` 子命令（量化可见 fb + 可选刷新）、KOReader 插件 "Duokan quant" 开关（接管 einkfb `refreshPartialImp/refreshFullImp`，blit 后 ioctl 前纯 LuaJIT FFI 就地量化，无需 .so）、KUAL 菜单 3 项（Quantize now / Duokan Quant On/Off）、mode.conf 第 2 行 `quant on|off`、FFI 对等导出 `k4refresh_quant_refresh`。
- 🆕 **v0.1.9（2026-11-07）**：KUAL 菜单改为嵌套子菜单（主菜单只显示 "K4Refresh ▸"，7 项收进二级，与 KOReader 扩展同构）；修复旧式 config.xml 菜单在 KUAL 2.7 下不显示的问题（parse.awk 只认 `<menu type="json">`）。CLI 二进制与 v0.1.8 相同，无功能变化。
- ⚠️ **状态：v0.1.10，Duokan 量化待真机 A/B 验收**（bench `--pattern gray --quant` 对比照片；v0.1.8/0.1.9 的 ghost 收尾链路已在真机验证，残影感知验收结论：ghost 6 下可接受）。部署前请阅读技术方案文档（本地工作文件，不入库）的测试表与风险清单，`info` 冒烟失败立即停用。
- 📦 下载：[Releases](https://github.com/dororo42/k4refresh/releases) 页 zip 包（含静态 CLI / Lua 桥 / KOReader 插件 / KUAL 扩展 / SHA256SUMS；v0.1.8 起不再分发 .so 与动态 CLI）；或 Actions → build → artifacts。
- 🤖 CI：push 自动构建 ARM 产物；打 `v*` tag 自动发布 Release。

---

## 1. zip 包内容（Release）

```text
k4refresh-cli            ← 静态 CLI（musl，首选，零依赖）
k4refresh.koplugin/      ← KOReader 插件：重影优先模式（每 N 页自动 slow 收尾）
lua/k4refresh.lua        ← KOReader 桥接脚本（控制台手动用）
kual/config.xml          ← KUAL 菜单注册（type="json" 指向 menu.json）
kual/menu.json           ← KUAL 菜单项（KUAL 2.7 json 格式）
kual/bin/k4r.sh          ← KUAL 菜单脚本
kual/bin/k4refresh-cli   ← KUAL 用的 CLI 副本
INSTALL.txt              ← 部署命令速查（与本节 §3 相同）
SHA256SUMS.txt           ← 完整性校验
```

> v0.1.8 方案 A 起不再分发 `libk4refresh.so` 与动态 CLI：K4 用户态为 armel
> （glibc 2.12.1），gnueabihf 动态件无法加载/运行（详见 §7 已知边界）。
> FFI 待 koxtoolchain armel 重建 .so 后恢复（届时 touch
> `/mnt/us/k4refresh/ffi.enabled` 即启用探测）。

校验：`sha256sum -c SHA256SUMS.txt`。

## 2. 构建（自建时）

```bash
./build.sh              # musl 静态 CLI（唯一分发产物）
cargo test              # 策略单测（host）
```

## 3. 部署（照抄即可；前置：越狱 + KUAL；SSH 通道二选一）

SSH 通道（二选一，推荐 A）：
- **A. KOReader 自带 SSH 服务器**（免装 usbnet，攻击面最小）：KOReader → 齿轮 → 网络 → 无线 → 开启 SSH 服务器，默认端口 `2222`，用户 `root`，密码在 KOReader 设置里可见/修改
- B. USBNetwork：usbnet 包安装后 `;debugOn` → `~usbNetwork` → `;debugOff` 启用

```bash
# 解压 zip 后，在 zip 目录内执行（IP/端口按你的 SSH 通道调整；scp 加 -P 2222）
ssh -P 2222 root@192.168.2.x "mkdir -p /mnt/us/k4refresh /mnt/us/extensions/k4refresh/bin /mnt/us/koreader/plugins"
scp -P 2222 k4refresh-cli                 root@192.168.2.x:/mnt/us/k4refresh/
scp -P 2222 k4refresh-cli                 root@192.168.2.x:/mnt/us/extensions/k4refresh/bin/
scp -P 2222 kual/config.xml               root@192.168.2.x:/mnt/us/extensions/k4refresh/
scp -P 2222 kual/menu.json                root@192.168.2.x:/mnt/us/extensions/k4refresh/
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
| Quantize now (16-level, no flash) | 手动一次性：可见 fb 量化 + partial 呈现（CLI 执行，与开关无关） |
| Duokan quant: On / Off | v0.1.10：自动量化开关。On 时接管 einkfb `refreshPartialImp/refreshFullImp`，每次刷新前对可见 fb 就地 16 级量化（纯 LuaJIT FFI 直写 KOReader 已 mmap 的 fb，无需 .so）；Off 即还原原函数 |
| Ghost clearing: Off / Every 4 / 6 / 8 pages | 设定每 N 页自动 slow 收尾（即时生效，并写入 mode.conf 与 KUAL/CLI 共享） |

行为：翻页保持原生 partial 不变；每 N 次翻页自动触发一次 slow 全刷（闪烁一次、残影清零）；章节/跳页等大步长跳转（|Δ页|>1）立即收尾。v0.1.8 方案 A 起 FFI 默认停用（K4 armel 无可加载 .so），插件直接走静态 CLI，收尾动作不中断。量化钩子与该 FFI 停用无关（量化用 KOReader 自身 LuaJIT + 已映射内存，不开 .so）。

**收尾策略接管（v0.1.7+，v0.1.8 修正还原行为）**：ghost 模式激活时，插件自动把 KOReader 内置的"每 6 次 partial 升级全刷"（UIManager `full_refresh_count`）设为 0，避免与插件收尾叠加成双重黑闪；Ghost: Off 时还原**接管前的档位**（v0.1.7 曾硬编码 6，会覆盖用户自定义档；v0.1.8 起捕获原值，且 0 不落盘，重启后也不会把 0 当用户设置）。背景：实测连续 partial 全屏反转的残影单调累积（第 1→4 次反转 RMS 约 10%→15%），周期性 slow 收尾是必要设计。

### 4.2 KUAL 菜单

KUAL → K4Refresh（写 mode.conf，**下次开书后**生效；KOReader 内菜单则即时生效）：

| 菜单项 | 动作 |
|---|---|
| Full Refresh (slow) | 立即整屏 slow 全刷，清残影 |
| Quantize now (16-level) | CLI `fxquant --refresh partial`：量化当前屏并呈现 |
| Duokan Quant: On / Off | mode.conf 第 2 行写 `quant on/off`，下次开书生效 |
| Ghost: Off | mode.conf 写 `off`：关闭自动收尾 |
| Ghost: Every 4 / 6 / 8 Pages | mode.conf 写 `ghost N`：每 N 页收尾 |
| Screen Info | 屏显分辨率/位深（600x800, bpp=8 为正常） |
| Benchmark (CSV) | 跑 bench，结果落 `/mnt/us/k4refresh/bench.csv` |

### 4.3 命令行（SSH）

```bash
/mnt/us/k4refresh/k4refresh-cli --version                  # 打印版本
/mnt/us/k4refresh/k4refresh-cli info                       # 冒烟第 1 条：600x800, bpp=8
/mnt/us/k4refresh/k4refresh-cli flash                      # 强制 slow 全刷
/mnt/us/k4refresh/k4refresh-cli fxquant --refresh partial  # v0.1.10+：可见 fb 16 级量化（幂等）+ partial 呈现
/mnt/us/k4refresh/k4refresh-cli fxquant --refresh none     # 只量化不刷新（等下次刷新呈现）
/mnt/us/k4refresh/k4refresh-cli restore --mode 1           # v0.1.7+：RESTORE_DISPLAY 整帧重推（花屏修复/确定性清残影）
/mnt/us/k4refresh/k4refresh-cli fxupdate --mode 1 --which -1            # v0.1.7+：整屏刷新（fx_t 通道）
/mnt/us/k4refresh/k4refresh-cli fxupdate --mode 0 --which 21 --exclude 0,750,600,800
#                                                          ↑ partial + 反色变换 + 排除底部 50 行
/mnt/us/k4refresh/k4refresh-cli refresh --mode fast --interval 6
/mnt/us/k4refresh/k4refresh-cli bench --fx partial,fast,slow --n 20 --label T --out /mnt/us/bench.csv
/mnt/us/k4refresh/k4refresh-cli bench --seq fast,fast,fast,slow --n 20 --label C --out /mnt/us/bench_seq.csv
#                                                          ↑ 组合序列计时：模拟"3 快 1 收尾"真实翻页周期
/mnt/us/k4refresh/k4refresh-cli bench --fx slow --n 3 --label A --delay-ms 3000 --out /dev/null
#                                                       ↑ 组内每次刷新间隔 3 秒，肉眼逐次辨认标记
/mnt/us/k4refresh/k4refresh-cli bench --fx partial --n 3 --delay-ms 7000 --pattern gray --shot-idx 1 --label N
#                                                       ↑ 灰阶条带残影观察；--quant 附加 16 级量化；--shot-idx 为 0-based（第 N+1 帧显示「标记+请拍摄」横幅）
#
# bench 说明（v0.1.2+）：
#   - 每次刷新的图案左上角带白底计数标记（A1/A2/A3…），方便真机肉眼计数
#   - --delay-ms N（v0.1.3+）：相邻刷新间暂停 N 毫秒；不加则连发，人眼跟不上
#   - --sync（v0.1.5+）：每次刷新后 fsync(fd)（= WAITFORVSYNC）计时；实测 ioctl 本身同步
#     （~125ms），fsync 仅 0.4-0.5ms，CSV 增列 sync_usec
#   - --pattern gray / --quant / --shot-idx（v0.1.5/0.1.6+）：残影对比实验工具，见上例
#   - 结束时自动恢复进入前的画面（不留测试图案）
#   - 完成消息为「N 行写入 xxx（M 次 ioctl 失败）」，M>0 才需要关注 CSV error 列
#
# restore/fxupdate 说明（v0.1.7+）：ioctl 编号自动按 新(0x46ef/0x46e4)→旧(0x4644/0x4642)
# 探测并在输出中报告生效者；K4 4.1.4 实测新编号直接生效。
```

### 4.4 KOReader 内（Lua 桥，手动模式）

KOReader 菜单 → 更多工具 → Lua 调试台：

```lua
local K4R = require("k4refresh")   -- 文件名小写
K4R.init()                          -- 幂等；自动读取 KUAL 写入的 mode.conf
K4R.flash()                         -- 立即全刷
K4R.quant("partial")                -- v0.1.10+：量化+刷新（"none"/"full"/"slow" 可选）
K4R.set_mode(0, 6)                  -- fast 模式，6 页一收尾
K4R.set_mode(1)                     -- 回 conservative
```

可把手势/按键绑定到上述调用（详见技术方案 §6.4）。

## 5. 卸载（100% 恢复原生）

```bash
ssh root@192.168.15.244
rm -rf /mnt/us/extensions/k4refresh              # KUAL 菜单
rm -rf /mnt/us/koreader/plugins/k4refresh.koplugin  # KOReader 插件
rm -f  /mnt/us/koreader/k4refresh.lua            # Lua 桥
rm -rf /mnt/us/k4refresh                         # 库、CLI、mode.conf、实验数据
sync
```

无系统分区写入、无驻留组件，删除即净。v0.1.8 起插件接管不污染 `full_refresh_count` 设置：卸载后重启 KOReader 即自动恢复内置全刷档；如需立即恢复，Lua 调试台执行 `require("ui/uimanager"):setRefreshRate(6)`。

## 6. 排障

| 现象 | 处理 |
|---|---|
| `info` 打不开 /dev/fb0 | 未越狱或未开 USBNet；需 root shell |
| `info` 分辨率非 600x800 | 立即停用并卸载，反馈 `/proc/version` |
| KOReader 内 `require("k4refresh")` 报错 | 确认文件已拷到 `/mnt/us/koreader/` 且大小写为小写 |
| KUAL 菜单点了没反应 | 确认 `extensions/k4refresh/bin/` 下有 `k4r.sh` 和 `k4refresh-cli` 且有执行权限 |
| 残影明显 | ghost 间隔调小（Every 4）或菜单 Full Refresh 手动收尾；图片多的书可加大间隔 |

更多：技术方案 §7 完整测试表、§8 备份与回滚、§9 风险清单。

## 7. 已知边界

- 仅适用 Kindle 4（Non-Touch，FW 4.1.4，legacy einkfb）；mxcfb 机型不适用。
- v0.1.7 插件接管的是"收尾调度"：翻页本身仍是 KOReader 原生 partial；插件每 N 页触发一次 slow 收尾，并在 ghost 激活时关闭 KOReader 内置的每 6 页 promotion 全刷（防双重黑闪，Off 时恢复）。直接改写 KOReader 刷新后端（让翻页走 fast 档）未实现——真机实测三种 fx 在真实翻页下视觉不可分辨，且 E1 实测各通道 ioctl 同步 ~125ms，该路径无收益，已关闭。
- **FFI 默认停用（方案 A，2026-11-01 起实施；2026-09-19 实机定性）**：K4 用户态为 armel/softfp + glibc 2.12.1，gnueabihf 产物（armhf + GLIBC_2.18~2.34 标签）无法 dlopen（真机 luajit 复现：`ld-linux-armhf.so.3` 缺失）。v0.1.8 起 zip 不再分发 .so 与动态 CLI，插件/Lua 桥默认直接走静态 CLI（真机 flash 冒烟可用）。FFI 路径待 koxtoolchain armel 重建 .so（方案 B）恢复；届时 `touch /mnt/us/k4refresh/ffi.enabled` 即恢复探测，无需改代码。
- KUAL 改档在下次开书后生效；KOReader 菜单改档即时生效。
- fast/conservative 库内模式只影响经本库（CLI refresh / Lua 桥 `K4R.refresh`）发起的刷新；无翻页路径消费者（v0.1.x 历史接口，保留仅为 ABI 兼容）。
- Windows 主机不可编译本 crate（`libc::ioctl` 仅 POSIX）；测试/构建用 Linux 或 CI。
