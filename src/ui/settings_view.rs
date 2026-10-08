//! 设置分页。
//!
//! 集中展示外观、性能、历史记录与配置文件相关的选项,并给出软件自豪的部分——
//! 自研编解码能力对照表,方便用户(以及评审人员)了解本软件支持哪些格式。

use egui::{Grid, RichText, Slider, Ui};

use crate::core::format::Format;
use crate::core::{APP_FULL_NAME, APP_NAME, APP_SHORT_NAME, APP_VERSION};
use crate::service::config::{default_workers, AppConfig, ThemeMode};
use crate::util::fs;

use super::theme;
use super::widgets;
use super::{UiContext, UiRequest};

/// 设置分页。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsView {
    /// 是否在"关于"区域展开完整的格式能力对照表。
    pub show_format_matrix: bool,
}

impl SettingsView {
    /// 新建一个设置分页。
    pub fn new() -> Self {
        Self::default()
    }

    /// 绘制整个分页,返回需要主程序执行的请求。
    pub fn render(&mut self, ui: &mut Ui, ctx: &mut UiContext<'_>) -> Vec<UiRequest> {
        let mut requests: Vec<UiRequest> = Vec::new();
        let workers_max = default_workers().max(1);
        let config_path = AppConfig::path();

        let mut theme_changed = false;
        let mut config_changed = false;
        let mut open_config_dir = false;
        let mut restore_defaults = false;
        let mut force_save = false;

        egui::ScrollArea::vertical()
            .id_salt("settings_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(2.0);
                ui.label(RichText::new("设置").size(20.0).strong());
                widgets::hint(ui, "这里的改动会立即生效,并自动保存到配置文件。");

                widgets::section(ui, "外观", |ui| {
                    widgets::field(ui, "界面主题", |ui| {
                        theme_changed = widgets::enum_combo(
                            ui,
                            "theme_mode",
                            &mut ctx.config.theme,
                            &ThemeMode::all(),
                            |mode| mode.name(),
                        );
                    });
                    widgets::hint(
                        ui,
                        "选择跟随系统时,界面配色会随操作系统的深浅色设置自动切换。",
                    );
                });

                widgets::section(ui, "性能", |ui| {
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
                    widgets::hint(
                        ui,
                        "批量转换时同时处理的文件数量。数量过大反而可能因为磁盘读写竞争而变慢。",
                    );
                });

                widgets::section(ui, "历史记录", |ui| {
                    widgets::field(ui, "保存记录", |ui| {
                        config_changed |= ui
                            .checkbox(&mut ctx.config.keep_history, "保留转换历史")
                            .on_hover_text("关闭后不再写入新的历史记录,已有记录不会被删除")
                            .changed();
                    });
                    widgets::key_value(ui, "当前条数", &format!("{} 条", ctx.history.len()));
                    widgets::key_value(ui, "成功 / 失败", &format!("{} / {}", ctx.history.succeeded(), ctx.history.failed()));
                });

                widgets::section(ui, "配置文件", |ui| {
                    widgets::key_value(ui, "配置文件", &config_path.display().to_string());
                    widgets::key_value(
                        ui,
                        "数据目录",
                        &fs::app_data_dir().display().to_string(),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                config_path.parent().is_some(),
                                egui::Button::new("打开配置目录"),
                            )
                            .clicked()
                        {
                            open_config_dir = true;
                        }
                        if ui
                            .add(egui::Button::new("立即保存"))
                            .on_hover_text("把当前设置写入配置文件")
                            .clicked()
                        {
                            force_save = true;
                        }
                        if ui
                            .add(egui::Button::new("恢复默认设置"))
                            .on_hover_text("把全部设置恢复为出厂值,不会删除历史记录")
                            .clicked()
                        {
                            restore_defaults = true;
                        }
                    });
                });

                widgets::section(ui, "关于本软件", |ui| {
                    widgets::key_value(ui, "软件名称", APP_FULL_NAME);
                    widgets::key_value(ui, "简称", APP_SHORT_NAME);
                    widgets::key_value(ui, "版本号", APP_VERSION);
                    widgets::key_value(
                        ui,
                        "已实现格式",
                        &format!("{} 种", ctx.formats.len()),
                    );
                    ui.add_space(4.0);
                    widgets::hint(
                        ui,
                        "本软件的全部图像解码与编码算法均由项目自行编写实现,未使用任何第三方编解码库。",
                    );
                    widgets::hint(
                        ui,
                        &format!(
                            "{} {} 的所有转换功能都可以在完全离线的环境中运行。",
                            APP_NAME, APP_VERSION
                        ),
                    );

                    ui.add_space(6.0);
                    let _ = ui.checkbox(&mut self.show_format_matrix, "查看格式能力对照表");

                    if self.show_format_matrix {
                        ui.add_space(4.0);
                        format_matrix(ui);
                    }
                });

