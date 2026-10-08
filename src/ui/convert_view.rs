//! 单文件转换分页。
//!
//! 页面上半部分是输入与参数,下半部分是结果或错误提示。参数直接读写全局配置,因此
//! 用户在上次的选择会被保留;只有"开始转换"会真正提交后台任务。

use std::path::{Path, PathBuf};

use egui::{RichText, Ui};

use crate::core::image::Image;
use crate::convert::pipeline::decode_input;
use crate::convert::ConversionOutcome;
use crate::util::fs;

use super::theme;
use super::widgets;
use super::{TaskKind, UiContext, UiRequest};

/// 预览图最长边的像素数。超过该值先等比缩小再上传纹理,避免占用过多显存。
const PREVIEW_MAX_SIDE: u32 = 420;

/// 单文件转换分页。
pub struct ConvertView {
    /// 待转换的输入文件。允许一次拖入多个,转换时按顺序逐个处理。
    pub inputs: Vec<PathBuf>,
    /// 最近一次成功的结果。
    pub outcome: Option<ConversionOutcome>,
    /// 最近一次失败的原因。
    pub error: Option<String>,
    /// 是否正在转换。由主程序根据任务状态维护。
    pub busy: bool,
    /// 已选源图的尺寸,作为缩放控件的默认值。
    pub source_size: (u32, u32),
    /// 源图预览纹理。
    preview: Option<egui::TextureHandle>,
    /// 预览对应的文件路径,用于判断是否需要重新生成纹理。
    preview_key: Option<String>,
}

impl Default for ConvertView {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            outcome: None,
            error: None,
            busy: false,
            source_size: (800, 600),
            preview: None,
            preview_key: None,
        }
    }
}

impl ConvertView {
    /// 新建一个空的转换分页。
    pub fn new() -> Self {
        Self::default()
    }

    /// 替换待转换文件列表,并清空上一轮结果。
    pub fn set_inputs(&mut self, files: Vec<PathBuf>) {
        self.inputs = files;
        self.outcome = None;
        self.error = None;
    }

    /// 追加文件,自动跳过重复项。
    pub fn add_inputs(&mut self, files: impl IntoIterator<Item = PathBuf>) {
        for file in files {
            if !self.inputs.contains(&file) {
                self.inputs.push(file);
            }
        }
    }

    /// 清空输入与结果。
    pub fn clear(&mut self) {
        self.inputs.clear();
        self.outcome = None;
        self.error = None;
    }

    /// 记录一次成功的结果。
    pub fn report_success(&mut self, outcome: ConversionOutcome) {
        self.outcome = Some(outcome);
        self.error = None;
        self.busy = false;
    }

    /// 记录一次失败。
    pub fn report_failure(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
        self.busy = false;
    }

