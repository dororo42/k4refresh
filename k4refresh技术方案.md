# k4refresh — Kindle 4 E-Ink 刷屏优化技术方案

> 版本 0.1.0 ｜ 2026-09-08 ｜ 配套代码仓库：本目录（`k4refresh-rust/`）
> 目标设备：Kindle 4 Non-Touch（i.MX508，FW 4.1.4），已越狱（KUAL + KOReader）
> 阅读前提：你只需要 USBNetwork（SSH）+ 传文件的能力，不需要编译机在本地——CI 会产出可下载的 ARM 二进制。

---

## 0. 五分钟速览

**这件事做什么**：Kindle 4 的墨水屏驱动是 legacy `einkfb`，用户态只有 4 种刷新类型（fx）可调，没有 waveform 概念。本方案交付一个 Rust 编写的刷新策略层：**翻页用 KOReader 从未启用的 `fx_update_fast` 快速档，每 N 页自动用 `fx_update_slow` 收尾清残影**，外加整屏手动全刷、区域合并与一个可测量的 benchmark 工具。全部功能不改系统文件、不碰内核，卸载即恢复原状。

**三个关键事实**（均有源码出处，详见 §2）：

| # | 事实 | 出处 |
|---|------|------|
| 1 | K4 走 legacy einkfb 驱动，接口只有 `fx_update_partial/full/fast/slow` 四型 + 两个整屏 ioctl | FBInk `eink/einkfb.h`；KOReader `ffi/framebuffer_einkfb.lua` |
| 2 | `fx_update_fast`（牺牲保真换速度）在 KOReader 中从未被调用，是现成的速度杠杆 | KOReader 源码仅 partial/slow 两处调用 |
| 3 | 驱动会跳过"内容未变"的刷新；`fx_update_slow` 豁免该检查 | koreader#12774 → koreader-base#2481（2026-08 合入） |

**目录**：

1. 环境约束（每条标注已核实/待验证）
2. 刷新机制与优化原理
3. 三类优化策略（参数/位置/预期效果/前后对比）
4. Rust 工程结构与代码导读
5. 构建指南（本地交叉编译 + GitHub Actions CI）
6. 部署接入（KUAL 菜单 + KOReader Lua 桥）
7. 设备端测试清单与记录表
8. 备份与一键回滚
9. 风险与兼容性清单
10. 术语表

---

## 1. 环境约束

### 1.1 硬件与系统

| 项 | 值 | 状态 | 出处/验证方法 |
|----|----|------|--------------|
| SoC | Freescale i.MX508（ARM Cortex-A8, ARMv7-A） | ✅ 已核实 | KOReader `device.lua` Kindle4 定义；实机 `cat /proc/cpuinfo` 复核 |
| 面板 | 6" E-Ink Pearl，600×800，8bpp 灰阶（反色 palette） | ✅ 已核实 | KOReader Kindle4 注释 "running @ 8bpp, expecting an inverted palette"；实机 `k4refresh-cli info` 复核 |
| 固件 | FW 4.1.4（K4 最终版） | ✅ 已核实 | 设置→设备信息；也适用于 4.x 其他小版本 |
| 用户态 ABI | hardfp（armhf），存在 `/lib/ld-linux-armhf.so.3` | ✅ 已核实 | KOReader `isHardFP()` 分流逻辑；实机 `ls /lib/ld-linux-armhf.so.3` 复核 |
| 内核 | 2.6.x 世代（lab126 4.1.4 内核） | ⚠️ 待验证 | 实机 `cat /proc/version` 记录精确版本（影响见 §9 R1） |
| eink 驱动接口 | legacy einkfb：`FBIO_EINK_UPDATE_DISPLAY`(0x46db) / `FBIO_EINK_UPDATE_DISPLAY_AREA`(0x46dd) | ✅ 已核实 | FBInk `refresh_legacy()`（注释原文 "[K2<->K4]"） |
| 刷新类型 | fx_update_partial=0 / full=1 / fast=2 / slow=3（+特效 flash/invert） | ✅ 已核实 | FBInk `einkfb.h` `enum fx_type`；KOReader `ffi/einkfb_h.lua` cdef 同值 |
| 等待机制 | 无（mxcfb 时代的 marker/wait ioctl 不存在） | ✅ 已核实 | FBInk legacy 分支无任何 wait 调用 |
| 已装软件 | KUAL + KOReader 2026.07.1（菜单版） | ✅ 用户确认 | KOReader 底部菜单→关于 |

