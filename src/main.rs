//! CLI：info / refresh / flash / bench 四个子命令。
//!
//! bench 是方案文档 §6 的实机测量载体：mmap framebuffer 逐屏写交替测试图案，
//! 对每种 fx 计时 N 次 ioctl 提交耗时，输出 CSV（设备端测试清单的原始数据源）。

mod bench;

use std::ffi::CString;
use std::process::ExitCode;

const FB_PATH: &str = "/dev/fb0";

fn usage() -> ! {
    eprintln!(
        "k4refresh-cli v{} — Kindle 4 einkfb 刷新控制器\n\
         \n\
         用法:\n\
         \x20 k4refresh-cli --version                    打印版本\n\
         \x20 k4refresh-cli info                          查询屏幕信息\n\
         \x20 k4refresh-cli refresh [--kind page|ui|full] [--mode fast|conservative]\n\
         \x20                 [--interval N] [--x1 A --y1 B --x2 C --y2 D]\n\
         \x20 k4refresh-cli flash                         整屏 fx_update_slow 清残影\n\
         \x20 k4refresh-cli restore [--mode 0|1] [--cmd new|old|auto]\n\
         \x20                 virtual_fb 整帧重推（RESTORE_DISPLAY；full 由驱动升级 slow）\n\
         \x20 k4refresh-cli fxupdate --mode 0|1 --which -1|21 [--exclude X1,Y1,X2,Y2]\n\
         \x20                 [--exclude2 X1,Y1,X2,Y2] [--cmd new|old|auto]\n\
         \x20                 UPDATE_DISPLAY_FX：整屏 + 逐像素 fx + 排除矩形\n\
         \x20 k4refresh-cli bench [--fx partial,fast,slow] [--n 20] [--label A]\n\
         \x20                 [--delay-ms 3000] [--out latency.csv] [--sync]\n\
         \x20                 [--seq fast,fast,fast,slow] [--pattern checker|gray] [--quant]\n\
         \n\
         fx 取值: partial=0 fast=2 slow=3（legacy einkfb 无 waveform 概念）\n\
         fxupdate --which: -1=无变换 21=反色（fx_t Shim 变换）",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(2);
}

struct Args(Vec<String>);
impl Args {
    fn flag(&self, name: &str) -> Option<String> {
        self.0
            .iter()
            .position(|a| a == name)
            .and_then(|i| self.0.get(i + 1))
            .cloned()
    }
    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }
}