    /// 绘制整个分页,返回需要主程序执行的请求。
    pub fn render(&mut self, ui: &mut Ui, ctx: &mut UiContext<'_>) -> Vec<UiRequest> {
        let mut requests: Vec<UiRequest> = Vec::new();
        let busy = ctx.busy;
        let formats = ctx.formats.to_vec();

        let mut picked: Vec<PathBuf> = Vec::new();
        let mut clear_clicked = false;
        let mut remove_index: Option<usize> = None;
        let mut start_clicked = false;
        let mut reset_clicked = false;
        let mut reveal_target: Option<PathBuf> = None;
        let mut open_dir_target: Option<PathBuf> = None;

        self.ensure_preview(ui, ctx.registry);

        egui::ScrollArea::vertical()
            .id_salt("convert_scroll")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - 54.0).max(0.0))
            .show(ui, |ui| {
                ui.add_space(2.0);
                ui.label(RichText::new("单文件转换").size(20.0).strong());
                widgets::hint(
                    ui,
                    "选择源文件,设定目标格式与可选变换,即可转换。所有图解码与编码均由本软件自行实现。",
                );

                widgets::section(ui, "输入文件", |ui| {
                    if widgets::drop_zone(ui, self.inputs.len(), !busy) {
                        picked = pick_input_files(ctx.registry, ctx.config.last_input_dir.as_deref())
                            .unwrap_or_default();
                    }

                    if !self.inputs.is_empty() {
                        ui.add_space(6.0);
                        for (index, path) in self.inputs.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let name = fs::file_name(path);
                                ui.label(RichText::new(name).strong());
                                let size = std::fs::metadata(path)
                                    .map(|meta| fs::human_size(meta.len()))
                                    .unwrap_or_else(|_| "大小未知".to_string());
                                ui.label(RichText::new(size).small().color(theme::MUTED));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add_enabled(!busy, egui::Button::new("移除").small())
                                            .clicked()
                                        {
                                            remove_index = Some(index);
                                        }
                                        if ui
                                            .add_enabled(!busy, egui::Button::new("添加").small())
                                            .on_hover_text("继续选择文件")
                                            .clicked()
                                        {
                                            picked.extend(
                                                pick_input_files(
                                                    ctx.registry,
                                                    ctx.config.last_input_dir.as_deref(),
                                                )
                                                .unwrap_or_default(),
                                            );
                                        }
                                    },
                                );
                            });
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!busy, egui::Button::new("清空列表"))
                                .clicked()
                            {
                                clear_clicked = true;
                            }
                            ui.label(
                                RichText::new(format!("共 {} 个文件", self.inputs.len()))
                                    .small()
                                    .color(theme::MUTED),
                            );
                        });
                    }
                });

                if let Some(texture) = &self.preview {
                    widgets::section(ui, "源图预览", |ui| {
                        let (width, height) = self.source_size;
                        let available = ui.available_width().min(PREVIEW_MAX_SIDE as f32);
                        let aspect = if height == 0 {
                            1.0
                        } else {
                            width as f32 / height as f32
                        };
                        let render_width = available;
                        let render_height = (render_width / aspect).min(PREVIEW_MAX_SIDE as f32);
                        ui.add(
                            egui::Image::from_texture(texture)
                                .fit_to_exact_size(egui::vec2(render_width, render_height))
                                .corner_radius(egui::CornerRadius::same(4)),
                        );
                        widgets::hint(ui, &format!("原始尺寸 {width}×{height} 像素"));
                    });
                }

                widgets::section(ui, "输出参数", |ui| {
                    widgets::output_editor(ui, ctx.config, &formats);
                });

                let _ = widgets::section_collapsible(ui, "图像变换(可选)", false, |ui| {
                    widgets::transform_editor(ui, &mut ctx.config.transform, self.source_size);
                });

                let _ = widgets::section_collapsible(ui, "编码参数", false, |ui| {
                    widgets::encode_editor(
                        ui,
                        &mut ctx.config.encode,
                        ctx.config.target_format,
                    );
                });

                if let Some(outcome) = &self.outcome {
                    let success_color = theme::SUCCESS;
                    widgets::section(ui, "转换结果", |ui| {
                        ui.horizontal(|ui| {
                            widgets::badge(ui, "转换成功", success_color);
                            ui.label(RichText::new(outcome.summary()).small());
                        });
                        ui.add_space(4.0);
                        widgets::key_value(ui, "来源格式", outcome.source_format.name());
                        widgets::key_value(ui, "目标格式", outcome.target_format.name());
                        widgets::key_value(
                            ui,
                            "输出尺寸",
                            &format!("{}×{} 像素", outcome.width, outcome.height),
                        );
                        widgets::key_value(
                            ui,
                            "文件体积",
                            &format!(
                                "{} → {}({:+.1}%)",
                                fs::human_size(outcome.source_bytes),
                                fs::human_size(outcome.output_bytes),
                                outcome.size_change_percent()
                            ),
                        );
                        widgets::key_value(ui, "耗时", &format!("{} 毫秒", outcome.elapsed.as_millis()));
                        widgets::key_value(
                            ui,
                            "输出文件",
                            &outcome.output.display().to_string(),
                        );

                        if !outcome.notes.is_empty() {
                            ui.add_space(4.0);
                            for note in &outcome.notes {
                                ui.label(RichText::new(format!("• {note}")).small().color(theme::WARNING));
                            }
                        }

                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button("在文件夹中显示").clicked() {
                                reveal_target = Some(outcome.output.clone());
                            }
                            if let Some(parent) = outcome.output.parent()
                                && ui.button("打开输出目录").clicked()
                            {
                                open_dir_target = Some(parent.to_path_buf());
                            }
                        });
                    });
                }

                if let Some(message) = &self.error {
                    let danger = theme::DANGER;
                    widgets::section(ui, "转换失败", |ui| {
                        widgets::badge(ui, "失败", danger);
                        ui.add_space(4.0);
                        ui.label(RichText::new(message).color(danger));
                        ui.add_space(4.0);
                        widgets::hint(
                            ui,
                            "常见原因:文件损坏、扩展名与实际内容不符、目标格式不支持该颜色模式或尺寸。",
                        );
                    });
                }
            });

        ui.separator();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let can_start = !self.inputs.is_empty() && !busy;
            if ui
                .add_enabled(
                    can_start,
                    egui::Button::new(RichText::new("开始转换").strong()),
                )
                .on_disabled_hover_text("请先选择至少一个输入文件")
                .clicked()
            {
                start_clicked = true;
            }
            if ui
                .add_enabled(!busy, egui::Button::new("重置参数"))
                .on_hover_text("把输出与变换参数恢复为默认值")
                .clicked()
            {
                reset_clicked = true;
            }
            if busy {
                ui.spinner();
                ui.label(RichText::new("正在转换...").color(theme::MUTED));
            }
        });

        if clear_clicked {
            self.clear();
        }
        if let Some(index) = remove_index
            && index < self.inputs.len()
        {
            self.inputs.remove(index);
            self.outcome = None;
        }
        if !picked.is_empty() {
            if self.inputs.is_empty() {
                self.set_inputs(picked);
            } else {
                self.add_inputs(picked);
            }
        }
        if reset_clicked {
            ctx.config.transform = Default::default();
            ctx.config.encode = Default::default();
            ctx.config.naming = Default::default();
            ctx.config.overwrite = false;
            requests.push(UiRequest::Status("已把转换参数恢复为默认值".to_string()));
        }
        if start_clicked {
            self.outcome = None;
            self.error = None;
            self.busy = true;
            let options = ctx.config.to_convert_options();
            requests.push(UiRequest::StartBatch {
                files: self.inputs.clone(),
                options: Box::new(options),
                workers: 1,
                kind: TaskKind::Single,
            });
        }
        if let Some(path) = reveal_target {
            requests.push(UiRequest::RevealFile(path));
        }
        if let Some(dir) = open_dir_target {
            requests.push(UiRequest::OpenDirectory(dir));
        }

        requests
    }

    /// 必要时重新解码源文件并生成预览纹理。
    ///
    /// 只有当选中文件发生变化时才会真正读取磁盘,因此不会影响每帧的绘制速度。
    fn ensure_preview(&mut self, ui: &Ui, registry: &crate::core::registry::Registry) {
        let key = self.inputs.first().map(|path| path.display().to_string());
        if key == self.preview_key {
            return;
        }
        self.preview = None;
        self.preview_key = key.clone();

        let Some(path) = self.inputs.first().cloned() else {
            self.source_size = (800, 600);
            return;
        };

        let Ok(data) = fs::read_file(&path) else {
            return;
        };
        let Ok((image, _)) = decode_input(registry, &path, &data) else {
            return;
        };

        self.source_size = (image.width, image.height);
        let preview = downscale_for_preview(&image);
        let rgba = preview.to_rgba();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(
            [rgba.width as usize, rgba.height as usize],
            &rgba.data,
        );
        self.preview = Some(ui.ctx().load_texture(
            format!("source-preview:{key:?}"),
            color_image,
            egui::TextureOptions::LINEAR,
        ));
    }
}

