//! 通用界面部件与参数编辑器。
//!


use std::path::PathBuf;

use egui::{
    Align, Button, Color32, ComboBox, CornerRadius, DragValue, Frame, Label, Margin, ProgressBar,
    Response, RichText, Sense, Slider, Stroke, TextEdit, Ui, Vec2,
};

use crate::core::format::Format;
use crate::core::options::{EncodeOptions, NamingRule, ResizeMode, Rotation, TransformOptions};
use crate::core::color::ColorType;
use crate::service::config::{AppConfig, MAX_DIMENSION};

use super::theme;

/// 表单左侧标签的固定宽度,使多行控件左边缘对齐。
const LABEL_WIDTH: f32 = 92.0;

/// 带标题的分组卡片。
pub fn section<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(4.0);
    ui.label(RichText::new(title).strong().size(15.0));
    let inner = Frame::group(ui.style())
        .inner_margin(Margin::symmetric(10, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(4.0);
    inner
}

/// 可折叠的分组卡片。
///
/// 批量转换分页的参数组较多,全部展开会把运行日志挤到可视区之外。这里把不常改动的
/// 参数组收起,首屏就能同时看到文件列表、任务控制和运行日志。
/// 收起时闭包不会执行,返回 `None`;展开时返回闭包结果。
pub fn section_collapsible<R>(
    ui: &mut Ui,
    title: &str,
    default_open: bool,
    add: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    ui.add_space(4.0);
    let mut out = None;
    egui::CollapsingHeader::new(RichText::new(title).strong().size(15.0))
        .default_open(default_open)
        .show(ui, |ui| {
            out = Some(
                Frame::group(ui.style())
                    .inner_margin(Margin::symmetric(10, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        add(ui)
                    })
                    .inner,
            );
        });
    ui.add_space(4.0);
    out
}

/// "标签 + 控件"的一行表单。
pub fn field(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    let height = ui.spacing().interact_size.y;
    ui.horizontal(|ui| {
        ui.add_sized(Vec2::new(LABEL_WIDTH, height), Label::new(label));
        add(ui);
    });
}

/// 只读键值对,用于结果面板与"关于"信息。
pub fn key_value(ui: &mut Ui, key: &str, value: &str) {
    let height = ui.spacing().interact_size.y;
    ui.horizontal(|ui| {
        ui.add_sized(
            Vec2::new(LABEL_WIDTH, height),
            Label::new(RichText::new(key).color(theme::MUTED)),
        );
        ui.label(value);
    });
}

/// 次要说明文字。
pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(theme::MUTED));
}

/// 彩色状态小标签。
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    Frame::NONE
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.55)))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).small().strong());
        });
}

/// 压缩率彩色胶囊徽章。
pub fn compression_badge(ui: &mut Ui, percent: f64) {
    let (text, color) = if percent <= -20.0 {
        (format!("{percent:+.1}%"), theme::SUCCESS)
    } else if percent < 0.0 {
        (format!("{percent:+.1}%"), theme::ACCENT)
    } else if percent == 0.0 {
        ("0.0%".to_string(), theme::MUTED)
    } else {
        (format!("{percent:+.1}%"), theme::WARNING)
    };
    badge(ui, &text, color);
}

/// 泛型枚举下拉框,返回选项是否发生变化。
pub fn enum_combo<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: &str,
    value: &mut T,
    options: &[T],
    name: impl Fn(T) -> &'static str,
) -> bool {
    let mut changed = false;
    ComboBox::from_id_salt(id)
        .selected_text(name(*value))
        .width(190.0)
        .show_ui(ui, |ui| {
            for option in options {
                if ui.selectable_value(value, *option, name(*option)).changed() {
                    changed = true;
                }
            }
        });
    changed
}

/// 目标格式下拉框。候选来自注册表,界面中不硬编码格式名称。
pub fn format_combo(ui: &mut Ui, id: &str, value: &mut Format, formats: &[Format]) -> bool {
    let mut changed = false;
    ComboBox::from_id_salt(id)
        .selected_text(value.name())
        .width(220.0)
        .show_ui(ui, |ui| {
            for format in formats {
                let label = format!("{} (.{})", format.name(), format.primary_extension());
                if ui.selectable_value(value, *format, label).changed() {
                    changed = true;
                }
            }
        });
    changed
}

