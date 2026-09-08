//! bench：对指定 fx 逐次执行「写满屏交替棋盘图案（叠加 A1/B1 等计数标记）→ 整屏区域刷新 → 计时」。
//!
//! 计时只覆盖 ioctl 提交耗时（驱动排队返回的时间），不等待面板完成——
//! legacy einkfb 没有等待 ioctl，面板完成时间以文档 §7 的主观/拍摄方法评估。
//! 图案用 8px 棋盘格逐次反相，保证每次刷新都有大面积像素变化，
//! 避开驱动「内容未变则跳过」的 no-op 逻辑污染数据。
//! 左上角以 5x7 点阵字体渲染当前次序标记（如 A1、B2），供真机肉眼计数。
//! 结束时把进入前的 framebuffer 原样写回并整屏刷新一次，不留测试图案。
//!
//! 两种模式：
//! - run：单 fx 独立计时（`--fx partial,fast,slow --label A --n 3`）；
//! - run_seq：组合序列计时（`--seq fast,fast,fast,slow --label C --n 15`），
//!   模拟真实翻页周期（fast×N + slow×1 收尾），CSV 增加 round/step 两列。

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

/// 5x7 列式点阵字模（bit0 = 顶行像素）。只含 bench 标记所需字符。
const FONT5X7: &[(&str, [u8; 5])] = &[
    ("A", [0x7C, 0x12, 0x11, 0x12, 0x7C]),
    ("B", [0x7F, 0x49, 0x49, 0x49, 0x36]),
    ("C", [0x3E, 0x41, 0x41, 0x41, 0x22]),
    ("D", [0x7F, 0x41, 0x41, 0x22, 0x1C]),
    ("F", [0x7F, 0x09, 0x09, 0x09, 0x01]),
    ("P", [0x7F, 0x09, 0x09, 0x09, 0x06]),
    ("S", [0x46, 0x49, 0x49, 0x49, 0x31]),
    ("T", [0x01, 0x01, 0x7F, 0x01, 0x01]),
    ("0", [0x3E, 0x51, 0x49, 0x45, 0x3E]),
    ("1", [0x00, 0x42, 0x7F, 0x40, 0x00]),
    ("2", [0x42, 0x61, 0x51, 0x49, 0x46]),
    ("3", [0x21, 0x41, 0x45, 0x4B, 0x31]),
    ("4", [0x18, 0x14, 0x12, 0x7F, 0x10]),
    ("5", [0x27, 0x45, 0x45, 0x45, 0x39]),
    ("6", [0x3C, 0x4A, 0x49, 0x49, 0x30]),
    ("7", [0x01, 0x71, 0x09, 0x05, 0x03]),
    ("8", [0x36, 0x49, 0x49, 0x49, 0x36]),
    ("9", [0x06, 0x49, 0x49, 0x29, 0x1E]),
    (".", [0x00, 0x60, 0x60, 0x00, 0x00]),
    ("-", [0x08, 0x08, 0x08, 0x08, 0x08]),
    (" ", [0x00, 0x00, 0x00, 0x00, 0x00]),
];

fn glyph(ch: char) -> &'static [u8; 5] {
    let s: &str = &ch.to_string();
    FONT5X7
        .iter()
        .find(|(name, _)| *name == s)
        .map(|(_, g)| g)
        .unwrap_or(&FONT5X7[20].1) // 未知字符 → 空格
}

/// 在棋盘图案左上角渲染标记：白底黑字，4 倍放大，带留白。
fn draw_label(fb: *mut u8, v: &VarScreenInfo, f: &FixScreenInfo, label: &str) {
    const SCALE: usize = 4;
    let xres = v.xres as usize;
    let yres = v.yres as usize;
    let stride = f.line_length as usize;
    let put = |x: usize, y: usize, val: u8| unsafe {
        if x < xres && y < yres {
            *fb.add(y * stride + x) = val;
        }
    };
    let x0 = 16;
    let y0 = 16;
    // 白底（含四周留白），保证任何图案上都可读
    let box_w = label.chars().count() * (5 * SCALE + SCALE) + SCALE;
    let box_h = 7 * SCALE + SCALE * 2;
    for y in y0..y0 + box_h {
        for x in x0..x0 + box_w {
            put(x, y, 0xFF);
        }
    }
    for (ci, ch) in label.chars().enumerate() {
        let g = glyph(ch);
        let cx = x0 + SCALE / 2 + ci * (5 * SCALE + SCALE);
        let cy = y0 + SCALE;
        for (col, bits) in g.iter().enumerate() {
            for row in 0..7u32 {
                if bits & (1 << row) != 0 {
                    for dy in 0..SCALE {
                        for dx in 0..SCALE {
                            put(cx + col * SCALE + dx, cy + row as usize * SCALE + dy, 0x00);
                        }
                    }
                }
            }
        }
    }
}