### 1.2 关键行为约束

- **no-op 跳过**：驱动对"请求区域像素与屏上现状一致"的刷新静默跳过；`fx_update_slow` 豁免。✅ 已核实（koreader#12774 实测记录 + base#2481 修复注释）。
  - ⚠️ 你的 KOReader 2026.07.1 **不含**该修复（2026-08 才合入），表现为"手动全刷有时无效"。本方案的自带全刷（`k4refresh-cli flash`）不受影响，因为走 slow。
- **`fx_update_full` 实际不闪烁**：经 AREA ioctl 调用时 full 与 partial 行为不同但都不闪；可靠的闪烁全刷是 slow。✅ 已核实（FBInk `refresh_legacy` 注释，并引用 base#2481）。
- **整屏刷新应走 `FBIO_EINK_UPDATE_DISPLAY`**：FBInk 实测比 AREA 版本可靠。✅ 已核实，`k4refresh` 已内置该逻辑。
- **8bpp + 反色 palette**：直接操作 framebuffer 内存时黑白色序相反；本方案默认不写帧缓冲（bench 除外），不受影响。✅ 已核实（FBInk legacy 反色 LUT 注释）。

### 1.3 刷新管线（修正版）

```text
KOReader (LuaJIT)
  → UIManager（脏区域/调度）
  → framebuffer_einkfb.lua      ← K4 实际后端（不是 mxcfb）
  → FBIO_EINK_UPDATE_DISPLAY_AREA / _DISPLAY
  → lab126 einkfb HAL（FX 决策）
  → i.MX508 EPDC + E-Ink Pearl  ← 波形由固件内部决定，用户态不可见
```

用户态可控制的全部自由度：**fx 类型 × 区域矩形 × 刷新时机 × 是否发请求（利用 no-op 跳过）**。本方案的优化全部落在这四个自由度内。

---

## 2. 刷新机制与优化原理

### 2.1 四种 fx 的语义与用途

| fx | 值 | 速度 | 保真 | 闪烁 | KOReader 现状 | 本方案用途 |
|----|----|------|------|------|--------------|-----------|
| partial | 0 | 快 | 中 | 否 | ✅ 默认 partial | UI/菜单/高亮 |
| full | 1 | 慢 | 高 | **否**（实测） | ❌ 未用 | 不使用（不可靠） |
| fast | 2 | **最快** | 低（黑白量化） | 否 | ❌ **未用** | **翻页主档** |
| slow | 3 | 最慢 | 最高 | **是** | ✅ 默认 full | 定期清残影 + 手动全刷 |

原生固件自己的"快速翻页"就是 **fast 序列 + slow 收尾**（`einkfb.h` 的 `UPDATE_FAST_PAGE_TURN` 宏定义 fast 与 slow 同组）。本方案把这个行为搬到 KOReader。

### 2.2 为什么用 Rust

- 收益不在速度（刷新是百毫秒级物理动作），而在：ioctl 薄层的内存安全、策略逻辑可用单测覆盖（9 个用例随仓库交付）、单个静态二进制零依赖分发。
- 依赖只有 `libc` 一个 crate（`default-features=false`），供应链最小。
- 不用 Rust 的替代路径：同一架构用 C + koxtoolchain 重写代价小，方案不锁定语言。

---

## 3. 三类优化策略（本方案核心）

### 优化 A：翻页刷新路径（最大收益点）

| 项 | 内容 |
|----|------|
| 参数 | 模式 `fast`；`page_interval` ∈ {4, 6, 8}，默认 **6** |
| 修改位置 | `k4refresh_set_mode(0, interval)`（Lua 桥调用）；CLI 侧 `--mode fast --interval N` |
| 行为 | 前 N-1 次翻页 → `fx_update_fast`（最快、不闪、轻微残影）；第 N 次 → `fx_update_slow`（闪烁一次、残影清零） |
| 预期效果 | 翻页 ioctl 即时性提升（fast 是驱动定义的最快档）；代价是每 N 页有一次全屏闪烁 |
| 优化前 | 每次翻页 = `fx_update_partial`（中等速度中等保真），残影靠手动全刷 |
| 优化后 | 翻页节奏接近原生固件"快速翻页"设计；残影有确定性的自动清偿周期 |