fn open_fb_or_die() -> (std::os::raw::c_int, k4refresh::eink::VarScreenInfo) {
    let path = CString::new(FB_PATH).expect("static path");
    let fd = k4refresh::eink::open_fb(&path).unwrap_or_else(|e| {
        eprintln!("错误: 打开 {FB_PATH} 失败: {e}（需要 root / USBNetwork shell）");
        std::process::exit(1);
    });
    let v = k4refresh::eink::var_info(fd).unwrap_or_else(|e| {
        eprintln!("错误: 读取屏幕信息失败: {e}");
        std::process::exit(1);
    });
    (fd, v)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let a = Args(args);
    match a.0[0].as_str() {
        "--version" | "version" => {
            println!("k4refresh-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "info" => {
            let (fd, v) = open_fb_or_die();
            let f = k4refresh::eink::fix_info(fd).unwrap_or_else(|e| {
                eprintln!("错误: 读取 fb 布局失败: {e}");
                std::process::exit(1);
            });
            println!("version     : {}", env!("CARGO_PKG_VERSION"));
            println!("device      : {FB_PATH}");
            println!("resolution  : {} x {}", v.xres, v.yres);
            println!("virtual     : {} x {}", v.xres_virtual, v.yres_virtual);
            println!("bpp         : {}", v.bits_per_pixel);
            println!("line_length : {}", f.line_length);
            println!("smem_len    : {}", f.smem_len);
            println!("id          : {}", String::from_utf8_lossy(&f.id).trim_end().to_string());
            k4refresh::eink::close_fd(fd);
            ExitCode::SUCCESS
        }
        "refresh" => {
            let kind = match a.flag("--kind").as_deref() {
                Some("ui") => k4refresh::fx::KIND_UI,
                Some("full") => k4refresh::fx::KIND_FULL,
                _ => k4refresh::fx::KIND_PAGE_TURN,
            };
            let mode = match a.flag("--mode").as_deref() {
                Some("conservative") => k4refresh::fx::MODE_CONSERVATIVE,
                _ => k4refresh::fx::MODE_FAST,
            };
            let interval: u32 = a
                .flag("--interval")
                .and_then(|s| s.parse().ok())
                .unwrap_or(6);
            k4refresh::set_mode(mode, interval);

            let (fd, v) = open_fb_or_die();
            let (x1, y1, x2, y2) = if a.has("--x1") {
                (
                    a.flag("--x1").and_then(|s| s.parse().ok()).unwrap_or(0),
                    a.flag("--y1").and_then(|s| s.parse().ok()).unwrap_or(0),
                    a.flag("--x2").and_then(|s| s.parse().ok()).unwrap_or(v.xres as i32),
                    a.flag("--y2").and_then(|s| s.parse().ok()).unwrap_or(v.yres as i32),
                )
            } else {
                (0, 0, v.xres as i32, v.yres as i32)
            };
            let fxv = k4refresh::refresh(fd, kind, x1, y1, x2, y2);
            match fxv {
                Ok(n) => {
                    println!("ok: fx={n}（0=partial 2=fast 3=slow；0 也可能是空区域跳过）");
                    k4refresh::eink::close_fd(fd);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("错误: 刷新失败: {e}");
                    k4refresh::eink::close_fd(fd);
                    ExitCode::FAILURE
                }
            }
        }
        "flash" => {
            let (fd, _) = open_fb_or_die();
            match k4refresh::flash_full(fd) {
                Ok(_) => {
                    println!("ok: fx_update_slow 整屏刷新完成");
                    k4refresh::eink::close_fd(fd);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("错误: {e}");
                    k4refresh::eink::close_fd(fd);
                    ExitCode::FAILURE
                }
            }
        }
        "restore" => {
            // E3 探针：RESTORE_DISPLAY 整帧重推。新编号 0x46ef 失败时试老编号
            // 0x4644（KindleApp 4.1.4 二进制内出现，编号代际待实测）。
            let mode: i32 = a.flag("--mode").and_then(|s| s.parse().ok()).unwrap_or(1);
            let cmd_pref = a.flag("--cmd").unwrap_or_else(|| "auto".into());
            let (fd, _) = open_fb_or_die();
            let attempt = |cmd: u64| k4refresh::eink::restore_display(fd, cmd, mode);
            let result = match cmd_pref.as_str() {
                "new" => attempt(k4refresh::eink::FBIO_EINK_RESTORE_DISPLAY_NEW)
                    .map(|_| "new(0x46ef)"),
                "old" => attempt(k4refresh::eink::FBIO_EINK_RESTORE_DISPLAY_OLD)
                    .map(|_| "old(0x4644)"),
                _ => attempt(k4refresh::eink::FBIO_EINK_RESTORE_DISPLAY_NEW)
                    .map(|_| "new(0x46ef)")
                    .or_else(|_| {
                        attempt(k4refresh::eink::FBIO_EINK_RESTORE_DISPLAY_OLD)
                            .map(|_| "old(0x4644)")
                    }),
            };
            k4refresh::eink::close_fd(fd);
            match result {
                Ok(which) => {
                    println!("ok: restore 完成（cmd={which}, mode={mode}）");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("错误: restore 失败（新旧编号均试）: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "fxupdate" => {
            // E2 探针：UPDATE_DISPLAY_FX（fx_t：波形档 + 逐像素变换 + 排除矩形）。
            let mode: i32 = a.flag("--mode").and_then(|s| s.parse().ok()).unwrap_or(1);
            let which: i32 = a.flag("--which").and_then(|s| s.parse().ok()).unwrap_or(-1);
            let cmd_pref = a.flag("--cmd").unwrap_or_else(|| "auto".into());
            let (fd, v) = open_fb_or_die();
            let mut fx = k4refresh::eink::FxStruct::new(mode, which);
            for key in ["--exclude", "--exclude2"] {
                if let Some(s) = a.flag(key) {
                    let parts: Vec<i32> = s.split(',').filter_map(|t| t.trim().parse().ok()).collect();
                    if parts.len() == 4 {
                        fx.push_exclude(parts[0], parts[1], parts[2], parts[3]);
                    } else {
                        eprintln!("警告: {key} 格式应为 X1,Y1,X2,Y2，已忽略");
                    }
                }
            }
            let attempt =
                |cmd: u64| k4refresh::eink::update_display_fx(fd, cmd, &fx);
            let result = match cmd_pref.as_str() {
                "new" => attempt(k4refresh::eink::FBIO_EINK_UPDATE_DISPLAY_FX_NEW)
                    .map(|_| "new(0x46e4)"),
                "old" => attempt(k4refresh::eink::FBIO_EINK_UPDATE_DISPLAY_FX_OLD)
                    .map(|_| "old(0x4642)"),
                _ => attempt(k4refresh::eink::FBIO_EINK_UPDATE_DISPLAY_FX_NEW)
                    .map(|_| "new(0x46e4)")
                    .or_else(|_| {
                        attempt(k4refresh::eink::FBIO_EINK_UPDATE_DISPLAY_FX_OLD)
                            .map(|_| "old(0x4642)")
                    }),
            };
            k4refresh::eink::close_fd(fd);
            match result {
                Ok(which_cmd) => {
                    let _ = v;
                    println!(
                        "ok: fxupdate 完成（cmd={which_cmd}, mode={mode}, which={which}, excludes={})",
                        fx.num_exclude_rects
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("错误: fxupdate 失败（新旧编号均试）: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "bench" => {
            let fx_list = bench::parse_fx_list(&a.flag("--fx").unwrap_or_else(|| "partial,fast,slow".into()));
            let n: u32 = a.flag("--n").and_then(|s| s.parse().ok()).unwrap_or(20);
            let out = a.flag("--out").unwrap_or_else(|| "latency.csv".into());
            let (fd, v) = open_fb_or_die();
            // bench 逐字节写帧假设 8bpp；非 8bpp 面板上会错位写花显存，直接拒绝。
            if v.bits_per_pixel != 8 {
                k4refresh::eink::close_fd(fd);
                eprintln!(
                    "错误: bpp={} ≠ 8，bench 的逐字节写帧仅适配 8bpp（K4），拒绝执行",
                    v.bits_per_pixel
                );
                return ExitCode::FAILURE;
            }
            let f = k4refresh::eink::fix_info(fd).unwrap_or_else(|e| {
                eprintln!("错误: {e}");
                std::process::exit(1);
            });
            let label = a
                .flag("--label")
                .unwrap_or_default()
                .trim()
                .to_uppercase();
            let label = if label.is_empty() { "T".into() } else { label };
            let delay_ms: u64 = a
                .flag("--delay-ms")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let pattern = bench::parse_pattern(&a.flag("--pattern").unwrap_or_default());
            let quant = a.has("--quant");
            let sync = a.has("--sync");
            let rc = match a.flag("--seq").map(|s| bench::parse_fx_list(&s)) {
                Some(seq) if seq.is_empty() => {
                    k4refresh::eink::close_fd(fd);
                    eprintln!("错误: --seq 未解析出有效 fx（可用: partial,fast,slow）");
                    return ExitCode::FAILURE;
                }
                Some(seq) => bench::run_seq(
                    fd, &v, &f, &seq, n, &out, &label, delay_ms, pattern, quant, sync,
                ),
                None => bench::run(
                    fd, &v, &f, &fx_list, n, &out, &label, delay_ms, pattern, quant, sync,
                ),
            };
            k4refresh::eink::close_fd(fd);
            match rc {
                Ok((rows, errors)) => {
                    println!("bench 完成: {rows} 行写入 {out}（{errors} 次 ioctl 失败）；屏幕已恢复");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("错误: bench 失败: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => usage(),
    }
}