/// 写第 frame 帧棋盘图案（8px 方块，黑白反相）+ 次序标记。
/// stride 取 line_length（可能大于 xres），只写可见区，避免污染驱动私有区。
fn draw_pattern(fb: *mut u8, v: &VarScreenInfo, f: &FixScreenInfo, frame: u32, label: &str) {
    unsafe {
        for y in 0..v.yres as usize {
            let row = fb.add(y * f.line_length as usize);
            for x in 0..v.xres as usize {
                let block = ((x / 8) + (y / 8) + frame as usize) % 2;
                *row.add(x) = if block == 0 { 0x00 } else { 0xFF };
            }
        }
    }
    draw_label(fb, v, f, label);
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

/// 单次测量：写图案+标记（不计入计时）→ 整屏区域刷新 → 计时。
/// 返回 (ioctl 提交耗时 usec, 失败时的 errno)。
fn time_update(
    fd: c_int,
    fb: *mut u8,
    v: &VarScreenInfo,
    f: &FixScreenInfo,
    fxv: i32,
    frame: u32,
    label: &str,
) -> (u128, Option<i32>) {
    draw_pattern(fb, v, f, frame, label);
    let area = UpdateArea::new(0, 0, v.xres as i32, v.yres as i32, fxv);
    let t0 = Instant::now();
    let r = update_area(fd, &area);
    let usec = t0.elapsed().as_micros();
    (usec, r.err().and_then(|e| e.raw_os_error()))
}

/// mmap 后立即备份当前 fb 内容；restore() 时写回并整屏 slow 刷一次恢复原画面。
struct FbGuard<'a> {
    fd: c_int,
    fb: *mut u8,
    v: &'a VarScreenInfo,
    saved: Vec<u8>,
    len: usize,
    restored: bool,
}

impl<'a> FbGuard<'a> {
    fn new(fd: c_int, fb: *mut u8, v: &'a VarScreenInfo, len: usize) -> Self {
        let saved = unsafe { std::slice::from_raw_parts(fb, len) }.to_vec();
        FbGuard { fd, fb, v, saved, len, restored: false }
    }
    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        unsafe {
            std::ptr::copy_nonoverlapping(self.saved.as_ptr(), self.fb, self.len);
        }
        let _ = update_area(
            self.fd,
            &UpdateArea::new(0, 0, self.v.xres as i32, self.v.yres as i32, FX_SLOW),
        );
    }
}

impl<'a> Drop for FbGuard<'a> {
    fn drop(&mut self) {
        self.restore();
    }
}

/// 模式一：单 fx 独立计时。返回 (CSV 行数, ioctl 失败次数)。
pub fn run(
    fd: c_int,
    v: &VarScreenInfo,
    f: &FixScreenInfo,
    fx_list: &[i32],
    n: u32,
    out_path: &str,
    label: &str,
) -> io::Result<(usize, usize)> {
    let (fb, len) = map_fb(fd, f)?;
    let guard = FbGuard::new(fd, fb, v, len);
    let mut out = io::BufWriter::new(std::fs::File::create(out_path)?);
    writeln!(out, "fx,seq,label,ioctl_usec,error")?;
    let mut rows = 0usize;
    let mut errors = 0usize;
    let mut counter = 0usize;

    for &fxv in fx_list {
        for i in 0..n {
            counter += 1;
            let mark = format!("{}{}", label, counter);
            let (usec, err) = time_update(fd, fb, v, f, fxv, i, &mark);
            match err {
                None => writeln!(out, "{fxv},{i},{mark},{usec},0")?,
                Some(errno) => {
                    errors += 1;
                    writeln!(out, "{fxv},{i},{mark},{usec},{errno}")?;
                }
            }
            rows += 1;
        }
    }
    out.flush()?;
    drop(guard);
    unmap_fb(fb, len)?;
    Ok((rows, errors))
}

/// 模式二：组合序列计时（如 fast,fast,fast,slow = 一次典型翻页收尾周期）。
/// 共 rounds 轮，每轮按 seq 顺序逐个测量。CSV 列：fx,round,step,label,ioctl_usec,error。
pub fn run_seq(
    fd: c_int,
    v: &VarScreenInfo,
    f: &FixScreenInfo,
    seq: &[i32],
    rounds: u32,
    out_path: &str,
    label: &str,
) -> io::Result<(usize, usize)> {
    if seq.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty seq"));
    }
    let (fb, len) = map_fb(fd, f)?;
    let guard = FbGuard::new(fd, fb, v, len);
    let mut out = io::BufWriter::new(std::fs::File::create(out_path)?);
    writeln!(out, "fx,round,step,label,ioctl_usec,error")?;
    let mut rows = 0usize;
    let mut errors = 0usize;

    for round in 0..rounds {
        for (step, &fxv) in seq.iter().enumerate() {
            // 帧号全局递增，保证相邻两次刷新图案必反相（避开 no-op 跳过）。
            let frame = round * seq.len() as u32 + step as u32;
            let mark = format!("{}.{}", label, round + 1);
            let mark = if seq.len() > 1 {
                format!("{}.{}", mark, step + 1)
            } else {
                mark
            };
            let (usec, err) = time_update(fd, fb, v, f, fxv, frame, &mark);
            match err {
                None => writeln!(out, "{fxv},{round},{step},{mark},{usec},0")?,
                Some(errno) => {
                    errors += 1;
                    writeln!(out, "{fxv},{round},{step},{mark},{usec},{errno}")?;
                }
            }
            rows += 1;
        }
    }
    out.flush()?;
    drop(guard);
    unmap_fb(fb, len)?;
    Ok((rows, errors))
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

    #[test]
    fn glyph_table_has_bench_charset() {
        for ch in "AB1.2-".chars() {
            assert!(FONT5X7.iter().any(|(name, _)| *name == ch.to_string()));
        }
        // 未知字符回退到空格字模
        assert_eq!(glyph('Z'), &[0u8; 5]);
    }
}