/// 输出目录编辑框。留空表示"输出到源文件所在目录"。
pub fn output_dir_editor(ui: &mut Ui, dir: &mut Option<PathBuf>) -> bool {
    let mut changed = false;
    let mut text = dir
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();

    ui.horizontal(|ui| {
        let width = (ui.available_width() - 84.0).max(80.0);
        let edited = ui
            .add(
                TextEdit::singleline(&mut text)
                    .desired_width(width)
                    .hint_text("留空则输出到源文件所在目录"),
            )
            .changed();
        if edited {
            let trimmed = text.trim();
            *dir = if trimmed.is_empty() {
                None
            } else {
                Some(PathBuf::from(trimmed))
            };
            changed = true;
        }

        if ui.button("浏览...").clicked() {
            let mut dialog = rfd::FileDialog::new().set_title("选择输出目录");
            if let Some(start) = dir.as_ref() {
                dialog = dialog.set_directory(start);
            }
            if let Some(picked) = dialog.pick_folder() {
                *dir = Some(picked);
                changed = true;
            }
        }
    });

    changed
}

/// 输出参数编辑器:目标格式、输出目录、命名规则、覆盖开关。
pub fn output_editor(ui: &mut Ui, config: &mut AppConfig, formats: &[Format]) -> bool {
    let mut changed = false;

    field(ui, "目标格式", |ui| {
        changed |= format_combo(ui, "target_format", &mut config.target_format, formats);
    });
    hint(ui, config.target_format.description());

    field(ui, "输出目录", |ui| {
        changed |= output_dir_editor(ui, &mut config.output_dir);
    });

    field(ui, "命名规则", |ui| {
        changed |= enum_combo(
            ui,
            "naming_rule",
            &mut config.naming,
            &NamingRule::all(),
            |rule| rule.name(),
        );
    });

    field(ui, "同名文件", |ui| {
        changed |= ui
            .checkbox(&mut config.overwrite, "允许覆盖")
            .on_hover_text("不勾选时遇到同名文件会自动追加序号,不会覆盖已有文件")
            .changed();
    });

    changed
}

/// 图像变换参数编辑器。
///
/// `default_size` 是启用缩放时的初始尺寸,通常传入源图尺寸;源图未知时传入
/// 一个常规大小的兜底值,避免出现 `1×1` 这类无意义的默认值。
pub fn transform_editor(
    ui: &mut Ui,
    options: &mut TransformOptions,
    default_size: (u32, u32),
) -> bool {
    let mut changed = false;

    field(ui, "缩放尺寸", |ui| {
        let mut enabled = options.resize.is_some();
        changed |= ui.checkbox(&mut enabled, "启用").changed();

        let (mut width, mut height) = options.resize.unwrap_or(default_size);
        changed |= ui
            .add_enabled(
                enabled,
                DragValue::new(&mut width)
                    .range(1..=MAX_DIMENSION)
                    .speed(1.0)
                    .prefix("宽 "),
            )
            .changed();
        changed |= ui
            .add_enabled(
                enabled,
                DragValue::new(&mut height)
                    .range(1..=MAX_DIMENSION)
                    .speed(1.0)
                    .prefix("高 "),
            )
            .changed();
        ui.label(RichText::new("像素").small().color(theme::MUTED));

        options.resize = if enabled {
            Some((width.max(1), height.max(1)))
        } else {
            None
        };
    });

    field(ui, "缩放算法", |ui| {
        changed |= ui
            .add_enabled_ui(options.resize.is_some(), |ui| {
                enum_combo(
                    ui,
                    "resize_mode",
                    &mut options.resize_mode,
                    &ResizeMode::all(),
                    |mode| mode.name(),
                )
            })
            .inner;
    });

    field(ui, "旋转", |ui| {
        changed |= enum_combo(
            ui,
            "rotation",
            &mut options.rotate,
            &Rotation::all(),
            |angle| angle.name(),
        );
    });

    field(ui, "镜像", |ui| {
        changed |= ui
            .checkbox(&mut options.flip_horizontal, "水平翻转")
            .changed();
        changed |= ui
            .checkbox(&mut options.flip_vertical, "垂直翻转")
            .changed();
    });

    field(ui, "颜色模式", |ui| {
        let mut grayscale = options.grayscale;
        changed |= ui.checkbox(&mut grayscale, "转为灰度").changed();
        options.grayscale = grayscale;
        if grayscale {
            // 灰度与"目标颜色"互斥,避免出现自相矛盾的组合
            options.color = None;
        }
    });

    field(ui, "目标颜色", |ui| {
        changed |= ui
            .add_enabled_ui(!options.grayscale, |ui| {
                let mut preserve = options.color.is_none();
                let mut color = options.color.unwrap_or(ColorType::Rgba8);
                let mut inner_changed = ui.checkbox(&mut preserve, "保持原样").changed();
                inner_changed |= ui
                    .add_enabled_ui(!preserve, |ui| {
                        enum_combo(ui, "target_color", &mut color, &ColorType::all(), |item| {
                            item.name()
                        })
                    })
                    .inner;
                options.color = if preserve { None } else { Some(color) };
                inner_changed
            })
            .inner;
    });

    hint(
        ui,
        "提示:不启用缩放时沿用源图尺寸;旋转 90° / 270° 会自动交换宽高。",
    );

    changed
}

