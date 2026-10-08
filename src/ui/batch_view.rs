//! 批量转换分页。
//!
//! 与单文件分页共用同一套参数编辑器,区别在于:输入是文件列表,任务由线程池并行执行,
//! 界面通过轮询任务事件来刷新进度与日志,不会阻塞绘制。

use std::path::PathBuf;

use egui::{Align, Layout, RichText, Slider, Ui};

use crate::service::config::default_workers;
use crate::service::task::TaskSummary;
use crate::util::fs;

use super::theme;
use super::widgets;
use super::{TaskKind, UiContext, UiRequest};

/// 日志最多保留的行数,防止长时间运行占满内存。
const MAX_LOG_LINES: usize = 2000;

/// 文件列表与日志区域的最大高度。
const FILE_LIST_HEIGHT: f32 = 156.0;
const LOG_HEIGHT: f32 = 170.0;

/// 源图尺寸未知时,缩放控件的兜底默认值。
const FALLBACK_SIZE: (u32, u32) = (1920, 1080);

/// 日志行的级别,决定文字颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    /// 一般信息。
    Info,
    /// 成功。
    Success,
    /// 失败。
    Failure,
}

/// 一条运行日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// 日志正文。
    pub text: String,
    /// 级别。
    pub kind: LogKind,
}

/// 批量转换分页。
#[derive(Default)]
pub struct BatchView {
    /// 待转换的文件列表。
    pub files: Vec<PathBuf>,
    /// 运行日志,最新的在最下面。
    pub log: Vec<LogLine>,
    /// 最近一次批量任务的汇总。
    pub summary: Option<TaskSummary>,
    /// 已处理的文件数。
    pub processed: usize,
    /// 本次任务的文件总数。
    pub total: usize,
}

impl BatchView {
    /// 新建一个空的批量分页。
    pub fn new() -> Self {
        Self::default()
    }

    /// 替换待转换文件列表。
    pub fn set_files(&mut self, files: Vec<PathBuf>) {
        self.files = files;
        self.summary = None;
        self.processed = 0;
        self.total = self.files.len();
    }

    /// 追加文件,自动跳过重复项。
    pub fn add_inputs(&mut self, files: impl IntoIterator<Item = PathBuf>) {
        for file in files {
            if !self.files.contains(&file) {
                self.files.push(file);
            }
        }
        self.total = self.files.len();
    }

    /// 清空文件列表与汇总结果。
    pub fn clear_files(&mut self) {
        self.files.clear();
        self.summary = None;
        self.processed = 0;
        self.total = 0;
    }

    /// 追加一行日志。
    pub fn push_log(&mut self, text: impl Into<String>, kind: LogKind) {
        self.log.push(LogLine {
            text: text.into(),
            kind,
        });
        if self.log.len() > MAX_LOG_LINES {
            let overflow = self.log.len() - MAX_LOG_LINES;
            self.log.drain(0..overflow);
        }
    }

    /// 清空日志。
    pub fn clear_log(&mut self) {
        self.log.clear();
    }

    /// 开始一批任务,`workers` 为实际使用的线程数。
    pub fn begin(&mut self, total: usize, workers: usize) {
        self.total = total;
        self.processed = 0;
        self.summary = None;
        self.clear_log();
        self.push_log(
            format!("开始处理 {total} 个文件,使用 {workers} 个并行线程"),
            LogKind::Info,
        );
    }

    /// 更新已处理数量。
    pub fn advance(&mut self, processed: usize) {
        self.processed = processed;
    }

    /// 收尾并记录汇总。
    pub fn finish(&mut self, summary: TaskSummary) {
        self.processed = summary.processed();
        self.total = summary.total;
        self.push_log(summary.summary_text(), LogKind::Info);
        self.summary = Some(summary);
    }

    /// 绘制整个分页,返回需要主程序执行的请求。
    pub fn render(&mut self, ui: &mut Ui, ctx: &mut UiContext<'_>) -> Vec<UiRequest> {
        let mut requests: Vec<UiRequest> = Vec::new();
        let busy = ctx.busy;
        let formats = ctx.formats.to_vec();
        let workers_max = default_workers().max(1);
        let empty = self.files.is_empty();

        let mut picked: Vec<PathBuf> = Vec::new();
        let mut remove_index: Option<usize> = None;
        let mut clear_files_clicked = false;
        let mut clear_log = false;
        let mut start = false;
        let mut cancel = false;
        let mut config_changed = false;

        egui::ScrollArea::vertical()
            .id_salt("batch_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(2.0);
                ui.label(RichText::new("批量转换").size(20.0).strong());
                widgets::hint(
                    ui,
                    "一次提交多个文件,由后台线程并行转换;参数与单文件转换完全一致。",
                );

                widgets::section(ui, "待转换文件", |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("添加文件"))
                            .clicked()
                        {
                            picked = super::convert_view::pick_input_files(
                                ctx.registry,
                                ctx.config.last_input_dir.as_deref(),
                            )
                            .unwrap_or_default();
                        }
                        if ui
                            .add_enabled(!busy && !empty, egui::Button::new("清空列表"))
                            .clicked()
                        {
                            clear_files_clicked = true;
                        }
                        let total_bytes: u64 = self
                            .files
                            .iter()
                            .filter_map(|path| std::fs::metadata(path).ok())
                            .map(|meta| meta.len())
                            .sum();
                        widgets::right_aligned_muted(
                            ui,
                            &format!("{} 个文件 / 合计 {}", self.files.len(), fs::human_size(total_bytes)),
                        );
                    });

