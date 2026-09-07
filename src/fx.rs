//! fx 类型常量与刷新策略（纯逻辑，无系统调用，可在任意平台单测）。
//!
//! Kindle 4 的 legacy einkfb 驱动没有 waveform 概念，`fx_type` 就是它唯一
//! 的"刷新模式选择器"。取值与语义来自 FBInk `eink/einkfb.h` 的枚举注释，
//! 并与 KOReader `ffi/einkfb_h.lua` 的 cdef 逐项交叉核对（见方案文档 §2）。

/// 高速低保真，不闪烁。KOReader 原生 partial 使用的就是它。
pub const FX_PARTIAL: i32 = 0;
/// 高保真低速；经 UPDATE_DISPLAY_AREA 实测并不闪烁（FBInk refresh_legacy 注释）。
pub const FX_FULL: i32 = 1;
/// 牺牲全部保真换取速度，不闪烁。KOReader 从未使用，本项目的核心增益点。
pub const FX_FAST: i32 = 2;
/// 牺牲速度换保真，闪烁。驱动对它的"内容未变则跳过"检查豁免（koreader-base#2481）。
pub const FX_SLOW: i32 = 3;

/// 刷新请求类别（策略层输入，对应 C ABI 的 kind 参数）。
pub const KIND_PAGE_TURN: u32 = 0;
pub const KIND_UI: u32 = 1;
pub const KIND_FULL: u32 = 2;

/// 运行模式：
/// - fast：翻页走 FX_FAST，每 page_interval 次翻页用 FX_SLOW 收尾清残影；
/// - conservative：翻页保持原生 FX_PARTIAL，全刷固定 FX_SLOW（等价 base#2481 行为）。
pub const MODE_FAST: u32 = 0;
pub const MODE_CONSERVATIVE: u32 = 1;

/// 刷新策略（lib 内用原子变量实现同一套规则；此结构供单测与 CLI 使用）。
#[derive(Debug, Clone)]
pub struct Policy {
    pub mode: u32,
    pub page_interval: u32,
    /// 当前累计的翻页次数（公开字段：lib 状态层需要跨调用搬运它）。
    pub flips_since_slow: u32,
}

impl Policy {
    pub fn new(mode: u32, page_interval: u32) -> Self {
        Policy {
            mode,
            page_interval: page_interval.max(1),
            flips_since_slow: 0,
        }
    }

    /// 决定本次刷新应使用的 fx 类型。
    pub fn decide(&mut self, kind: u32) -> i32 {
        match kind {
            // 显式全刷：固定 slow（残影清零 + no-op 豁免），并复位计数。
            KIND_FULL => {
                self.flips_since_slow = 0;
                FX_SLOW
            }
            // UI 类（菜单/高亮）保持原生 partial 语义。
            KIND_UI => FX_PARTIAL,
            _ => match self.mode {
                MODE_CONSERVATIVE => FX_PARTIAL,
                _ => {
                    self.flips_since_slow += 1;
                    if self.flips_since_slow >= self.page_interval {
                        self.flips_since_slow = 0;
                        FX_SLOW
                    } else {
                        FX_FAST
                    }
                }
            },
        }
    }
}

/// 把请求矩形裁剪到屏幕范围内；退化矩形（宽或高 <= 1）返回 None。
///
/// 丢弃 <=1px 区域沿用 KOReader 对无效刷新区域的防御性处理
/// （framebuffer_mxcfb.lua：1px 区域曾在部分内核上引发故障，按同类风险防御）。
pub fn clamp_rect(
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    max_w: i32,
    max_h: i32,
) -> Option<(i32, i32, i32, i32)> {
    if max_w <= 0 || max_h <= 0 {
        return None;
    }
    let x1 = x1.clamp(0, max_w);
    let x2 = x2.clamp(0, max_w);
    let y1 = y1.clamp(0, max_h);
    let y2 = y2.clamp(0, max_h);
    if x2 - x1 <= 1 || y2 - y1 <= 1 {
        return None;
    }
    Some((x1, y1, x2, y2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_then_slow_at_interval() {
        let mut p = Policy::new(MODE_FAST, 3);
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_FAST);
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_FAST);
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_SLOW); // 第 3 次到达间隔
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_FAST); // 计数已复位
    }

    #[test]
    fn full_resets_counter_and_uses_slow() {
        let mut p = Policy::new(MODE_FAST, 10);
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_FAST);
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_FAST);
        assert_eq!(p.decide(KIND_FULL), FX_SLOW);
        // 全刷后计数复位，还要再等满一个间隔才会 slow
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_FAST);
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_FAST);
    }

    #[test]
    fn ui_is_always_partial() {
        let mut p = Policy::new(MODE_FAST, 1);
        assert_eq!(p.decide(KIND_UI), FX_PARTIAL);
    }

    #[test]
    fn conservative_mode_keeps_partial_pages() {
        let mut p = Policy::new(MODE_CONSERVATIVE, 5);
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_PARTIAL);
        assert_eq!(p.decide(KIND_FULL), FX_SLOW);
    }

    #[test]
    fn interval_one_flashes_every_page() {
        let mut p = Policy::new(MODE_FAST, 1);
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_SLOW);
        assert_eq!(p.decide(KIND_PAGE_TURN), FX_SLOW);
    }

    #[test]
    fn clamp_rect_cases() {
        // 600x800 屏：正常区域原样通过
        assert_eq!(clamp_rect(0, 0, 600, 800, 600, 800), Some((0, 0, 600, 800)));
        // 越界裁剪
        assert_eq!(clamp_rect(-5, -5, 700, 900, 600, 800), Some((0, 0, 600, 800)));
        // 完全出屏 → 空
        assert_eq!(clamp_rect(600, 800, 700, 900, 600, 800), None);
        // 1px 退化 → 空
        assert_eq!(clamp_rect(10, 10, 11, 11, 600, 800), None);
        // 坐标反转（x2<x1）裁剪后为空 → None
        assert_eq!(clamp_rect(50, 50, 10, 10, 600, 800), None);
    }
}