/// 把图像等比缩小到预览尺寸以内,避免超大图直接上传纹理。
fn downscale_for_preview(image: &Image) -> Image {
    let longest = image.width.max(image.height);
    if longest <= PREVIEW_MAX_SIDE || longest == 0 {
        return image.clone();
    }
    let scale = PREVIEW_MAX_SIDE as f64 / longest as f64;
    let width = ((image.width as f64 * scale).round() as u32).max(1);
    let height = ((image.height as f64 * scale).round() as u32).max(1);
    image
        .resize_bilinear(width, height)
        .unwrap_or_else(|_| image.clone())
}

/// 弹出文件选择框,可读格式与扩展名完全来自注册表。
pub fn pick_input_files(
    registry: &crate::core::registry::Registry,
    start: Option<&Path>,
) -> Option<Vec<PathBuf>> {
    let mut extensions: Vec<&str> = registry
        .readable_formats()
        .iter()
        .flat_map(|format| format.extensions().iter().copied())
        .collect();
    extensions.sort_unstable();
    extensions.dedup();

    let mut dialog = rfd::FileDialog::new()
        .set_title("选择要转换的图片")
        .add_filter("支持的图片", &extensions);
    if let Some(dir) = start {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_files()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::build_default;
    use crate::core::pixel::ColorType;
    use crate::service::config::AppConfig;
    use crate::service::history::HistoryStore;
    use crate::ui::run_frame;

    fn sample_image() -> Image {
        Image::filled(4, 3, ColorType::Rgba8, crate::core::pixel::Rgba::new(1, 2, 3, 255))
            .expect("构造测试图像")
    }

    fn sample_outcome() -> ConversionOutcome {
        ConversionOutcome {
            input: PathBuf::from("a.bmp"),
            output: PathBuf::from("out/a.qoi"),
            source_format: crate::core::format::Format::Bmp,
            target_format: crate::core::format::Format::Qoi,
            width: 4,
            height: 3,
            source_bytes: 1000,
            output_bytes: 400,
            elapsed: std::time::Duration::from_millis(12),
            notes: vec!["体积缩小".to_string()],
        }
    }

    #[test]
    fn add_inputs_skips_duplicates() {
        let mut view = ConvertView::new();
        view.add_inputs([PathBuf::from("a.bmp"), PathBuf::from("a.bmp")]);
        view.add_inputs([PathBuf::from("b.qoi")]);
        assert_eq!(view.inputs.len(), 2);
    }

    #[test]
    fn set_inputs_clears_previous_result() {
        let mut view = ConvertView::new();
        view.report_success(sample_outcome());
        assert!(view.outcome.is_some());
        view.set_inputs(vec![PathBuf::from("c.tga")]);
        assert!(view.outcome.is_none());
        assert!(view.error.is_none());
    }

    #[test]
    fn reporting_failure_clears_busy_flag() {
        let mut view = ConvertView::new();
        view.busy = true;
        view.report_failure("文件损坏");
        assert!(!view.busy);
        assert_eq!(view.error.as_deref(), Some("文件损坏"));
    }

    #[test]
    fn reporting_success_clears_error() {
        let mut view = ConvertView::new();
        view.report_failure("文件损坏");
        view.report_success(sample_outcome());
        assert!(view.error.is_none());
        assert!(view.outcome.is_some());
    }

    #[test]
    fn clear_removes_everything() {
        let mut view = ConvertView::new();
        view.set_inputs(vec![PathBuf::from("a.bmp")]);
        view.report_success(sample_outcome());
        view.clear();
        assert!(view.inputs.is_empty());
        assert!(view.outcome.is_none());
    }

    #[test]
    fn downscale_keeps_small_images_untouched() {
        let image = sample_image();
        let preview = downscale_for_preview(&image);
        assert_eq!(preview.width, 4);
        assert_eq!(preview.height, 3);
    }

    #[test]
    fn downscale_preserves_aspect_ratio() {
        let image = Image::filled(2000, 1000, ColorType::Gray8, crate::core::pixel::Rgba::new(0, 0, 0, 255))
            .expect("构造测试图像");
        let preview = downscale_for_preview(&image);
        assert_eq!(preview.width, PREVIEW_MAX_SIDE);
        assert_eq!(preview.height, PREVIEW_MAX_SIDE / 2);
    }

    #[test]
    fn render_without_inputs_requests_nothing() {
        let registry = build_default();
        let formats = registry.writable_formats();
        let mut config = AppConfig::default();
        let history = HistoryStore::default();
        let mut ctx = UiContext {
            registry: &registry,
            config: &mut config,
            history: &history,
            formats: &formats,
            busy: false,
        };
        let mut view = ConvertView::new();
        let ctx_egui = egui::Context::default();
        let mut requests: Vec<UiRequest> = Vec::new();
        run_frame(&ctx_egui, |ui| {
            requests = view.render(ui, &mut ctx);
        });
        assert!(requests.is_empty(), "未选择文件时不应产生任何请求");
        assert!(config.transform.is_identity());
        assert_eq!(config.encode, Default::default());
    }

    #[test]
    fn render_shows_result_panel_without_panicking() {
        let registry = build_default();
        let formats = registry.writable_formats();
        let mut config = AppConfig::default();
        let history = HistoryStore::default();
        let mut ctx = UiContext {
            registry: &registry,
            config: &mut config,
            history: &history,
            formats: &formats,
            busy: false,
        };
        let mut view = ConvertView::new();
        view.report_success(sample_outcome());
        let ctx_egui = egui::Context::default();
        run_frame(&ctx_egui, |ui| {
            let _ = view.render(ui, &mut ctx);
        });
        assert!(view.outcome.is_some());
    }
}
