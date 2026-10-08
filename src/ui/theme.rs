//! 界面主题与配色。
//!
//! 界面提供"跟随系统 / 浅色 / 深色"三种主题。除切换 egui 内置配色外,这里还统一
//! 了间距、强调色与状态色,保证四个分页的外观一致,也避免各分页各自硬编码颜色。

use egui::{Color32, Context, Stroke, Theme, ThemePreference, Vec2};

use crate::service::config::ThemeMode;

/// 注册系统中的简体中文字体,作为 egui 内置字体的字形回退。
///
/// egui 的默认字体不包含中文字符;没有这层回退时,中文会显示成方框。
pub fn install_system_fonts(ctx: &Context) {
    #[cfg(target_os = "windows")]
    {
        let windows_dir = std::env::var_os("WINDIR")
            .or_else(|| std::env::var_os("SystemRoot"))
            .map(std::path::PathBuf::from);

        let Some(windows_dir) = windows_dir else {
            log::warn!("未找到 Windows 系统目录,无法加载中文字体");
            return;
        };

        let font_dir = windows_dir.join("Fonts");
        let candidates = ["simhei.ttf", "simkai.ttf", "simfang.ttf", "Deng.ttf"];
        let mut font_bytes = None;

        for name in candidates {
            let path = font_dir.join(name);
            match std::fs::read(&path) {
                Ok(bytes) => {
                    font_bytes = Some(bytes);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => log::warn!("读取系统字体 {} 失败: {error}", path.display()),
            }
        }

        let Some(font_bytes) = font_bytes else {
            log::warn!("未找到可用的简体中文 TrueType 字体,中文可能显示为方框");
            return;
        };

        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "system_cjk".to_owned(),
            egui::FontData::from_owned(font_bytes).into(),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            if let Some(fonts) = fonts.families.get_mut(&family) {
                fonts.insert(0, "system_cjk".to_owned());
            }
        }
        ctx.set_fonts(fonts);
    }
}

/// 浅色模式下的主强调色,用于选中项、链接与进度条。
pub const ACCENT: Color32 = Color32::from_rgb(0x2B, 0x6C, 0xB0);

/// 深色模式下的主强调色。深色底需要更亮的颜色才能保证对比度。
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0x6C, 0xB0, 0xF0);

/// 成功状态色,用于"转换成功"等提示。
pub const SUCCESS: Color32 = Color32::from_rgb(0x2E, 0x8B, 0x57);

/// 失败状态色,用于错误提示。
pub const DANGER: Color32 = Color32::from_rgb(0xC0, 0x39, 0x2B);

/// 警告状态色,用于体积增大、参数被忽略等提醒。
pub const WARNING: Color32 = Color32::from_rgb(0xB0, 0x7A, 0x10);

/// 次要文字色,用于说明文字与单位。
pub const MUTED: Color32 = Color32::from_rgb(0x8A, 0x8A, 0x8A);

/// 应用主题偏好并统一界面间距。
///
/// 该函数是幂等的:重复调用只会写入同样的值,因此可以在每帧开始时调用。
pub fn apply(ctx: &Context, mode: ThemeMode) {
    ctx.set_theme(match mode {
        ThemeMode::System => ThemePreference::System,
        ThemeMode::Light => ThemePreference::Light,
        ThemeMode::Dark => ThemePreference::Dark,
    });
    tune(ctx);
}

/// 统一各控件的间距,使界面呈现紧凑的桌面软件风格。
pub fn tune(ctx: &Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        style.spacing.indent = 18.0;
        style.spacing.interact_size.y = 24.0;
        style.spacing.slider_width = 140.0;
        style.visuals.window_corner_radius = egui::CornerRadius::same(6);
    });
    accent(ctx);
}

/// 把当前生效主题的强调色替换为软件自己的配色。
pub fn accent(ctx: &Context) {
    let theme = ctx.theme();
    let color = match theme {
        Theme::Dark => ACCENT_DARK,
        Theme::Light => ACCENT,
    };

    let mut visuals = ctx.style_of(theme).visuals.clone();
    visuals.hyperlink_color = color;
    visuals.selection.bg_fill = color.gamma_multiply(0.55);
    visuals.selection.stroke = Stroke::new(1.0, color);
    visuals.widgets.active.bg_fill = color.gamma_multiply(0.65);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, color.gamma_multiply(0.7));
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, color);
    visuals.widgets.noninteractive.fg_stroke.color = match theme {
        Theme::Dark => Color32::from_rgb(0xD0, 0xD0, 0xD0),
        Theme::Light => Color32::from_rgb(0x30, 0x30, 0x30),
    };
    ctx.set_visuals_of(theme, visuals);
}

/// 当前主题下的强调色,供需要手动绘制的部件使用。
pub fn accent_color(ctx: &Context) -> Color32 {
    match ctx.theme() {
        Theme::Dark => ACCENT_DARK,
        Theme::Light => ACCENT,
    }
}

/// 成功/失败状态对应的颜色,便于状态列统一着色。
pub fn status_color(success: bool) -> Color32 {
    if success { SUCCESS } else { DANGER }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_mode_applies_without_panicking() {
        let ctx = Context::default();
        for mode in ThemeMode::all() {
            apply(&ctx, mode);
        }
    }

    #[test]
    fn accent_follows_theme() {
        let ctx = Context::default();
        apply(&ctx, ThemeMode::Light);
        assert_eq!(ctx.theme(), Theme::Light);
        assert_eq!(accent_color(&ctx), ACCENT);

        apply(&ctx, ThemeMode::Dark);
        assert_eq!(ctx.theme(), Theme::Dark);
        assert_eq!(accent_color(&ctx), ACCENT_DARK);
    }

    #[test]
    fn spacing_is_tightened() {
        let ctx = Context::default();
        tune(&ctx);
        let style = ctx.style_of(Theme::Light);
        assert_eq!(style.spacing.item_spacing, Vec2::new(8.0, 8.0));
        assert_eq!(style.spacing.indent, 18.0);
    }

    #[test]
    fn status_color_matches_outcome() {
        assert_eq!(status_color(true), SUCCESS);
        assert_eq!(status_color(false), DANGER);
    }
}
