//! Duokan（多看）对齐的 16 级灰阶量化。
//!
//! 逆向依据（2026-11-07 REA，报告《Duokan-xKindle-K4固件逆向与借鉴优化报告》）：
//! KindleApp 在两处转换路径上对 8bpp 像素执行同一恒等式
//! `v' = (v & 0xF0) | (v >> 4)`（取高 nibble 并双写，0x1174a8 / 0x493e20）。
//! 输出域收敛到 {17n | n∈0..15} = {0x00,0x11,…,0xFF}，且**幂等**：
//! 已量化像素再量化不动——partial 刷新的累积灰阶污染被锁死在 16 级内，
//! 这是 nibble 双写的设计动机，也是本模块可以整屏反复执行的原因。

/// 原地量化一段 8bpp 灰阶（Duokan 同款 floor 量化 + nibble 双写）。
pub fn quantize_slice(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        *b = (*b & 0xF0) | (*b >> 4);
    }
}

/// 量化可见 fb 区域：每行 stride 取 line_length（可能大于 xres），只动可见列，
/// 不触碰驱动私有区（与 bench 写帧同口径）。
///
/// # Safety
/// ptr 必须指向至少 `line_length * yres` 字节的可写映射（map_fb 的 smem_len 保证）。
pub unsafe fn quantize_visible_fb(
    ptr: *mut u8,
    line_length: u32,
    xres: u32,
    yres: u32,
) -> usize {
    let mut total = 0usize;
    for y in 0..yres as usize {
        let row = std::slice::from_raw_parts_mut(
            ptr.add(y * line_length as usize),
            xres as usize,
        );
        quantize_slice(row);
        total += row.len();
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors_match_duokan_identity() {
        // 与反汇编逐位对齐：(v & 0xF0) | (v >> 4)
        let cases: [(u8, u8); 8] = [
            (0x00, 0x00),
            (0xFF, 0xFF),
            (0x08, 0x00), // 低 nibble 舍弃（floor 量化）
            (0x18, 0x11),
            (0xF8, 0xFF),
            (0x7F, 0x77),
            (0x80, 0x88),
            (0x10, 0x11), // 高 nibble 双写回填
        ];
        for (input, want) in cases {
            let mut b = [input];
            quantize_slice(&mut b);
            assert_eq!(b[0], want, "v={input:#04x}");
        }
    }

    #[test]
    fn output_domain_is_17n() {
        let mut buf: Vec<u8> = (0..=255u8).collect();
        quantize_slice(&mut buf);
        for &v in buf.iter() {
            assert_eq!(v % 17, 0, "量化输出应落在 17n（nibble 双写）域，got {v:#04x}");
        }
    }

    #[test]
    fn idempotent_on_all_bytes() {
        let mut buf: Vec<u8> = (0..=255u8).collect();
        quantize_slice(&mut buf);
        let once = buf.clone();
        quantize_slice(&mut buf);
        assert_eq!(buf, once, "再量化必须不动（幂等）");
    }

    #[test]
    fn visible_fb_respects_stride() {
        // 3 行 × line_length=8（可见 4 列）：行间 pad 与行尾 pad 不被触碰。
        let line_length = 8u32;
        let (xres, yres) = (4u32, 3u32);
        let mut fb = vec![0u8; (line_length * yres) as usize];
        // 预填：可见区放 0xA5（期望 → 0xAA），pad 放哨兵 0x5C（期望原样）
        for y in 0..yres as usize {
            for x in 0..xres as usize {
                fb[y * line_length as usize + x] = 0xA5;
            }
            for x in xres as usize..line_length as usize {
                fb[y * line_length as usize + x] = 0x5C;
            }
        }
        let n = unsafe { quantize_visible_fb(fb.as_mut_ptr(), line_length, xres, yres) };
        assert_eq!(n, (xres * yres) as usize);
        for y in 0..yres as usize {
            for x in 0..xres as usize {
                assert_eq!(fb[y * line_length as usize + x], 0xAA);
            }
            for x in xres as usize..line_length as usize {
                assert_eq!(fb[y * line_length as usize + x], 0x5C, "pad 被改写");
            }
        }
    }
}