/// 根据目标格式显示有损质量或 PNG 压缩等级。
pub fn encode_editor(ui: &mut Ui, options: &mut EncodeOptions, format: Format) -> bool {
    let mut changed = false;
    if format == Format::Png {
        field(ui, "压缩等级", |ui| {
            changed |= ui
                .add(
                    Slider::new(&mut options.png_compression_level, 0..=9)
                        .integer()
                        .suffix(" / 9"),
                )
                .on_hover_text("等级越低转换越快,文件通常越大")
                .changed();
        });
        field(ui, "极速模式", |ui| {
            changed |= ui
                .checkbox(&mut options.png_fast_mode, "启用快速行滤波器")
                .on_hover_text("启用轻量快速滤波器,处理速度提升数倍且体积基本一致")
                .changed();
        });
        hint(
            ui,
            "调低等级或开启极速模式可显著加快转换;调高等级可进一步缩小文件。",
        );
    } else if format == Format::Gif {
        hint(
            ui,
            "GIF 格式使用 256 色调色板与 LZW 压缩算法,无需额外编码参数。",
        );
    } else if format == Format::Webp {
        hint(
            ui,
            "WebP 格式使用无损 VP8L 压缩算法,兼顾高压缩率与完全保真。",
        );
    } else if format == Format::Svg {
        hint(
            ui,
            "SVG 格式将输出封装为矢量容器 (<image> 标签),支持任意比例无损缩放。",
        );
    } else {
        field(ui, "编码质量", |ui| {
            let mut quality = options.quality as f64;
            if ui
                .add_enabled(
                    !format.is_lossless(),
                    Slider::new(&mut quality, 1.0..=100.0)
                        .integer()
                        .suffix(" / 100"),
                )
                .changed()
            {
                options.quality = quality.round() as u8;
                changed = true;
            }
        });
        if format.is_lossless() {
            hint(ui, "当前目标格式为无损格式,质量参数会被忽略。");
        }
    }
    changed
}

/// 进度条 + 说明文字。
pub fn progress_row(ui: &mut Ui, progress: f32, text: &str) {
    let width = ui.available_width();
    ui.add(
        ProgressBar::new(progress.clamp(0.0, 1.0))
            .desired_width(width)
            .text(text.to_owned()),
    );
}

/// 文件拖放区域。返回是否被点击(用于打开文件选择框)。
pub fn drop_zone(ui: &mut Ui, count: usize, enabled: bool) -> bool {
    let accent = theme::accent_color(ui.ctx());
    let response = Frame::group(ui.style())
        .inner_margin(Margin::symmetric(12, 20))
        .stroke(Stroke::new(1.0, accent.gamma_multiply(0.5)))
        .show(ui, |ui| {
            let width = ui.available_width();
            ui.set_width(width);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("把图片文件拖放到这里").size(15.0).strong());
                ui.add_space(4.0);
                if count == 0 {
                    ui.label(RichText::new("或点击本区域选择文件").color(theme::MUTED));
                } else {
                    ui.label(
                        RichText::new(format!("已选择 {count} 个文件,再次点击可继续追加"))
                            .color(theme::MUTED),
                    );
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new("可读格式:BMP / PNG / Netpbm / TGA / QOI / farbfeld / ICO")
                        .small()
                        .color(theme::MUTED),
                );
            });
        })
        .response;

    if !enabled {
        return false;
    }
    response.interact(Sense::click()).clicked()
}

/// 固定宽度的小按钮,用于表格行内操作。
pub fn small_action(ui: &mut Ui, text: &str) -> Response {
    ui.add_sized(
        Vec2::new(54.0, 22.0),
        Button::new(RichText::new(text).small()),
    )
}