                    ui.add_space(4.0);
                    if self.files.is_empty() {
                        widgets::hint(ui, "列表为空。点击上方的添加文件按钮,或把文件直接拖到窗口里。");
                    } else {
                        egui::ScrollArea::vertical()
                            .id_salt("batch_file_list")
                            .max_height(FILE_LIST_HEIGHT)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for (index, path) in self.files.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(fs::file_name(path)).strong());
                                        let size = std::fs::metadata(path)
                                            .map(|meta| fs::human_size(meta.len()))
                                            .unwrap_or_else(|_| "大小未知".to_string());
                                        ui.label(RichText::new(size).small().color(theme::MUTED));
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui
                                                    .add_enabled(
                                                        !busy,
                                                        egui::Button::new("移除").small(),
                                                    )
                                                    .clicked()
                                                {
                                                    remove_index = Some(index);
                                                }
                                            },
                                        );
                                    });
                                }
                            });
                    }
                });

                // 页面顺序:文件列表 → 任务控制 → 运行日志 → 高级参数。
                // 批量转换时最常用的是"选文件、开始、看日志",把参数组放到页尾并默认收起,
                // 运行日志才能留在首屏;标题里带上当前目标格式,收起时也不会藏住关键信息。
                widgets::section(ui, "任务控制", |ui| {
                    widgets::field(ui, "并行线程", |ui| {
                        config_changed |= ui
                            .add(
                                Slider::new(&mut ctx.config.max_workers, 1..=workers_max)
                                    .integer()
                                    .suffix(" 个"),
                            )
                            .changed();
                        ui.label(
                            RichText::new(format!("本机可用 {workers_max} 个"))
                                .small()
                                .color(theme::MUTED),
                        );
                    });

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !empty && !busy,
                                egui::Button::new(RichText::new("开始批量转换").strong()),
                            )
                            .on_disabled_hover_text("请先添加文件,并等待当前任务结束")
                            .clicked()
                        {
                            start = true;
                        }
                        if ui
                            .add_enabled(busy, egui::Button::new("取消任务"))
                            .on_hover_text("已开始的文件会跑完,剩余文件不再处理")
                            .clicked()
                        {
                            cancel = true;
                        }
                        if busy {
                            ui.spinner();
                        }
                    });

                    let progress = if self.total == 0 {
                        0.0
                    } else {
                        self.processed as f32 / self.total as f32
                    };
                    let label = format!(
                        "{}/{} 文件",
                        self.processed,
                        self.total.max(self.files.len())
                    );
                    ui.add_space(6.0);
                    widgets::progress_row(ui, progress, &label);
                });

                widgets::section(ui, "运行日志", |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.log.is_empty(), egui::Button::new("清空日志"))
                            .clicked()
                        {
                            clear_log = true;
                        }
                        widgets::right_aligned_muted(ui, &format!("{} 行", self.log.len()));
                    });
                    ui.add_space(4.0);

                    if self.log.is_empty() {
                        widgets::hint(ui, "暂无日志。任务开始后会在这里显示每个文件的处理结果。");
                    } else {
                        egui::ScrollArea::vertical()
                            .id_salt("batch_log")
                            .max_height(LOG_HEIGHT)
                            .auto_shrink([false, true])
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                for line in &self.log {
                                    let color = match line.kind {
                                        LogKind::Info => theme::MUTED,
                                        LogKind::Success => theme::SUCCESS,
                                        LogKind::Failure => theme::DANGER,
                                    };
                                    ui.label(RichText::new(&line.text).small().color(color));
                                }
                            });
                    }
                });

                let output_title =
                    format!("输出参数(目标格式 {})", ctx.config.target_format.name());
                if let Some(changed) =
                    widgets::section_collapsible(ui, &output_title, false, |ui| {
                        widgets::output_editor(ui, ctx.config, &formats)
                    })
                {
                    config_changed |= changed;
                }

                if let Some(changed) =
                    widgets::section_collapsible(ui, "图像变换(可选)", false, |ui| {
                        widgets::transform_editor(ui, &mut ctx.config.transform, FALLBACK_SIZE)
                    })
                {
                    config_changed |= changed;
                }

                if let Some(changed) = widgets::section_collapsible(ui, "编码参数", false, |ui| {
                    widgets::encode_editor(ui, &mut ctx.config.encode, ctx.config.target_format)
                }) {
                    config_changed |= changed;
                }

                if let Some(summary) = &self.summary {
                    let color = if summary.failed == 0 && !summary.cancelled {
                        theme::SUCCESS
                    } else {
                        theme::WARNING
                    };
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        widgets::badge(
                            ui,
                            if summary.cancelled { "已取消" } else { "任务结束" },
                            color,
                        );
                        ui.label(RichText::new(summary.summary_text()).small());
                    });
                }
            });

        // ---- 先处理状态更新,再处理需要主程序执行的动作 ----
        if clear_files_clicked {
            self.clear_files();
        }
        if let Some(index) = remove_index
            && index < self.files.len()
        {
            self.files.remove(index);
            self.total = self.files.len();
        }
        if !picked.is_empty() {
            self.add_inputs(picked);
        }
        if clear_log {
            self.clear_log();
        }

        if config_changed {
            requests.push(UiRequest::SaveConfig);
        }
        if start {
            if let Some(dir) = self.files.iter().find_map(|path| path.parent().map(|p| p.to_path_buf()))
            {
                ctx.config.last_input_dir = Some(dir);
                requests.push(UiRequest::SaveConfig);
            }
            self.begin(self.files.len(), ctx.config.max_workers.max(1));
            let workers = ctx.config.max_workers.max(1);
            let options = ctx.config.to_convert_options();
            requests.push(UiRequest::StartBatch {
                files: self.files.clone(),
                options: Box::new(options),
                workers,
                kind: TaskKind::Batch,
            });
        }
        if cancel {
            requests.push(UiRequest::CancelBatch);
        }

        requests
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::build_default;
    use crate::service::config::AppConfig;
    use crate::service::history::HistoryStore;
    use crate::ui::run_frame;

    fn summary(total: usize, succeeded: usize, failed: usize, cancelled: bool) -> TaskSummary {
        TaskSummary {
            total,
            succeeded,
            failed,
            cancelled,
            elapsed: std::time::Duration::from_millis(250),
        }
    }

    #[test]
    fn add_inputs_skips_duplicates() {
        let mut view = BatchView::new();
        view.add_inputs([PathBuf::from("a.bmp"), PathBuf::from("a.bmp")]);
        view.add_inputs([PathBuf::from("b.qoi")]);
        assert_eq!(view.files.len(), 2);
    }

    #[test]
    fn clear_files_resets_counters() {
        let mut view = BatchView::new();
        view.set_files(vec![PathBuf::from("a.bmp")]);
        view.finish(summary(1, 1, 0, false));
        view.clear_files();
        assert!(view.files.is_empty());
        assert_eq!(view.total, 0);
        assert_eq!(view.processed, 0);
        assert!(view.summary.is_none());
    }

    #[test]
    fn begin_clears_previous_log() {
        let mut view = BatchView::new();
        view.push_log("旧的日志", LogKind::Info);
        view.begin(3, 2);
        assert_eq!(view.log.len(), 1, "开始新任务时应清空旧日志");
        assert_eq!(view.total, 3);
        assert_eq!(view.processed, 0);
    }

    #[test]
    fn advance_and_finish_track_progress() {
        let mut view = BatchView::new();
        view.begin(4, 1);
        view.advance(2);
        assert_eq!(view.processed, 2);
        view.finish(summary(4, 3, 1, false));
        assert_eq!(view.processed, 4);
        assert_eq!(view.summary.as_ref().map(|item| item.failed), Some(1));
    }

    #[test]
    fn log_is_capped_to_max_lines() {
        let mut view = BatchView::new();
        for index in 0..(MAX_LOG_LINES + 50) {
            view.push_log(format!("第 {index} 行"), LogKind::Info);
        }
        assert_eq!(view.log.len(), MAX_LOG_LINES);
        let last = MAX_LOG_LINES + 49;
        let expected = format!("第 {last} 行");
        assert_eq!(
            view.log.last().map(|line| line.text.as_str()),
            Some(expected.as_str())
        );
    }

    #[test]
    fn render_with_empty_list_offers_no_start() {
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
        let mut view = BatchView::new();
        let ctx_egui = egui::Context::default();
        let mut requests: Vec<UiRequest> = Vec::new();
        run_frame(&ctx_egui, |ui| {
            requests = view.render(ui, &mut ctx);
        });
        assert!(
            !requests
                .iter()
                .any(|request| matches!(request, UiRequest::StartBatch { .. })),
            "列表为空时不应提交任务"
        );
    }

    #[test]
    fn render_with_files_and_summary_works() {
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
        let mut view = BatchView::new();
        view.set_files(vec![PathBuf::from("a.bmp"), PathBuf::from("b.bmp")]);
        view.push_log("a.bmp 转换成功", LogKind::Success);
        view.push_log("b.bmp 读取失败", LogKind::Failure);
        view.finish(summary(2, 1, 1, false));
        let ctx_egui = egui::Context::default();
        run_frame(&ctx_egui, |ui| {
            let _ = view.render(ui, &mut ctx);
        });
        assert!(view.summary.is_some());
    }
}
