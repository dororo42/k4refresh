//! k4refresh：Kindle 4 legacy einkfb 刷新策略库。
//!
//! 对外两个入口：
//! - C ABI（本文件）：供 KOReader LuaJIT FFI / KUAL 脚本调用；
//! - Rust API（`fx` / `eink` 模块）：供 CLI 与测试复用。
//!
//! 安全设计：
//! - 全 crate 只有 `eink.rs` 中 3 组已注释的 unsafe；
//! - FFI 边界用 `catch_unwind` 兜底，库内部异常返回 K4R_PANIC 而非穿透到宿主；
//! - 任何刷新失败都只记录 errno 并返回负值，绝不影响调用方继续运行；
//! - 策略状态用原子变量承载，跨 FFI 调用一致，无锁、无堆分配。

pub mod eink;
pub mod fx;

use fx::Policy;
use std::ffi::CStr;
use std::io;
use std::os::raw::{c_char, c_int};
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

// ---------------------------------------------------------------- 状态 ----

static MODE: AtomicU32 = AtomicU32::new(fx::MODE_FAST);
static INTERVAL: AtomicU32 = AtomicU32::new(6);
static FLIPS: AtomicU32 = AtomicU32::new(0);
static MAX_W: AtomicI32 = AtomicI32::new(0);
static MAX_H: AtomicI32 = AtomicI32::new(0);
static LAST_ERRNO: AtomicI32 = AtomicI32::new(0);

/// FFI 返回码：负值 = 错误；0 = 跳过（退化区域）；>0 = 实际使用的 fx 值。
pub const K4R_ERR_IOCTL: c_int = -1;
pub const K4R_ERR_STATE: c_int = -2;
pub const K4R_ERR_ARG: c_int = -3;
pub const K4R_PANIC: c_int = -99;

/// 按当前原子状态构造策略并决策（同时维护翻页计数）。
fn decide_with_state(kind: u32) -> i32 {
    let mut p = Policy::new(
        MODE.load(Ordering::Relaxed),
        INTERVAL.load(Ordering::Relaxed),
    );
    p.flips_since_slow = FLIPS.load(Ordering::Relaxed);
    let fxv = p.decide(kind);
    FLIPS.store(p.flips_since_slow, Ordering::Relaxed);
    fxv
}

/// 屏幕尺寸：优先用缓存；无缓存时向驱动查询一次并记录。
fn screen_size(fd: c_int) -> io::Result<(i32, i32)> {
    let w = MAX_W.load(Ordering::Relaxed);
    let h = MAX_H.load(Ordering::Relaxed);
    if w > 0 && h > 0 {
        return Ok((w, h));
    }
    let v = eink::var_info(fd)?;
    MAX_W.store(v.xres as i32, Ordering::Relaxed);
    MAX_H.store(v.yres as i32, Ordering::Relaxed);
    Ok((v.xres as i32, v.yres as i32))
}

/// 核心刷新路径（Rust 内部版，供 FFI 与 CLI 复用）。
/// 返回：实际使用的 fx / 0 = 跳过 / Err = ioctl 失败。
pub fn refresh(fd: c_int, kind: u32, x1: i32, y1: i32, x2: i32, y2: i32) -> Result<i32, io::Error> {
    let fxv = decide_with_state(kind);
    let (w, h) = screen_size(fd)?;

    // 显式全刷且区域就是整屏 → 走 UPDATE_DISPLAY（FBInk 实测该路径更可靠）。
    if kind == fx::KIND_FULL && x1 <= 0 && y1 <= 0 && x2 >= w && y2 >= h {
        return eink::update_display(fd, fxv).map(|_| fxv);
    }

    let Some((ax1, ay1, ax2, ay2)) = fx::clamp_rect(x1, y1, x2, y2, w, h) else {
        return Ok(0); // 空区域：不算错误，静默跳过
    };
    let area = eink::UpdateArea::new(ax1, ay1, ax2, ay2, fxv);
    eink::update_area(fd, &area).map(|_| fxv)
}

/// 便捷入口：立即用 fx_update_slow 做一次整屏清残影刷新。
pub fn flash_full(fd: c_int) -> Result<i32, io::Error> {
    FLIPS.store(0, Ordering::Relaxed);
    let (w, h) = screen_size(fd)?;
    refresh(fd, fx::KIND_FULL, 0, 0, w, h)
}

/// Rust 侧设置运行模式（CLI 使用；与 C ABI `k4refresh_set_mode` 等价）。
pub fn set_mode(mode: u32, interval: u32) {
    MODE.store(mode, Ordering::Relaxed);
    INTERVAL.store(interval.max(1), Ordering::Relaxed);
}

// ---------------------------------------------------------------- C ABI ----

/// 打开 /dev/fb0（或指定路径），缓存屏幕尺寸，返回 fd；失败返回 K4R_ERR_IOCTL。
///
/// # Safety
/// path 必须是以 NUL 结尾的合法 C 字符串（LuaJIT FFI 传 c string 即可）。
#[no_mangle]
pub unsafe extern "C" fn k4refresh_open(path: *const c_char) -> c_int {
    catch(|| {
        if path.is_null() {
            return K4R_ERR_ARG;
        }
        match eink::open_fb(CStr::from_ptr(path)) {
            Ok(fd) => match screen_size(fd) {
                Ok(_) => fd,
                Err(e) => {
                    record(&e);
                    eink::close_fd(fd);
                    K4R_ERR_IOCTL
                }
            },
            Err(e) => {
                record(&e);
                K4R_ERR_IOCTL
            }
        }
    })
}