/// 右对齐的次要文字。
pub fn right_aligned_muted(ui: &mut Ui, text: &str) {
    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
        ui.label(RichText::new(text).small().color(theme::MUTED));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::run_frame;

    #[test]
    fn section_and_field_render_without_panicking() {
        let ctx = egui::Context::default();
        run_frame(&ctx, |ui| {
            section(ui, "基本设置", |ui| {
                field(ui, "目标格式", |ui| {
                    ui.label("BMP 位图");
                });
                key_value(ui, "来源格式", "QOI 图像");
                hint(ui, "这是一条说明");
                badge(ui, "成功", theme::SUCCESS);
            });
        });
    }

    #[test]
    fn collapsible_section_returns_none_while_collapsed() {
        // 折叠状态记在 egui 的上下文里,默认展开要和默认收起分开建上下文测,
        // 否则第二次渲染会沿用第一次存下的收起状态。
        let collapsed_ctx = egui::Context::default();
        let collapsed = run_frame(&collapsed_ctx, |ui| {
            section_collapsible(ui, "输出参数", false, |ui| {
                ui.label("内容");
                7u32
            })
        });
        assert_eq!(collapsed, None, "收起状态下不应执行内容闭包");

        let opened_ctx = egui::Context::default();
        let opened = run_frame(&opened_ctx, |ui| {
            section_collapsible(ui, "输出参数", true, |ui| {
                ui.label("内容");
                7u32
            })
        });
        assert_eq!(opened, Some(7), "展开状态下应返回闭包结果");
    }

    #[test]
    fn output_editor_keeps_selection_on_first_frame() {
        let ctx = egui::Context::default();
        let mut config = AppConfig::default();
        let formats = [
            Format::Bmp,
            Format::Netpbm,
            Format::Tga,
            Format::Qoi,
            Format::Farbfeld,
            Format::Ico,
        ];
        let changed = run_frame(&ctx, |ui| output_editor(ui, &mut config, &formats));
        assert!(!changed, "首帧只是绘制控件,不应报告参数变化");
        assert!(formats.contains(&config.target_format));
    }

    #[test]
    fn transform_editor_keeps_resize_disabled_by_default() {
        let ctx = egui::Context::default();
        let mut options = TransformOptions::default();
        run_frame(&ctx, |ui| transform_editor(ui, &mut options, (800, 600)));
        assert!(options.resize.is_none());
        assert!(options.is_identity());
    }

    #[test]
    fn transform_editor_uses_default_size_when_enabled() {
        let ctx = egui::Context::default();
        let mut options = TransformOptions {
            resize: Some((800, 600)),
            ..Default::default()
        };
        run_frame(&ctx, |ui| transform_editor(ui, &mut options, (640, 480)));
        assert_eq!(options.resize, Some((800, 600)));
    }

    #[test]
    fn transform_editor_clears_color_when_grayscale_is_on() {
        let ctx = egui::Context::default();
        let mut options = TransformOptions {
            grayscale: true,
            color: Some(ColorType::Rgb8),
            ..Default::default()
        };
        run_frame(&ctx, |ui| transform_editor(ui, &mut options, (100, 100)));
        assert!(options.color.is_none());
    }

    #[test]
    fn progress_row_clamps_out_of_range_values() {
        let ctx = egui::Context::default();
        run_frame(&ctx, |ui| {
            progress_row(ui, -5.0, "开始");
            progress_row(ui, 12.0, "结束");
        });
    }

    #[test]
    fn drop_zone_reports_no_click_without_input() {
        let ctx = egui::Context::default();
        let clicked = run_frame(&ctx, |ui| drop_zone(ui, 0, true));
        assert!(!clicked);
    }

    #[test]
    fn encode_editor_does_not_mutate_lossless_quality() {
        let ctx = egui::Context::default();
        let mut options = EncodeOptions {
            quality: 88,
            ..Default::default()
        };
        let changed = run_frame(&ctx, |ui| encode_editor(ui, &mut options, Format::Bmp));
        assert!(!changed);
        assert_eq!(options.quality, 88);
    }

    #[test]
    fn encode_editor_renders_png_compression_control() {
        let ctx = egui::Context::default();
        let mut options = EncodeOptions::default();
        let changed = run_frame(&ctx, |ui| encode_editor(ui, &mut options, Format::Png));
        assert!(!changed);
        assert_eq!(options.png_compression_level, 1);
    }

    #[test]
    fn encode_editor_renders_gif_hint() {
        let ctx = egui::Context::default();
        let mut options = EncodeOptions::default();
        let changed = run_frame(&ctx, |ui| encode_editor(ui, &mut options, Format::Gif));
        assert!(!changed);
    }

    #[test]
    fn encode_editor_renders_webp_and_svg_hint() {
        let ctx = egui::Context::default();
        let mut options = EncodeOptions::default();
        let changed_webp = run_frame(&ctx, |ui| encode_editor(ui, &mut options, Format::Webp));
        assert!(!changed_webp);
        let changed_svg = run_frame(&ctx, |ui| encode_editor(ui, &mut options, Format::Svg));
        assert!(!changed_svg);
    }

    #[test]
    fn small_action_and_right_aligned_text_render() {
        let ctx = egui::Context::default();
        run_frame(&ctx, |ui| {
            small_action(ui, "定位");
            right_aligned_muted(ui, "共 3 项");
        });
    }
}