**假设声明**（标注为待实测）：fast 档在 8bpp K4 上的残影幅度未知，interval 的最优值必须用 §7 的测试表实测定；若 fast 残影不可接受，退回 `conservative` 模式（行为等同原生 partial，只有全刷调度差异），零风险。

### 优化 B：全刷频率控制（Ghost Budget）

| 项 | 内容 |
|----|------|
| 参数 | 显式全刷 → `fx_update_slow`（闪烁、no-op 豁免）；策略计数器在显式全刷后归零 |
| 修改位置 | `k4refresh_flash()`（Lua/KUAL）；策略逻辑 `src/fx.rs` `Policy::decide()` |
| 行为 | 任何"全刷"请求都转为可靠的 slow；翻页计数的 slow 收尾同时复位计数，二者共享一个预算 |
| 预期效果 | 全刷**确定性生效**（绕开 no-op 跳过）；闪烁次数 = 策略决定，不再随机 |
| 优化前 | KOReader 2026.07.1 的全刷请求可能被驱动静默丢弃（#12774），用户感知"按了没反应" |
| 优化后 | 每次全刷必闪必清；翻页收尾全刷按固定节奏出现，可预期 |

### 优化 C：刷新区域与时机

| 项 | 内容 |
|----|------|
| 参数 | 区域矩形 (x1,y1,x2,y2)，开边界；退化区域（宽/高≤1 或完全出屏）自动跳过返回 0 |
| 修改位置 | `k4refresh_refresh(fd, kind, x1, y1, x2, y2)`；裁剪逻辑 `src/fx.rs` `clamp_rect()` |
| 行为 | 越界矩形裁剪到屏幕；整屏全刷自动升级为 `FBIO_EINK_UPDATE_DISPLAY`（FBInk 实测更可靠） |
| 预期效果 | 避免无效 ioctl；为后续 KOReader 深度集成（脏区域合并）预留接口 |
| 优化前 | KOReader einkfb 后端每次传整页区域，无合并、无防御性裁剪 |
| 优化后 | 任意区域安全可达；1px 退化区域不再打到驱动 |

> **明确不做的**（与上一轮评估报告一致）：不改内核、不换波形固件、不承诺 Duokan 级体验——K4 的波形决策在固件内，用户态只有 fx 这一个旋钮。

---

## 4. Rust 工程结构与代码导读

```text
k4refresh-rust/
├── Cargo.toml              # 依赖仅 libc(default-features=false)；cdylib+rlib；opt-level="s"
├── .cargo/config.toml      # musl 静态 target 的 linker 配置（含回退说明）
├── build.sh                # 一键构建脚本（musl/gnueabihf/host 三模式）
├── src/
│   ├── fx.rs               # ★ 策略层（纯逻辑）：fx 常量、Policy 状态机、clamp_rect
│   ├── eink.rs             # ★ 驱动层：2 个刷新 ioctl + fbinfo；全 crate 仅有的 unsafe（逐条注释）
│   ├── lib.rs              # C ABI（k4refresh_* 7 个导出）+ 原子策略状态 + panic 兜底
│   ├── main.rs             # CLI：info / refresh / flash / bench 四个子命令
│   └── bench.rs            # bench 实现：mmap 帧缓冲 + 棋盘图案 + ioctl 计时 CSV
├── lua/k4refresh.lua       # KOReader LuaJIT FFI 桥（§6.2，文件名小写）
├── kual/                   # KUAL 扩展（config.xml + k4r.sh）
├── .github/workflows/      # CI：三平台矩阵构建 + musl 静态 + 产物上传（§5.2）
└── dist/                   # 本次已验证的 ARM 产物（v0.1.0，见 §5.3 校验值）
```

**代码导读（对应验收标准"干净且安全"）**：

