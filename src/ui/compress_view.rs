//! 图片压缩专用分页。
//!
//! 专为缩小图片体积设计:
//! - 支持拖放单张或批量图片;
//! - 提供「极速无损」、「智能均衡」、「极限压缩」、「自定义」4 个大号预设卡片;
//! - 支持保持原格式或转换为 WebP/JPEG;
//! - 尺寸降采样（保持原尺寸、1920px、2560px、75%、50%）;
//! - 极速模式（PNG 快速滤波器）;
//! - 结果展示：总体积节省统计卡片以及逐项对比列表。

use std::path::PathBuf;

use egui::{Button, Layout, RichText, ScrollArea, Slider, Ui};

use crate::convert::ConversionOutcome;
use crate::core::options::{
    CompressFormatStrategy, CompressPreset, DownscaleLimit, NamingRule,
};
use crate::service::config::default_workers;
use crate::ui::convert_view::pick_input_files;
use crate::util::fs;

use super::theme;
use super::widgets;
use super::{UiContext, UiRequest};

/// 单个文件列表区域最大高度。
const FILE_LIST_MAX_HEIGHT: f32 = 140.0;
/// 压缩结果明细列表最大高度。
const OUTCOMES_MAX_HEIGHT: f32 = 220.0;

/// 图片压缩专用分页。
#[derive(Default)]
pub struct CompressView {
    /// 待压缩的文件列表。
    pub files: Vec<PathBuf>,
    /// 压缩成功的详细结果清单。
    pub outcomes: Vec<ConversionOutcome>,
    /// 是否正在运行后台压缩任务。
    pub busy: bool,
    /// 已处理完成的文件数量。
    pub processed: usize,
    /// 任务总文件数。
    pub total: usize,
}

impl CompressView {
    /// 新建空的压缩分页。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置待压缩文件列表。
    pub fn set_files(&mut self, files: Vec<PathBuf>) {
        self.files = files;
        self.outcomes.clear();
        self.processed = 0;
        self.total = self.files.len();
    }

    /// 追加文件，自动去重。
    pub fn add_inputs(&mut self, files: impl IntoIterator<Item = PathBuf>) {
        for file in files {
            if !self.files.contains(&file) {
                self.files.push(file);
            }
        }
        self.total = self.files.len();
    }

    /// 清空文件列表与结果。
    pub fn clear_files(&mut self) {
        self.files.clear();
        self.outcomes.clear();
        self.processed = 0;
        self.total = 0;
    }

    /// 开始压缩任务。
    pub fn begin(&mut self, total: usize) {
        self.total = total;
        self.processed = 0;
        self.outcomes.clear();
        self.busy = true;
    }

    /// 记录单个文件的完成结果。
    pub fn record_outcome(&mut self, outcome: ConversionOutcome) {
        self.outcomes.push(outcome);
        self.processed = self.outcomes.len();
    }

    /// 更新进度计数。
    pub fn advance(&mut self, processed: usize) {
        self.processed = processed;
    }

    /// 结束压缩任务。
    pub fn finish(&mut self) {
        self.busy = false;
    }

    /// 计算累计原始总字节与压缩后总字节。
    pub fn total_sizes(&self) -> (u64, u64) {
        let mut source_total = 0u64;
        let mut output_total = 0u64;
        for item in &self.outcomes {
            source_total += item.source_bytes;
            output_total += item.output_bytes;
        }
        (source_total, output_total)
    }

