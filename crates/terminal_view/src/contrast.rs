//! APCA（Accessible Perceptual Contrast Algorithm）感知对比度算法。
//!
//! 从 Zed 的 `crates/ui/src/utils/apca_contrast.rs` 移植而来，仅依赖 gpui 的 [`Hsla`]，
//! 不依赖 Zed 的 `ui` / `theme` 等 crate。
//!
//! 算法基于 APCA 0.0.98G-4g（W3 兼容常量，https://github.com/Myndex/apca-w3）。
//! 相比 WCAG 2.x，APCA 在深色背景下更符合人眼感知，且具有极性（方向）感知性。

use gpui::Hsla;

/// APCA 常量（G-4g 版本）。
struct APCAConstants {
    // 主 TRC 指数（显示器感知）
    main_trc: f32,
    // sRGB 系数
    s_rco: f32,
    s_gco: f32,
    s_bco: f32,
    // 使用 2.4 指数的 G-4g 常量
    norm_bg: f32,
    norm_txt: f32,
    rev_txt: f32,
    rev_bg: f32,
    // 钳制与缩放
    blk_thrs: f32,
    blk_clmp: f32,
    scale_bow: f32,
    scale_wob: f32,
    lo_bow_offset: f32,
    lo_wob_offset: f32,
    delta_y_min: f32,
    lo_clip: f32,
}

impl Default for APCAConstants {
    fn default() -> Self {
        Self {
            main_trc: 2.4,
            s_rco: 0.2126729,
            s_gco: 0.7151522,
            s_bco: 0.0721750,
            norm_bg: 0.56,
            norm_txt: 0.57,
            rev_txt: 0.62,
            rev_bg: 0.65,
            blk_thrs: 0.022,
            blk_clmp: 1.414,
            scale_bow: 1.14,
            scale_wob: 1.14,
            lo_bow_offset: 0.027,
            lo_wob_offset: 0.027,
            delta_y_min: 0.0005,
            lo_clip: 0.1,
        }
    }
}

/// 计算两个颜色的 APCA 对比度。
///
/// 返回值约为 -108..106：
/// - 正值表示深色文字在浅色背景上（正常极性）
/// - 负值表示浅色文字在深色背景上（反极性）
/// - 0 表示对比度过低或相同颜色
pub fn apca_contrast(text_color: Hsla, background_color: Hsla) -> f32 {
    let constants = APCAConstants::default();

    let text_y = srgb_to_y(text_color, &constants);
    let bg_y = srgb_to_y(background_color, &constants);

    // 对接近黑色的颜色应用软钳制
    let text_y_clamped = if text_y > constants.blk_thrs {
        text_y
    } else {
        text_y + (constants.blk_thrs - text_y).powf(constants.blk_clmp)
    };

    let bg_y_clamped = if bg_y > constants.blk_thrs {
        bg_y
    } else {
        bg_y + (constants.blk_thrs - bg_y).powf(constants.blk_clmp)
    };

    // Y 差值极小时返回 0
    if (bg_y_clamped - text_y_clamped).abs() < constants.delta_y_min {
        return 0.0;
    }

    let sapc;
    let output_contrast;
    if bg_y_clamped > text_y_clamped {
        // 正常极性：深色文字在浅色背景上
        sapc = (bg_y_clamped.powf(constants.norm_bg) - text_y_clamped.powf(constants.norm_txt))
            * constants.scale_bow;

        // 低对比度平滑滚降，防止极性反转
        output_contrast = if sapc < constants.lo_clip {
            0.0
        } else {
            sapc - constants.lo_bow_offset
        };
    } else {
        // 反极性：浅色文字在深色背景上
        sapc = (bg_y_clamped.powf(constants.rev_bg) - text_y_clamped.powf(constants.rev_txt))
            * constants.scale_wob;

        output_contrast = if sapc > -constants.lo_clip {
            0.0
        } else {
            sapc + constants.lo_wob_offset
        };
    }

    // 缩放为百分比形式的 Lc 值
    output_contrast * 100.0
}

/// 将 sRGB 颜色转换为 APCA 计算用的 Y（亮度）值。
fn srgb_to_y(color: Hsla, constants: &APCAConstants) -> f32 {
    let rgba = color.to_rgb();

    // sRGB 编码值直接做 gamma 空间运算（APCA 算法的特点）
    let r_linear = rgba.r.powf(constants.main_trc);
    let g_linear = rgba.g.powf(constants.main_trc);
    let b_linear = rgba.b.powf(constants.main_trc);

    constants.s_rco * r_linear + constants.s_gco * g_linear + constants.s_bco * b_linear
}