- `fx.rs`：纯逻辑零 syscall，策略状态机含 6 个单测（间隔收尾、全刷复位、UI 恒 partial、conservative 等价原生、interval=1 逐页收尾、矩形裁剪边界）。
- `eink.rs`：unsafe 仅 `open/close`、`ioctl` 两类，每条注释说明驱动 ABI 依据与"驱动只读入参、无用户态写入"的安全性论证；`update_area_t` 与 C 布局逐字段一致（ARM32 24 字节）。
- `lib.rs`：FFI 边界 `catch_unwind` 兜底（内部异常返回 `K4R_PANIC=-99`，不穿透 KOReader）；所有 ioctl 失败只记录 errno 并返回负值，**调用方（阅读器）永不因刷新失败而中断**；策略状态用原子变量，无锁无堆。
- `main.rs`/`bench.rs`：bench 的 mmap 长度取驱动报告的 `smem_len`，显式 munmap 封闭生命周期；失败行写入 CSV error 列不中断批量测量。

---

## 5. 构建指南

### 5.1 本地构建（Linux x86_64 主机）

```bash
# 1. 工具链（任选一个渠道装 rustup 后）
rustup target add armv7-unknown-linux-gnueabihf

# 2. ARM hf 交叉工具链（K4 是 hardfp； musl 分支见下）
#    推荐.bootlin glibc 2024.05（已验证）:
curl -LO https://toolchains.bootlin.com/downloads/releases/toolchains/armv7-eabihf/tarballs/armv7-eabihf--glibc--stable-2024.05-1.tar.xz
tar xJf armv7-eabihf--glibc--stable-2024.05-1.tar.xz -C /opt
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=/opt/armv7-eabihf--glibc--stable-2024.05-1/bin/arm-buildroot-linux-gnueabihf-gcc

# 3. 测试 + 构建
cargo test                                    # 9 个策略单测（host 上跑）
cargo build --release --target armv7-unknown-linux-gnueabihf
# 产物: target/armv7-unknown-linux-gnueabihf/release/{k4refresh-cli, libk4refresh.so}
file target/armv7-unknown-linux-gnueabihf/release/libk4refresh.so
#   → ELF 32-bit LSB shared object, ARM, EABI5, dynamically linked
file target/armv7-unknown-linux-gnueabihf/release/k4refresh-cli
#   → ELF 32-bit LSB pie executable, ARM, interpreter /lib/ld-linux-armhf.so.3  ← 与 K4 匹配
```

**musl 静态分支（CLI 备选，经验教训）**：`armv7-unknown-linux-musleabihf` 在当前 stable 上对 `cdylib` 会报 `dropping unsupported crate type`（musl target 默认 `dynamic_linking=false`），**静态 CLI 可用但 .so 不产出**。因此：
- `libk4refresh.so`（KOReader FFI 用）→ 走 gnueabihf 动态（本方案主路径，已验证）；
- `k4refresh-cli` 静态版（KUAL 脚本用）→ musl 可选，`.cargo/config.toml` 已配 `rust-lld` 自包含链接，`build.sh musl` 一键产出（已验证 `statically linked`）。

### 5.2 GitHub Actions CI（仓库自带）

`.github/workflows/build.yml`：push/PR 触发，ubuntu runner 上固定 rustc 版本、装 gnueabihf/musl target + bootlin 工具链（带缓存），跑 `cargo test` → 两个 target 构建 → 上传 artifact（`k4refresh-arm-binaries`，含 .so、cli 与 kual/ 文件）。`.github/workflows/release.yml`：打 `v*` tag 触发，自动构建并发布 GitHub Release（zip + sha256）。无需自建编译机。

### 5.3 本轮已验证产物（v0.1.0，随仓库 dist/ 分发）

| 文件 | 大小 | 类型（file 输出） |
|------|------|------------------|
| `dist/libk4refresh.so` | 306,132 B | ELF 32-bit LSB shared object, ARM, EABI5, dynamically linked, stripped |
| `dist/k4refresh-cli` | 318,504 B | ELF 32-bit LSB pie executable, ARM, EABI5, interpreter /lib/ld-linux-armhf.so.3 |

验证环境：Ubuntu 24.04 x86_64 + rustc 1.98.1 + bootlin glibc 2024.05 工具链；`cargo test` 9/9 通过、0 warning。

---

## 6. 部署接入

### 6.1 前置：开启 USBNetwork（一次性）

