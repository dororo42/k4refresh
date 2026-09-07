//! bench：对指定 fx 逐次执行「写满屏交替图案 → 整屏区域刷新 → 计时」。
//!
//! 计时只覆盖 ioctl 提交耗时（驱动排队返回的时间），不等待面板完成——
//! legacy einkfb 没有等待 ioctl，面板完成时间以文档 §7 的主观/拍摄方法评估。
//! 图案用 8px 棋盘格逐次反相，保证每次刷新都有大面积像素变化，
//! 避开驱动「内容未变则跳过」的 no-op 逻辑污染数据。

use k4refresh::eink::{update_area, FixScreenInfo, UpdateArea, VarScreenInfo};
use std::io::{self, Write};
use std::time::Instant;

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

/// 执行一轮 bench。返回写入的 CSV 行数（含失败行，失败行 error 列非 0）。
pub fn run(
    fd: std::os::raw::c_int,
    v: &VarScreenInfo,
    f: &FixScreenInfo,
    fx_list: &[i32],
    n: u32,
    out_path: &str,
) -> io::Result<usize> {
    // mmap 必须用真实 fd；长度取驱动报告的 smem_len，越界写不可能超出该长度。
    // unsafe：仅 CLI 使用，生命周期由本函数内的显式 munmap 封闭。
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
    let fb = ptr as *mut u8;

    let mut out = io::BufWriter::new(std::fs::File::create(out_path)?);
    writeln!(out, "fx,seq,ioctl_usec,error")?;
    let mut rows = 0usize;

    for &fxv in fx_list {
        for i in 0..n {
            // 先写图案再计时：计时窗口只含 ioctl，不含绘制。
            draw_pattern(fb, v, f, i);
            let area = UpdateArea::new(0, 0, v.xres as i32, v.yres as i32, fxv);
            let t0 = Instant::now();
            let r = update_area(fd, &area);
            let usec = t0.elapsed().as_micros();
            match r {
                Ok(()) => writeln!(out, "{fxv},{i},{usec},0")?,
                Err(e) => writeln!(out, "{fxv},{i},{usec},{}", e.raw_os_error().unwrap_or(-1))?,
            }
            rows += 1;
        }
    }
    out.flush()?;

    unsafe {
        if libc::munmap(ptr, len) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(rows)
}