/// 调整前景色以满足最小 APCA 对比度。
///
/// `minimum_apca_contrast` 应为绝对值（如 45 表示 Lc 45）。
/// 调整策略（尽量保留颜色）：
/// 1. 保持色相/饱和度，二分搜索调整明度
/// 2. 明度不足时逐步降低饱和度再调明度
/// 3. 最后退化为纯黑或纯白
pub fn ensure_minimum_contrast(
    foreground: Hsla,
    background: Hsla,
    minimum_apca_contrast: f32,
) -> Hsla {
    if minimum_apca_contrast <= 0.0 {
        return foreground;
    }

    let current_contrast = apca_contrast(foreground, background).abs();

    if current_contrast >= minimum_apca_contrast {
        return foreground;
    }

    // 第一步：保持色相/饱和度，只调明度
    let adjusted = adjust_lightness_for_contrast(foreground, background, minimum_apca_contrast);

    let adjusted_contrast = apca_contrast(adjusted, background).abs();
    if adjusted_contrast >= minimum_apca_contrast {
        return adjusted;
    }

    // 第二步：逐步降低饱和度同时调整明度
    let desaturated =
        adjust_lightness_and_saturation_for_contrast(foreground, background, minimum_apca_contrast);

    let desaturated_contrast = apca_contrast(desaturated, background).abs();
    if desaturated_contrast >= minimum_apca_contrast {
        return desaturated;
    }

    // 最后手段：使用纯黑或纯白
    let black = Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.0,
        a: foreground.a,
    };

    let white = Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: foreground.a,
    };

    let black_contrast = apca_contrast(black, background).abs();
    let white_contrast = apca_contrast(white, background).abs();

    if white_contrast > black_contrast {
        white
    } else {
        black
    }
}

/// 仅调整明度以满足最小对比度（保持色相与饱和度）。
fn adjust_lightness_for_contrast(
    foreground: Hsla,
    background: Hsla,
    minimum_apca_contrast: f32,
) -> Hsla {
    // 判断需要变亮还是变暗
    let bg_luminance = srgb_to_y(background, &APCAConstants::default());
    let should_go_darker = bg_luminance > 0.5;

    // 二分搜索最优明度
    let mut low = if should_go_darker { 0.0 } else { foreground.l };
    let mut high = if should_go_darker { foreground.l } else { 1.0 };
    let mut best_l = foreground.l;

    for _ in 0..20 {
        let mid = (low + high) / 2.0;
        let test_color = Hsla {
            h: foreground.h,
            s: foreground.s,
            l: mid,
            a: foreground.a,
        };
        let contrast = apca_contrast(test_color, background).abs();

        if contrast >= minimum_apca_contrast {
            best_l = mid;
            // 向最小值方向逼近
            if should_go_darker {
                low = mid;
            } else {
                high = mid;
            }
        } else if should_go_darker {
            high = mid;
        } else {
            low = mid;
        }

        // 接近目标即停止
        if (contrast - minimum_apca_contrast).abs() < 1.0 {
            best_l = mid;
            break;
        }
    }

    Hsla {
        h: foreground.h,
        s: foreground.s,
        l: best_l,
        a: foreground.a,
    }
}

/// 同时调整明度与饱和度以满足最小对比度。
fn adjust_lightness_and_saturation_for_contrast(
    foreground: Hsla,
    background: Hsla,
    minimum_apca_contrast: f32,
) -> Hsla {
    // 依次尝试不同饱和度
    let saturation_steps = [1.0, 0.8, 0.6, 0.4, 0.2, 0.0];

    for &sat_multiplier in &saturation_steps {
        let test_color = Hsla {
            h: foreground.h,
            s: foreground.s * sat_multiplier,
            l: foreground.l,
            a: foreground.a,
        };

        let adjusted =
            adjust_lightness_for_contrast(test_color, background, minimum_apca_contrast);
        let contrast = apca_contrast(adjusted, background).abs();

        if contrast >= minimum_apca_contrast {
            return adjusted;
        }
    }

    // 灰度也无法满足时，返回灰度尝试
    Hsla {
        h: foreground.h,
        s: 0.0,
        l: foreground.l,
        a: foreground.a,
    }
}