1. 越狱后安装 USBNetwork hack（MR 包），`~usbnet/bin/enable`（或 KUAL → USBNet）。
2. USB 连接电脑，SSH `root@192.168.15.244`（USB 网口默认地址；密码或密钥按你装 hack 时的配置）。
3. 备份清单见 §8.1 —— **先备份，后部署**。

### 6.2 文件部署

```bash
# 电脑上（产物来自 dist/、CI artifact 或 Release zip）
# 注意：KUAL 脚本固定从 /mnt/us/extensions/k4refresh/bin/ 找 CLI，
#       因此 CLI 要放两份（k4refresh/ 与 extensions/k4refresh/bin/）。
ssh root@192.168.15.244 "mkdir -p /mnt/us/k4refresh /mnt/us/extensions/k4refresh/bin"
scp dist/libk4refresh.so dist/k4refresh-cli root@192.168.15.244:/mnt/us/k4refresh/
scp dist/k4refresh-cli   root@192.168.15.244:/mnt/us/extensions/k4refresh/bin/
scp kual/config.xml      root@192.168.15.244:/mnt/us/extensions/k4refresh/
scp kual/k4r.sh          root@192.168.15.244:/mnt/us/extensions/k4refresh/bin/
scp lua/k4refresh.lua    root@192.168.15.244:/mnt/us/koreader/   # 位置 A（推荐，文件名小写）
ssh root@192.168.15.244 "chmod +x /mnt/us/k4refresh/* /mnt/us/extensions/k4refresh/bin/* && sync"
```

部署后目录：

```text
/mnt/us/k4refresh/libk4refresh.so      ← 共享库
/mnt/us/k4refresh/k4refresh-cli        ← CLI（可独立用；模式文件 mode.conf 也落此目录）
/mnt/us/koreader/k4refresh.lua         ← Lua 桥
/mnt/us/extensions/k4refresh/          ← KUAL 菜单（config.xml + bin/k4r.sh + bin/k4refresh-cli）
```

### 6.3 冒烟验证（部署后立即做）

```bash
ssh root@192.168.15.244
/mnt/us/k4refresh/k4refresh-cli info      # 应打印 600x800 bpp=8
/mnt/us/k4refresh/k4refresh-cli flash     # 屏幕应闪烁一次并清残影
/mnt/us/k4refresh/k4refresh-cli refresh --mode fast --interval 6   # 应输出 ok: fx=2
```

三条都正常 → 基础链路通。KOReader 侧验证见 §7。

### 6.4 KOReader 内调用（手动模式，零侵入）

KOReader 菜单 → （工具）→ 更多工具 → **Lua 调试台（Lua console）**，逐条：

```lua
local K4R = require("K4Refresh"); K4R.init()   -- 首次
K4R.flash()                                     -- 立即全刷（验证闪烁）
K4R.set_mode(0, 6)                              -- fast 模式，6 页一收尾
```

要挂到手势/按键：KOReader → 点按手势/按键 → 按类别筛选 → 屏幕刷新类目（`flash` 对应手动全刷）。第二阶段（自动接管 refreshPartial）见 §9 R4 说明。

---

## 7. 设备端测试清单与记录表

### 7.1 基准测量（先跑原生的，再跑本方案）

```bash
# A. ioctl 提交耗时（每 fx 各 20 次，CSV 落盘）
/mnt/us/k4refresh/k4refresh-cli bench --fx partial,fast,slow --n 20 --out /mnt/us/bench_before.csv
# B. KOReader 默认阅读体验：同一本书同一章翻 20 页，按 §7.2 表格记录
```

### 7.2 主观/半客观测试表（每模式一份，优化前后各跑一遍）

