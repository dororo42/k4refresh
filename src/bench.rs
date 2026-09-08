//! bench：对指定 fx 逐次执行「写满屏交替棋盘图案 → 整屏区域刷新 → 计时」。
//!
//! 计时只覆盖 ioctl 提交耗时（驱动排队返回的时间），不等待面板完成——
//! legacy einkfb 没有等待 ioctl，面板完成时间以文档 §7 的主观/拍摄方法评估。
//! 图案用 8px 棋盘格逐次反相，保证每次刷新都有大面积像素变化，
//! 避开驱动「内容未变则跳过」的 no-op 逻辑污染数据。
//!
//! 两种模式：
//! - run：单 fx 独立计时（`--fx partial,fast,slow`）；
//! - run_seq：组合序列计时（`--seq fast,fast,fast,slow`），模拟真实翻页周期
//!   （fast×N + slow×1 收尾），CSV 增加 round/step 两列。

use k4refresh::eink::{update_area, FixScreenInfo, UpdateArea, VarScreenInfo};
use k4refresh::fx::{FX_FAST, FX_PARTIAL, FX_SLOW};
use std::io::{self, Write};
use std::os::raw::c_int;
use std::time::Instant;

/// 解析 "partial,fast,slow" 形式的 fx 列表；非法 token 忽略。
pub fn parse_fx_list(s: &str) -> Vec<i32> {
    s.split(',')
        .filter_map(|t| match t.trim() {
            "partial" => Some(FX_PARTIAL),
            "fast" => Some(FX_FAST),
            "slow" => Some(FX_SLOW),
            _ => None,
        })
        .collect()
}

/// 写第 frame 帧棋盘图案（8px 方块，黑白反相）。
/// stride 取 line_length（可能大于 xres），只写可见区，避免污染驱动私有区。
fn draw_pattern(fb: *mut u8, v: &VarScreenInfo, f: &FixScreenInfo, frame: u32) {
    unsafe {
        for y in 0..v.yres as usize {
            let row = fb.add(y * f.line_length as usize);
            for x in 0..v.xres as usize {
                let block = ((x / 8) + (y / 8) + frame as usize) % 2;
                *row.add(x) = if block == 0 { 0x00 } else { 0xFF };
            }
        }
    }
}

/// mmap framebuffer（长度取驱动报告的 smem_len，越界写不可能超出该长度）。
/// unsafe：仅 CLI 使用，生命周期由配对的 unmap_fb 封闭。
fn map_fb(fd: c_int, f: &FixScreenInfo) -> io::Result<(*mut u8, usize)> {
    let len = f.smem_len as usize;
    if len == 0 {
        return Err(io::Error::new(io::ErrorKind::Other, "smem_len == 0"));
    }
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok((ptr as *mut u8, len))
}

fn unmap_fb(ptr: *mut u8, len: usize) -> io::Result<()> {
    if unsafe { libc::munmap(ptr as *mut libc::c_void, len) } != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// 单次测量：写图案（不计入计时）→ 整屏区域刷新 → 计时。
/// 返回 (ioctl 提交耗时 usec, 失败时的 errno)。
fn time_update(
    fd: c_int,
    fb: *mut u8,
    v: &VarScreenInfo,
    f: &FixScreenInfo,
    fxv: i32,
    frame: u32,
) -> (u128, Option<i32>) {
    draw_pattern(fb, v, f, frame);
    let area = UpdateArea::new(0, 0, v.xres as i32, v.yres as i32, fxv);
    let t0 = Instant::now();
    let r = update_area(fd, &area);
    let usec = t0.elapsed().as_micros();
    (usec, r.err().and_then(|e| e.raw_os_error()))
}

/// 模式一：单 fx 独立计时。返回写入的 CSV 行数（含失败行，error 列非 0）。
pub fn run(
    fd: c_int,
    v: &VarScreenInfo,
    f: &FixScreenInfo,
    fx_list: &[i32],
    n: u32,
    out_path: &str,
) -> io::Result<usize> {
    let (fb, len) = map_fb(fd, f)?;
    let mut out = io::BufWriter::new(std::fs::File::create(out_path)?);
    writeln!(out, "fx,seq,ioctl_usec,error")?;
    let mut rows = 0usize;

    for &fxv in fx_list {
        for i in 0..n {
            let (usec, err) = time_update(fd, fb, v, f, fxv, i);
            match err {
                None => writeln!(out, "{fxv},{i},{usec},0")?,
                Some(errno) => writeln!(out, "{fxv},{i},{usec},{errno}")?,
            }
            rows += 1;
        }
    }
    out.flush()?;
    unmap_fb(fb, len)?;
    Ok(rows)
}

/// 模式二：组合序列计时（如 fast,fast,fast,slow = 一次典型翻页收尾周期）。
/// 共 rounds 轮，每轮按 seq 顺序逐个测量。CSV 列：fx,round,step,ioctl_usec,error。
pub fn run_seq(
    fd: c_int,
    v: &VarScreenInfo,
    f: &FixScreenInfo,
    seq: &[i32],
    rounds: u32,
    out_path: &str,
) -> io::Result<usize> {
    if seq.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty seq"));
    }
    let (fb, len) = map_fb(fd, f)?;
    let mut out = io::BufWriter::new(std::fs::File::create(out_path)?);
    writeln!(out, "fx,round,step,ioctl_usec,error")?;
    let mut rows = 0usize;

    for round in 0..rounds {
        for (step, &fxv) in seq.iter().enumerate() {
            // 帧号全局递增，保证相邻两次刷新图案必反相（避开 no-op 跳过）。
            let frame = round * seq.len() as u32 + step as u32;
            let (usec, err) = time_update(fd, fb, v, f, fxv, frame);
            match err {
                None => writeln!(out, "{fxv},{round},{step},{usec},0")?,
                Some(errno) => writeln!(out, "{fxv},{round},{step},{usec},{errno}")?,
            }
            rows += 1;
        }
    }
    out.flush()?;
    unmap_fb(fb, len)?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fx_list_tokens() {
        assert_eq!(
            parse_fx_list("partial,fast,slow"),
            vec![FX_PARTIAL, FX_FAST, FX_SLOW]
        );
        // 允许空白与末尾逗号
        assert_eq!(parse_fx_list(" fast , slow ,"), vec![FX_FAST, FX_SLOW]);
        // 非法 token 全部忽略
        assert!(parse_fx_list("bogus,").is_empty());
        assert!(parse_fx_list("").is_empty());
    }
}
