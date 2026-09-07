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
        "k4refresh-cli — Kindle 4 einkfb 刷新控制器\n\
         \n\
         用法:\n\
         \x20 k4refresh-cli info                          查询屏幕信息\n\
         \x20 k4refresh-cli refresh [--kind page|ui|full] [--mode fast|conservative]\n\
         \x20                 [--interval N] [--x1 A --y1 B --x2 C --y2 D]\n\
         \x20 k4refresh-cli flash                         整屏 fx_update_slow 清残影\n\
         \x20 k4refresh-cli bench [--fx partial,fast,slow] [--n 20] [--out latency.csv]\n\
         \n\
         fx 取值: partial=0 fast=2 slow=3（legacy einkfb 无 waveform 概念）"
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
        "info" => {
            let (fd, v) = open_fb_or_die();
            let f = k4refresh::eink::fix_info(fd).unwrap_or_else(|e| {
                eprintln!("错误: 读取 fb 布局失败: {e}");
                std::process::exit(1);
            });
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
                Ok(n) if n >= 0 => {
                    println!("ok: fx={n}（0=partial 2=fast 3=slow；0 也可能是空区域跳过）");
                    k4refresh::eink::close_fd(fd);
                    ExitCode::SUCCESS
                }
                Ok(_) => {
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
        "bench" => {
            let fx_list: Vec<i32> = a
                .flag("--fx")
                .unwrap_or_else(|| "partial,fast,slow".into())
                .split(',')
                .filter_map(|s| match s.trim() {
                    "partial" => Some(k4refresh::fx::FX_PARTIAL),
                    "fast" => Some(k4refresh::fx::FX_FAST),
                    "slow" => Some(k4refresh::fx::FX_SLOW),
                    _ => None,
                })
                .collect();
            let n: u32 = a.flag("--n").and_then(|s| s.parse().ok()).unwrap_or(20);
            let out = a.flag("--out").unwrap_or_else(|| "latency.csv".into());
            let (fd, v) = open_fb_or_die();
            let f = k4refresh::eink::fix_info(fd).unwrap_or_else(|e| {
                eprintln!("错误: {e}");
                std::process::exit(1);
            });
            let rc = bench::run(fd, &v, &f, &fx_list, n, &out);
            k4refresh::eink::close_fd(fd);
            match rc {
                Ok(errors) => {
                    if errors == 0 {
                        println!("bench 完成: 结果写入 {out}");
                        ExitCode::SUCCESS
                    } else {
                        println!("bench 完成（{errors} 次 ioctl 报错，已记入 CSV）: {out}");
                        ExitCode::SUCCESS
                    }
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