| 测试项 | 方法 | 通过判据 | 结果（优化前） | 结果（优化后 fast/interval=6） |
|--------|------|----------|----------------|------------------------------|
| 翻页即视延迟 | 翻 20 页，数"按键到内容可读"的节拍（手机 240fps 慢动作辅助） | 平均节拍不劣于优化前 | ______ | ______ |
| 残影程度 | 翻 5 页后观察上一页文字印痕：0 无 / 1 轻 / 2 明显 / 3 严重 | 平均 ≤ 优化前 +1 档 | ______ | ______ |
| 慢动作残影帧 | 240fps 拍单次翻页，看旧内容消散帧数 | 记录帧数 | ______ | ______ |
| 异常闪烁 | 阅读 10 分钟，数非预期全屏闪烁次数 | = 策略预期值（fast 模式 ≈ 每 interval 页 1 次） | ______ | ______ |
| 手动全刷 | KUAL → K4Refresh → Full Refresh ×5 | 5 次全部闪烁（无"没反应"） | ______ | ______ |
| 灰阶图片页 | 开一本含插图的书翻 10 页 | 图片页无明显碎裂/发虚（fast 档可能在图片页发虚——如实记录） | ______ | ______ |
| 长稳阅读 | 连续 30 分钟 | 无卡死、无 KOReader 崩溃、残影未累积恶化 | ______ | ______ |
| 休眠唤醒 | 盖面板休眠 → 唤醒 ×5 | 唤醒画面正常 | ______ | ______ |

### 7.3 CSV 数据怎么读

`bench_before.csv` 每行 `fx,seq,ioctl_usec,error`。比较三件事：① 各 fx 的 usec 中位数（提交成本）；② slow 是否明显更慢（符合预期，它要驱动完整处理）；③ error 列非 0 的行数（应为 0，非 0 记录到 §9 风险触发判断）。**注意**：usec 只含提交排队，不含面板完成时间；面板完成时间看 §7.2 慢动作项。

### 7.4 调参流程

```text
interval=8 → 记录残影 → 不可接受 → 6 → 4 → 仍不可接受 → conservative 模式
（图片多的书用 conservative；纯文字书用 fast + 较大 interval）
```

---

## 8. 备份与一键回滚

### 8.1 部署前备份（必做）

```bash
ssh root@192.168.15.244
mkdir -p /mnt/us/k4refresh-backup
# 本方案不改任何系统文件；备份的只是"可能被 KUAL 菜单触碰的"KOReader 配置与目录清单
cp -a /mnt/us/koreader/settings  /mnt/us/k4refresh-backup/koreader_settings 2>/dev/null
cp -a /mnt/us/extensions         /mnt/us/k4refresh-backup/extensions_readme 2>/dev/null || true
ls -laR /mnt/us/k4refresh /mnt/us/koreader/k4refresh.lua > /mnt/us/k4refresh-backup/manifest_before.txt 2>/dev/null
sync
```

> 本方案的全部文件都落在 `/mnt/us/`（用户分区），**不写入 /var /etc /usr 等系统分区**。理论上系统分区零风险，备份是对 KUAL/KOReader 配置的额外保险。

### 8.2 一键卸载/回滚（完整恢复默认刷新行为）

```bash
ssh root@192.168.15.244
rm -rf /mnt/us/extensions/k4refresh      # 移除 KUAL 菜单入口
rm -f  /mnt/us/koreader/k4refresh.lua    # 移除 Lua 桥
rm -rf /mnt/us/k4refresh                 # 移除库与 CLI
cp -a /mnt/us/k4refresh-backup/koreader_settings /mnt/us/koreader/ 2>/dev/null  # 还原配置（可选）
sync
# 重启 KOReader：菜单 → 退出 KOReader → 再进
```

卸载后 KOReader 的刷新路径回到 100% 原生（`framebuffer_einkfb` 默认行为）。**本方案无任何驻留组件、无自启动项，删除即净。**

### 8.3 刷新异常时的应急恢复

```bash
# 屏幕花屏/残影异常时（SSH 还可用）：
/mnt/us/k4refresh/k4refresh-cli flash          # 强制 slow 全刷
# SSH 也不可用：长按电源 20s+ 强制重启（K4 无硬复位键，重启后 einkfb 状态自动重建）
```

---

## 9. 风险与兼容性清单