    /// 绘制压缩分页。
    pub fn render(&mut self, ui: &mut Ui, ctx: &mut UiContext<'_>) -> Vec<UiRequest> {
        let mut requests: Vec<UiRequest> = Vec::new();
        let busy = ctx.busy;
        let empty = self.files.is_empty();

        let mut picked_files: Vec<PathBuf> = Vec::new();
        let mut remove_index: Option<usize> = None;
        let mut clear_clicked = false;
        let mut start_clicked = false;
        let mut cancel_clicked = false;
        let mut reveal_target: Option<PathBuf> = None;
        let mut preset_to_apply: Option<CompressPreset> = None;

        ScrollArea::vertical()
            .id_salt("compress_scroll")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - 54.0).max(0.0))
            .show(ui, |ui| {
                ui.add_space(2.0);
                ui.label(RichText::new("图片压缩").size(20.0).strong());
                widgets::hint(
                    ui,
                    "专为图片瘦身设计，支持智能预设、批量极速并发压缩与体积前后对比",
                );
                ui.add_space(8.0);

                // ==================== 1. 文件选择与列表 ====================
                widgets::section(ui, "待压缩文件", |ui| {
                    if widgets::drop_zone(ui, self.files.len(), !busy) {
                        picked_files.extend(
                            pick_input_files(
                                ctx.registry,
                                ctx.config.last_input_dir.as_deref(),
                            )
                            .unwrap_or_default(),
                        );
                    }

                    if !self.files.is_empty() {
                        ui.add_space(6.0);
                        ScrollArea::vertical()
                            .id_salt("compress_files_list")
                            .max_height(FILE_LIST_MAX_HEIGHT)
                            .show(ui, |ui| {
                                for (index, path) in self.files.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(fs::file_name(path)).strong());
                                        let size_str = std::fs::metadata(path)
                                            .map(|m| fs::human_size(m.len()))
                                            .unwrap_or_else(|_| "大小未知".to_string());
                                        ui.label(RichText::new(size_str).small().color(theme::MUTED));

                                        ui.with_layout(
                                            Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .add_enabled(!busy, Button::new("移除").small())
                                                    .clicked()
                                                {
                                                    remove_index = Some(index);
                                                }
                                            },
                                        );
                                    });
                                }
                            });

                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!busy, Button::new("清空列表").small())
                                .clicked()
                            {
                                clear_clicked = true;
                            }
                            if ui
                                .add_enabled(!busy, Button::new("添加更多文件...").small())
                                .clicked()
                            {
                                picked_files.extend(
                                    pick_input_files(
                                        ctx.registry,
                                        ctx.config.last_input_dir.as_deref(),
                                    )
                                    .unwrap_or_default(),
                                );
                            }
                            ui.label(
                                RichText::new(format!("共 {} 个待处理文件", self.files.len()))
                                    .small()
                                    .color(theme::MUTED),
                            );
                        });
                    }
                });

                ui.add_space(4.0);

                // ==================== 2. 压缩预设选择 ====================
                widgets::section(ui, "压缩预设方案", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for preset in CompressPreset::all() {
                            let is_active = ctx.config.compress.preset == preset;
                            let btn_text = RichText::new(preset.name())
                                .strong()
                                .size(14.0)
                                .color(if is_active { theme::ACCENT } else { ui.visuals().text_color() });
                            let resp = ui.add_enabled(
                                !busy,
                                Button::new(btn_text)
                                    .min_size(egui::vec2(100.0, 32.0))
                                    .selected(is_active),
                            );
                            if resp.clicked() {
                                preset_to_apply = Some(preset);
                            }
                        }
                    });
                    ui.add_space(4.0);
                    widgets::hint(ui, ctx.config.compress.preset.hint());
                });

                ui.add_space(4.0);

                // ==================== 3. 详细参数调节 ====================
                widgets::section(ui, "压缩选项微调", |ui| {
                    let compress_opts = &mut ctx.config.compress;

                    widgets::field(ui, "输出格式", |ui| {
                        widgets::enum_combo(
                            ui,
                            "compress_format_strat",
                            &mut compress_opts.format_strategy,
                            &CompressFormatStrategy::all(),
                            |item| item.name(),
                        );
                    });

                    widgets::field(ui, "压缩质量", |ui| {
                        let mut q = compress_opts.quality as f64;
                        let slider = Slider::new(&mut q, 1.0..=100.0)
                            .integer()
                            .suffix(" / 100");
                        if ui
                            .add_enabled(!busy, slider)
                            .on_hover_text("数值越低压缩率越高，建议保持在 70~90 之间")
                            .changed()
                        {
                            compress_opts.quality = q.round() as u8;
                            compress_opts.preset = CompressPreset::Custom;
                        }
                    });

                    widgets::field(ui, "尺寸降采样", |ui| {
                        if widgets::enum_combo(
                            ui,
                            "compress_downscale_limit",
                            &mut compress_opts.downscale,
                            &DownscaleLimit::all_presets(),
                            |item| item.name(),
                        ) {
                            compress_opts.preset = CompressPreset::Custom;
                        }
                    });

                    widgets::field(ui, "极速模式", |ui| {
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Checkbox::new(&mut compress_opts.png_fast_mode, "PNG快速滤波器"),
                            )
                            .on_hover_text("开启后大幅缩短导出耗时，特别适合高分辨率图片")
                            .changed()
                        {
                            compress_opts.preset = CompressPreset::Custom;
                        }
                    });

                    widgets::field(ui, "输出目录", |ui| {
                        let mut dir = if compress_opts.output_dir.as_os_str().is_empty() {
                            None
                        } else {
                            Some(compress_opts.output_dir.clone())
                        };
                        if widgets::output_dir_editor(ui, &mut dir) {
                            compress_opts.output_dir = dir.unwrap_or_default();
                        }
                    });

                    widgets::field(ui, "命名规则", |ui| {
                        widgets::enum_combo(
                            ui,
                            "compress_naming",
                            &mut compress_opts.naming,
                            &NamingRule::all(),
                            |item| item.name(),
                        );
                    });

                    widgets::field(ui, "覆盖文件", |ui| {
                        ui.checkbox(&mut compress_opts.overwrite, "允许覆盖同名文件");
                    });
                });

                ui.add_space(8.0);

                // ==================== 4. 开始与取消操作 ====================
                ui.horizontal(|ui| {
                    let start_label = if busy {
                        format!("正在压缩 ({}/{}) ...", self.processed, self.total)
                    } else if empty {
                        "开始压缩".to_string()
                    } else {
                        format!("开始压缩 (共 {} 个文件)", self.files.len())
                    };

                    let start_btn = ui.add_enabled(
                        !empty && !busy,
                        Button::new(RichText::new(start_label).strong().size(15.0))
                            .min_size(egui::vec2(160.0, 36.0)),
                    );
                    if start_btn.clicked() {
                        start_clicked = true;
                    }

                    if busy {
                        let cancel_btn = ui.add(
                            Button::new(RichText::new("取消任务").size(14.0))
                                .min_size(egui::vec2(80.0, 36.0)),
                        );
                        if cancel_btn.clicked() {
                            cancel_clicked = true;
                        }
                    }
                });

                if busy {
                    ui.add_space(6.0);
                    let progress = if self.total > 0 {
                        self.processed as f32 / self.total as f32
                    } else {
                        0.0
                    };
                    widgets::progress_row(
                        ui,
                        progress,
                        &format!("已完成 {} / {}", self.processed, self.total),
                    );
                }

                ui.add_space(8.0);

                // ==================== 5. 压缩成果与统计卡片 ====================
                if !self.outcomes.is_empty() {
                    let (total_src, total_out) = self.total_sizes();
                    let saved_bytes = total_src.saturating_sub(total_out);
                    let percent = if total_src > 0 {
                        (total_out as f64 - total_src as f64) / total_src as f64 * 100.0
                    } else {
                        0.0
                    };

                    widgets::section(ui, "压缩成果总览", |ui| {
                        ui.horizontal(|ui| {
                            widgets::compression_badge(ui, percent);
                            if total_out <= total_src {
                                ui.label(
                                    RichText::new(format!(
                                        "累计节省空间: {} ({} → {})",
                                        fs::human_size(saved_bytes),
                                        fs::human_size(total_src),
                                        fs::human_size(total_out)
                                    ))
                                    .strong()
                                    .color(theme::SUCCESS),
                                );
                            } else {
                                ui.label(
                                    RichText::new(format!(
                                        "总体积变化: {} → {}",
                                        fs::human_size(total_src),
                                        fs::human_size(total_out)
                                    ))
                                    .strong(),
                                );
                            }
                        });

                        ui.add_space(6.0);
                        ScrollArea::vertical()
                            .id_salt("compress_outcomes_list")
                            .max_height(OUTCOMES_MAX_HEIGHT)
                            .show(ui, |ui| {
                                for outcome in &self.outcomes {
                                    ui.horizontal(|ui| {
                                        let name = fs::file_name(&outcome.input);
                                        ui.label(RichText::new(name).strong());
                                        ui.label(
                                            RichText::new(format!(
                                                "{} → {}",
                                                fs::human_size(outcome.source_bytes),
                                                fs::human_size(outcome.output_bytes)
                                            ))
                                            .small()
                                            .color(theme::MUTED),
                                        );
                                        widgets::compression_badge(ui, outcome.size_change_percent());
                                        ui.label(
                                            RichText::new(format!("{}×{} px", outcome.width, outcome.height))
                                                .small()
                                                .color(theme::MUTED),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui.add(Button::new("定位").small()).clicked() {
                                                    reveal_target = Some(outcome.output.clone());
                                                }
                                            },
                                        );
                                    });
                                }
                            });
                    });
                }
            });

        // ==================== 状态变更与请求收集 ====================
        if let Some(preset) = preset_to_apply {
            ctx.config.compress.apply_preset(preset);
            requests.push(UiRequest::SaveConfig);
        }

        if let Some(target) = reveal_target {
            requests.push(UiRequest::RevealFile(target));
        }

        if clear_clicked {
            self.clear_files();
        }

        if let Some(index) = remove_index
            && index < self.files.len()
        {
            self.files.remove(index);
            self.total = self.files.len();
        }

        if !picked_files.is_empty() {
            if let Some(parent) = picked_files.first().and_then(|p| p.parent()) {
                ctx.config.last_input_dir = Some(parent.to_path_buf());
                requests.push(UiRequest::SaveConfig);
            }
            self.add_inputs(picked_files);
        }

        if start_clicked && !self.files.is_empty() {
            let files = self.files.clone();
            let workers = default_workers().max(1);
            let options = ctx.config.to_compress_options();
            self.begin(files.len());
            requests.push(UiRequest::StartCompress {
                files,
                options: Box::new(options),
                workers,
            });
            requests.push(UiRequest::SaveConfig);
        }

        if cancel_clicked {
            requests.push(UiRequest::CancelBatch);
        }

        requests
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::build_default;
    use crate::core::format::Format;
    use crate::service::config::AppConfig;
    use crate::service::history::HistoryStore;
    use crate::ui::run_frame;

    fn sample_outcome() -> ConversionOutcome {
        ConversionOutcome {
            input: PathBuf::from("photo.jpg"),
            output: PathBuf::from("photo_compressed.jpg"),
            source_format: Format::Jpeg,
            target_format: Format::Jpeg,
            width: 1920,
            height: 1080,
            source_bytes: 4_000_000,
            output_bytes: 800_000,
            elapsed: std::time::Duration::from_millis(45),
            notes: vec!["已节省空间".to_string()],
        }
    }

    #[test]
    fn compress_view_records_outcomes_and_calculates_sizes() {
        let mut view = CompressView::new();
        view.set_files(vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")]);
        assert_eq!(view.total, 2);

        view.record_outcome(sample_outcome());
        assert_eq!(view.outcomes.len(), 1);
        let (src, out) = view.total_sizes();
        assert_eq!(src, 4_000_000);
        assert_eq!(out, 800_000);

        view.clear_files();
        assert!(view.files.is_empty());
        assert!(view.outcomes.is_empty());
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

        let mut view = CompressView::new();
        let ctx_egui = egui::Context::default();
        let mut requests: Vec<UiRequest> = Vec::new();
        run_frame(&ctx_egui, |ui| {
            requests = view.render(ui, &mut ctx);
        });
        assert!(requests.is_empty());
    }

    #[test]
    fn render_with_outcomes_calculates_summary() {
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

        let mut view = CompressView::new();
        view.record_outcome(sample_outcome());
        let ctx_egui = egui::Context::default();
        run_frame(&ctx_egui, |ui| {
            let _ = view.render(ui, &mut ctx);
        });
        assert_eq!(view.outcomes.len(), 1);
    }
}
