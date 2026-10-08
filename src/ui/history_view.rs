//! 转换历史分页。
//!
//! 历史记录以 JSON 形式保存在用户配置目录下,这里只做展示与检索,不负责写入。

use egui::{Align, Label, Layout, RichText, TextEdit, Ui};

use crate::service::history::{HistoryEntry, MAX_HISTORY_ENTRIES};
use crate::util::fs;

use super::theme;
use super::widgets;
use super::{UiContext, UiRequest};

/// 表格中"文件"列的最大宽度,超出部分省略显示。
const FILE_COLUMN_WIDTH: f32 = 300.0;

/// 转换历史分页。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryView {
    /// 检索关键字。为空时展示全部记录。
    pub keyword: String,
}

impl HistoryView {
    /// 新建一个历史分页。
    pub fn new() -> Self {
        Self::default()
    }

    /// 清空检索条件。
    pub fn reset_filter(&mut self) {
        self.keyword.clear();
    }

    /// 绘制整个分页,返回需要主程序执行的请求。
    pub fn render(&mut self, ui: &mut Ui, ctx: &mut UiContext<'_>) -> Vec<UiRequest> {
        let mut requests: Vec<UiRequest> = Vec::new();
        let mut clear_clicked = false;
        let mut reveal_target: Option<std::path::PathBuf> = None;

        let entries: Vec<&HistoryEntry> = ctx.history.search(&self.keyword);
        let total = ctx.history.len();
        let succeeded = ctx.history.succeeded();
        let failed = ctx.history.failed();
        let source_bytes = ctx.history.total_source_bytes();
        let output_bytes = ctx.history.total_output_bytes();

        egui::ScrollArea::vertical()
            .id_salt("history_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(2.0);
                ui.label(RichText::new("转换历史").size(20.0).strong());
                widgets::hint(ui, "记录每一次转换的来源、目标与结果,最多保留最近若干条。");

                widgets::section(ui, "统计概览", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        widgets::badge(ui, &format!("总计 {total} 条"), theme::ACCENT);
                        widgets::badge(ui, &format!("成功 {succeeded} 条"), theme::SUCCESS);
                        widgets::badge(ui, &format!("失败 {failed} 条"), theme::DANGER);
                        ui.label(
                            RichText::new(format!(
                                "累计输入 {} → 输出 {}",
                                fs::human_size(source_bytes),
                                fs::human_size(output_bytes)
                            ))
                            .small()
                            .color(theme::MUTED),
                        );
                    });
                    ui.add_space(4.0);
                    widgets::hint(
                        ui,
                        &format!("上限 {MAX_HISTORY_ENTRIES} 条,超出后自动丢弃最早的记录。"),
                    );
                });

                widgets::section(ui, "检索与操作", |ui| {
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 190.0).max(120.0);
                        ui.add(
                            TextEdit::singleline(&mut self.keyword)
                                .desired_width(width)
                                .hint_text("按文件路径或结果信息检索"),
                        );
                        if ui
                            .add_enabled(!self.keyword.is_empty(), egui::Button::new("清除条件"))
                            .clicked()
                        {
                            self.reset_filter();
                        }
                        if ui
                            .add_enabled(total > 0, egui::Button::new("清空历史"))
                            .on_hover_text("删除全部历史记录,该操作不可撤销")
                            .clicked()
                        {
                            clear_clicked = true;
                        }
                    });
                    widgets::hint(
                        ui,
                        &format!(
                            "当前筛选出 {} 条记录{}",
                            entries.len(),
                            if self.keyword.trim().is_empty() {
                                ""
                            } else {
                                "(已应用检索条件)"
                            }
                        ),
                    );
                });

                widgets::section(ui, "记录明细", |ui| {
                    if entries.is_empty() {
                        widgets::hint(
                            ui,
                            if total == 0 {
                                "还没有任何记录。完成一次转换后,这里会自动出现对应条目。"
                            } else {
                                "没有匹配的记录,可尝试清空检索条件。"
                            },
                        );
                        return;
                    }

                    egui::Grid::new("history_grid")
                        .num_columns(9)
                        .striped(true)
                        .spacing([12.0, 6.0])
                        .min_col_width(58.0)
                        .show(ui, |ui| {
                            for header in [
                                "时间", "来源", "目标", "尺寸", "体积", "耗时", "状态", "文件", "",
                            ] {
                                ui.label(RichText::new(header).strong().small());
                            }
                            ui.end_row();

                            for entry in &entries {
                                ui.label(RichText::new(entry.time_text()).small());
                                ui.label(RichText::new(entry.format_text()).small());
                                ui.label(RichText::new(entry.target_format.name()).small());
                                ui.label(
                                    RichText::new(format!("{}×{}", entry.width, entry.height))
                                        .small(),
                                );
                                ui.label(RichText::new(entry.volume_text()).small());

                                let elapsed = ui.label(RichText::new(entry.elapsed_text()).small());
                                elapsed.on_hover_text(format!(
                                    "输入 {} / 输出 {}",
                                    fs::human_size(entry.source_bytes),
                                    fs::human_size(entry.output_bytes)
                                ));

                                let color = if entry.success {
                                    theme::SUCCESS
                                } else {
                                    theme::DANGER
                                };
                                widgets::badge(ui, entry.status_text(), color);

                                let name = fs::file_name(&entry.input);
                                ui.add_sized(
                                    [FILE_COLUMN_WIDTH, 20.0],
                                    Label::new(RichText::new(name).small())
                                        .truncate(),
                                )
                                .on_hover_text(format!(
                                    "{}\n{}",
                                    entry.input.display(),
                                    entry.message
                                ));

                                let target = if entry.success {
                                    entry.output.clone()
                                } else {
                                    entry.input.clone()
                                };
                                let action = widgets::small_action(ui, "定位");
                                if action
                                    .on_hover_text("在文件资源管理器中定位该文件")
                                    .clicked()
                                {
                                    reveal_target = Some(target);
                                }

                                ui.end_row();
                            }
                        });
                });

                ui.add_space(8.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    widgets::hint(ui, &format!("共 {} 条记录", entries.len()));
                });
            });

        if clear_clicked {
            requests.push(UiRequest::ClearHistory);
        }
        if let Some(path) = reveal_target {
            requests.push(UiRequest::RevealFile(path));
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
    use crate::ui::run_frame;

    fn entry(success: bool, input: &str) -> HistoryEntry {
        HistoryEntry {
            timestamp: chrono::Local::now(),
            input: std::path::PathBuf::from(input),
            output: std::path::PathBuf::from("out/result.qoi"),
            source_format: Some(Format::Bmp),
            target_format: Format::Qoi,
            width: 320,
            height: 240,
            source_bytes: 4096,
            output_bytes: 1024,
            elapsed_ms: 7,
            success,
            message: if success {
                "转换成功".to_string()
            } else {
                "文件损坏".to_string()
            },
        }
    }

    /// 在无窗口的 egui 上下文里绘制一帧历史分页,返回请求与绘制后的视图状态。
    fn render(history: &crate::service::history::HistoryStore, keyword: &str) -> (Vec<UiRequest>, HistoryView) {
        let registry = build_default();
        let formats = registry.writable_formats();
        let mut config = AppConfig::default();
        let mut view = HistoryView {
            keyword: keyword.to_string(),
        };
        let mut requests: Vec<UiRequest> = Vec::new();
        {
            let mut ctx = UiContext {
                registry: &registry,
                config: &mut config,
                history,
                formats: &formats,
                busy: false,
            };
            let ctx_egui = egui::Context::default();
            run_frame(&ctx_egui, |ui| {
                requests = view.render(ui, &mut ctx);
            });
        }
        (requests, view)
    }

    #[test]
    fn reset_filter_clears_keyword() {
        let mut view = HistoryView::new();
        view.keyword = "bmp".to_string();
        view.reset_filter();
        assert!(view.keyword.is_empty());
    }

    #[test]
    fn render_empty_history_requests_nothing() {
        let history = crate::service::history::HistoryStore::default();
        let (requests, _) = render(&history, "");
        assert!(requests.is_empty());
    }

    #[test]
    fn render_with_entries_produces_no_requests_on_first_frame() {
        let mut history = crate::service::history::HistoryStore::default();
        history.push(entry(true, "a.bmp"));
        history.push(entry(false, "b.tga"));
        let (requests, _) = render(&history, "");
        assert!(requests.is_empty(), "仅仅绘制不应产生任何请求");
    }

    #[test]
    fn render_with_filter_matching_nothing_works() {
        let mut history = crate::service::history::HistoryStore::default();
        history.push(entry(true, "a.bmp"));
        let (requests, _) = render(&history, "绝不匹配的关键字");
        assert!(requests.is_empty());
    }
}