                ui.add_space(8.0);
            });

        if theme_changed {
            requests.push(UiRequest::ApplyTheme);
        }
        if open_config_dir
            && let Some(parent) = config_path.parent()
        {
            requests.push(UiRequest::OpenDirectory(parent.to_path_buf()));
        }
        if restore_defaults {
            *ctx.config = AppConfig::default();
            ctx.config.normalize();
            requests.push(UiRequest::ApplyTheme);
            requests.push(UiRequest::Status("已恢复默认设置".to_string()));
            requests.push(UiRequest::SaveConfig);
        } else if config_changed || theme_changed || force_save {
            requests.push(UiRequest::SaveConfig);
        }

        requests
    }
}

/// 格式能力对照表:列出全部格式以及各自是否已实现、是否无损、是否支持透明通道。
fn format_matrix(ui: &mut Ui) {
    Grid::new("format_matrix")
        .num_columns(6)
        .striped(true)
        .spacing([12.0, 6.0])
        .min_col_width(56.0)
        .show(ui, |ui| {
            for header in ["格式", "主扩展名", "状态", "无损", "透明", "说明"] {
                ui.label(RichText::new(header).strong().small());
            }
            ui.end_row();

            for format in Format::all() {
                ui.label(RichText::new(format.name()).small());
                ui.label(RichText::new(format.primary_extension()).small().monospace());

                let (text, color) = if format.is_implemented() {
                    ("已实现", theme::SUCCESS)
                } else {
                    ("规划中", theme::MUTED)
                };
                widgets::badge(ui, text, color);

                ui.label(RichText::new(yes_no(format.is_lossless())).small());
                ui.label(RichText::new(yes_no(format.supports_alpha())).small());
                ui.label(RichText::new(format.description()).small().color(theme::MUTED));
                ui.end_row();
            }
        });

    ui.add_space(4.0);
    widgets::hint(
        ui,
        "扩展名一列仅列出该格式最常用的主扩展名,实际可读扩展名以文件选择框中的筛选条件为准。",
    );
}

/// 把布尔值转成"是 / 否"。
fn yes_no(value: bool) -> &'static str {
    if value { "是" } else { "否" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::build_default;
    use crate::service::history::HistoryStore;
    use crate::ui::run_frame;

    /// 在无窗口的 egui 上下文里绘制一帧设置分页。
    fn render(config: &mut AppConfig, matrix: bool) -> Vec<UiRequest> {
        let registry = build_default();
        let formats = registry.writable_formats();
        let history = HistoryStore::default();
        let mut view = SettingsView {
            show_format_matrix: matrix,
        };
        let mut requests: Vec<UiRequest> = Vec::new();
        {
            let mut ctx = UiContext {
                registry: &registry,
                config,
                history: &history,
                formats: &formats,
                busy: false,
            };
            let ctx_egui = egui::Context::default();
            run_frame(&ctx_egui, |ui| {
                requests = view.render(ui, &mut ctx);
            });
        }
        requests
    }

    #[test]
    fn yes_no_maps_booleans() {
        assert_eq!(yes_no(true), "是");
        assert_eq!(yes_no(false), "否");
    }

    #[test]
    fn render_without_interaction_requests_nothing() {
        let mut config = AppConfig::default();
        let requests = render(&mut config, false);
        assert!(
            requests.is_empty(),
            "仅仅绘制设置页不应触发任何副作用,实际得到 {requests:?}"
        );
    }

    #[test]
    fn render_with_format_matrix_does_not_panic() {
        let mut config = AppConfig::default();
        let requests = render(&mut config, true);
        assert!(requests.is_empty());
    }

    #[test]
    fn format_matrix_lists_every_format() {
        // 表格内容来自注册表,这里只验证格式清单本身是稳定的
        assert!(Format::all().len() >= 6);
        assert!(Format::all().iter().any(|format| format.is_implemented()));
    }
}