| # | 风险 | 等级 | 规避 | 触发时的判断/恢复 |
|---|------|------|------|-------------------|
| R1 | 老 2.6.x 内核对动态库/新编译器的兼容边角 | 中 | 产物按 K4 用户态（armhf/glibc 2.11 世代符号集）构建并用 §6.3 冒烟验证；CI 固定 rustc 版本 | 冒烟第 1 条 `info` 失败即停，卸载（§8.2），把 `/proc/version` 发 issue |
| R2 | fx 语义与枚举注释不符（full 实测不闪已是先例） | 中 | 本方案只用实测过行为的 fast/slow/partial；策略参数以 §7 实测为准，不信注释 | 阅读异常 → 切 conservative 模式（等价原生） |
| R3 | fast 档残影超预期（图片页尤其） | 低-中 | interval 调大或图片页用 conservative；CLI `--kind ui` 恒 partial | §7.2 残影评分 ≥3 → 降级路径 |
| R4 | 深度接管 KOReader 刷新入口的接口变动风险 | 中（第二阶段） | 第一阶段只做手动桥（§6.4），不 patch KOReader 本体；第二阶段对齐 nightly 的 einkfb 后端后再做 | 深度接管异常 → 删 k4refresh.lua 即回到原生 |
| R5 | KOReader 2026.07.1 缺 #2481 修复导致"全刷偶尔无效" | 低 | 本方案全刷走 slow（豁免 no-op），天然规避；建议升级 KOReader nightly 对齐 | 无需处理 |
| R6 | 固件变体差异（同代 K4 不同地区固件） | 低 | ioctl 号与结构体为 einkfb 家族稳定 ABI（K2↔K4 通吃）；部署前跑 `info` 冒烟 | `info` 分辨率异常 → 停用并反馈 |
| R7 | 误写系统文件 | 低 | 全部产物仅落 /mnt/us；脚本无任何系统分区写操作（k4r.sh 只调 CLI + eips） | §8.1 备份还原 |
| R8 | 不可逆操作 | — | **本方案无不可逆操作**（无刷机/写分区/改内核）；所有变更可通过 §8.2 完整撤销 | — |

**用户二次确认要求**：只有当未来扩展涉及 KUAL 之外的系统分区写入（如改 `/etc/kdb.*`）时才需要；本方案范围内无需。

---

## 10. 术语表

| 术语 | 解释 |
|------|------|
| einkfb | Amazon 早期 Kindle 的 eink 帧缓冲驱动（K2~K4），提供 `FBIO_EINK_*` ioctl |
| mxcfb | 后续机型的驱动（Touch 起），才有 waveform/marker/wait 概念——K4 上不存在 |
| fx（fx_type） | einkfb 的刷新类型枚举：partial/full/fast/slow（+flash/invert 特效） |
| partial / fast | 不闪烁的局部刷新；fast 更快但把页面量化成近黑白 |
| slow | 闪烁的高保真全刷，驱动对它豁免"内容未变跳过"检查 |
| no-op 跳过 | 驱动对内容未变的刷新请求静默丢弃的行为（koreader#12774） |
| armhf / hardfp | ARM 硬件浮点 ABI；K4 FW 4.x 用户态即此 ABI（`/lib/ld-linux-armhf.so.3`） |
| musl / glibc | 两种 C 库；musl 利于静态链接，glibc 分发动态库。本方案 .so 走 glibc、CLI 可选 musl 静态 |
| Ghost Budget | 残影预算策略：翻页累计 N 次后触发一次清残影全刷 |
| LuaJIT FFI | KOReader 的 C 接口调用机制，`lua/k4refresh.lua` 即基于此 |

---

## 11. 参考来源（本轮实际核查）

- FBInk 仓库与 `fbink.c` `refresh_legacy()` / `eink/einkfb.h`：<https://github.com/NiLuJe/FBInk>
- KOReader `frontend/device/kindle/device.lua`（Kindle4 定义 / hardfp）：<https://github.com/koreader/koreader/blob/master/frontend/device/kindle/device.lua>
- KOReader base `ffi/framebuffer_einkfb.lua`、`ffi/einkfb_h.lua`：<https://github.com/koreader/koreader-base/tree/master/ffi>
- Issue #12774（K4 全刷 no-op 行为实录）：<https://github.com/koreader/koreader/issues/12774>
- koreader-base#2481（slow 豁免修复，2026-08）：<https://github.com/koreader/koreader-base/pull/2481>
- FBInk Rust 绑定（fbink-sys / fbink-rs，Rust↔eink FFI 生态佐证）：见 FBInk README "Bindings in other languages"
- rust musl cdylib 限制（`dropping unsupported crate type`）：rust-lang/rust#110509、cargo#8607

**待实机确认项汇总**：内核精确版本（§1.1）、fast 档残影幅度与最优 interval（§3 优化 A / §7.2）。