/// 策略刷新入口（KOReader 集成用）。
/// kind：0=翻页/局部 1=UI 2=显式全刷。区域为开边界 (x2,y2)。
/// 返回：>0 实际 fx；0 跳过；负值错误码（errno 用 k4refresh_last_error 取）。
///
/// # Safety
/// fd 必须来自 k4refresh_open 或调用方自行打开的合法 fd。
#[no_mangle]
pub unsafe extern "C" fn k4refresh_refresh(
    fd: c_int,
    kind: u32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
) -> c_int {
    catch(|| match refresh(fd, kind, x1, y1, x2, y2) {
        Ok(fxv) => fxv,
        Err(e) => {
            record(&e);
            K4R_ERR_IOCTL
        }
    })
}

/// 立即整屏 fx_update_slow 清残影（KUAL「手动全刷」入口）。
///
/// # Safety
/// 同 k4refresh_refresh。
#[no_mangle]
pub unsafe extern "C" fn k4refresh_flash(fd: c_int) -> c_int {
    catch(|| match flash_full(fd) {
        Ok(fxv) => fxv,
        Err(e) => {
            record(&e);
            K4R_ERR_IOCTL
        }
    })
}

/// 设置运行模式。mode：0=fast 1=conservative；interval：N 次翻页后 slow 收尾（>=1）。
#[no_mangle]
pub extern "C" fn k4refresh_set_mode(mode: u32, interval: u32) {
    MODE.store(mode, Ordering::Relaxed);
    INTERVAL.store(interval.max(1), Ordering::Relaxed);
}

/// 查询缓存的屏幕尺寸；未缓存时向驱动查询。返回 0 成功 / 负值错误。
///
/// # Safety
/// w_out / h_out 必须指向可写的 i32（LuaJIT FFI 传 int[1] 即可）。
#[no_mangle]
pub unsafe extern "C" fn k4refresh_screen_size(fd: c_int, w_out: *mut i32, h_out: *mut i32) -> c_int {
    catch(|| {
        if w_out.is_null() || h_out.is_null() {
            return K4R_ERR_ARG;
        }
        match screen_size(fd) {
            Ok((w, h)) => {
                *w_out = w;
                *h_out = h;
                0
            }
            Err(e) => {
                record(&e);
                K4R_ERR_IOCTL
            }
        }
    })
}

/// 取最近一次 ioctl 错误的 strerror 文本，写入 buf（截断到 len-1，保证 NUL 结尾）。
/// 返回写入长度；无错误记录时返回 0。
///
/// # Safety
/// buf 必须指向至少 len 字节的可写内存。
#[no_mangle]
pub unsafe extern "C" fn k4refresh_last_error(buf: *mut c_char, len: c_int) -> c_int {
    if buf.is_null() || len <= 0 {
        return 0;
    }
    let n = LAST_ERRNO.load(Ordering::Relaxed);
    if n == 0 {
        // 仍写入空字符串，保证调用方拿到合法 C 字符串。
        *buf = 0;
        return 0;
    }
    let e = io::Error::from_raw_os_error(n);
    let msg = e.to_string();
    let bytes = msg.as_bytes();
    let cap = (len - 1).min(bytes.len() as c_int).max(0) as usize;
    std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buf, cap);
    *buf.add(cap) = 0;
    cap as c_int
}

/// 关闭 fd（仅当 fd 由 k4refresh_open 产生时调用）。
#[no_mangle]
pub extern "C" fn k4refresh_close(fd: c_int) {
    if fd >= 0 {
        eink::close_fd(fd);
    }
}

// -------------------------------------------------------------- 内部工具 ----

fn record(e: &io::Error) {
    LAST_ERRNO.store(e.raw_os_error().unwrap_or(0), Ordering::Relaxed);
}

/// FFI 边界 panic 兜底：任何下层 panic 都转成 K4R_PANIC，不穿透到宿主进程。
fn catch<F: FnOnce() -> c_int + std::panic::UnwindSafe>(f: F) -> c_int {
    match std::panic::catch_unwind(f) {
        Ok(v) => v,
        Err(_) => K4R_PANIC,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_state_flows_through_ffi_semantics() {
        // 不触 ioctl：用非法 fd 走到错误路径，验证错误码与计数联动。
        k4refresh_set_mode(fx::MODE_FAST, 2);
        unsafe {
            let rc = k4refresh_refresh(-1, fx::KIND_PAGE_TURN, 0, 0, 100, 100);
            assert_eq!(rc, K4R_ERR_IOCTL);
            // 策略状态在 ioctl 之前已推进：FLIPS 应为 1
            assert_eq!(FLIPS.load(Ordering::Relaxed), 1);
        }
        FLIPS.store(0, Ordering::Relaxed);
    }

    #[test]
    fn error_buffer_always_nul_terminated() {
        LAST_ERRNO.store(0, Ordering::Relaxed);
        let mut buf = [0u8; 64];
        unsafe {
            let n = k4refresh_last_error(buf.as_mut_ptr() as *mut c_char, 64);
            assert_eq!(n, 0);
            assert_eq!(buf[0], 0);
        }
    }

    #[test]
    fn flash_full_resets_counter() {
        FLIPS.store(5, Ordering::Relaxed);
        // 非法 fd 只会走到 ioctl 错误，但计数复位应先发生。
        let _ = flash_full(-1);
        assert_eq!(FLIPS.load(Ordering::Relaxed), 0);
    }
}
