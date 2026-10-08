//! 应用主体:窗口骨架、后台任务调度与请求执行。
//!
//! 四个分页的绘制代码不产生任何副作用,它们把要做的事写成 [`UiRequest`] 返回;
//! 所有带副作用的操作(启动任务、保存配置、打开资源管理器等)都集中在本模块的
//! [`App::execute`] 里。后台转换任务上报的事件也在本模块被翻译成界面状态与历史
//! 记录,这样界面就不必关心线程模型。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use egui::{Align, Layout, RichText, Ui};

use crate::codecs::build_default;
use crate::core::format::Format;
use crate::core::options::ConvertOptions;
use crate::core::registry::Registry;
use crate::core::{APP_FULL_NAME, APP_NAME, APP_VERSION};
use crate::service::config::AppConfig;
use crate::service::history::{HistoryEntry, HistoryStore, MAX_HISTORY_ENTRIES};
use crate::service::task::{TaskEvent, TaskRunner};
use crate::ui::batch_view::{BatchView, LogKind};
use crate::ui::convert_view::ConvertView;
use crate::ui::history_view::HistoryView;
use crate::ui::settings_view::SettingsView;
use crate::ui::{TaskKind, UiContext, UiRequest, View};
use crate::ui::theme;
use crate::util::fs;

/// 状态栏与任务列表的刷新间隔。任务运行期间按这个节奏请求重绘。
const REFRESH_INTERVAL: Duration = Duration::from_millis(100);

/// 命令行启动参数。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StartupOptions {
    /// 启动时要载入的文件,不存在的路径不会出现在这里。
    pub files: Vec<PathBuf>,
    /// 启动后直接显示的分页;`None` 表示使用默认分页。
    pub view: Option<View>,
    /// 载入文件后是否立刻开始转换,不再等待点击「开始」。
    pub auto_start: bool,
}

/// 解析命令行参数。
///
/// `--view <分页>` 或 `--view=<分页>` 指定启动后显示的分页,分页名可以写
/// `convert` / `batch` / `history` / `settings`,也可以直接写界面上的中文名称;
/// `--auto-start` 表示载入文件后立刻开始转换;其余参数一律当作待载入的文件路径。
/// 不存在的路径被忽略,无法识别的分页名称也不会影响启动。这样既能支持资源管理器
/// 里的「打开方式」,也方便从脚本直接进入指定的分页并跑完一批转换。
pub fn parse_startup_args<I, S>(args: I) -> StartupOptions
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut options = StartupOptions::default();
    let mut waiting_for_view = false;

    for arg in args {
        let text = arg.as_ref().to_string_lossy().into_owned();

        if waiting_for_view {
            options.view = View::from_key(&text);
            waiting_for_view = false;
        } else if text == "--view" {
            waiting_for_view = true;
        } else if text == "--auto-start" {
            options.auto_start = true;
        } else if let Some(name) = text.strip_prefix("--view=") {
            options.view = View::from_key(name);
        } else if !text.starts_with("--") {
            let path = PathBuf::from(arg.as_ref());
            if path.is_file() {
                options.files.push(path);
            }
        }
    }

    options
}

/// 应用主体。
pub struct App {
    /// 编解码器注册表,后台线程与界面共用同一份。
    registry: Arc<Registry>,
    /// 可写出的格式清单,启动时计算一次后复用。
    formats: Vec<Format>,
    /// 当前配置。
    config: AppConfig,
    /// 转换历史。
    history: HistoryStore,
    /// 当前显示的分页。
    view: View,
    /// 转换分页的状态。
    convert: ConvertView,
    /// 批量分页的状态。
    batch: BatchView,
    /// 历史分页的状态。
    history_view: HistoryView,
    /// 设置分页的状态。
    settings: SettingsView,
    /// 正在运行的后台任务。为 `None` 表示当前空闲。
    runner: Option<TaskRunner>,
    /// 后台任务由哪个分页发起,决定进度与结果回填到何处。
    task_kind: Option<TaskKind>,
    /// 状态栏文字。
    status: String,
    /// 状态栏文字是否表示出错。
    message_is_error: bool,
}

impl App {
    /// 用给定的组件构造应用主体。
    ///
    /// 与 [`App::new`] 分开是为了在测试中不依赖窗口环境。
    pub fn with_parts(registry: Arc<Registry>, config: AppConfig, history: HistoryStore) -> Self {
        let formats = registry.writable_formats();
        let status = format!("{APP_FULL_NAME} {APP_VERSION} 已就绪");
        Self {
            registry,
            formats,
            config,
            history,
            view: View::Convert,
            convert: ConvertView::new(),
            batch: BatchView::new(),
            history_view: HistoryView::new(),
            settings: SettingsView::new(),
            runner: None,
            task_kind: None,
            status,
            message_is_error: false,
        }
    }

    /// 启动时构造应用:读取配置与历史,套用主题。
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut config = AppConfig::load();
        config.normalize();

        let mut history = HistoryStore::load();
        history.retain_recent(MAX_HISTORY_ENTRIES);

        theme::install_system_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, config.theme);

        Self::with_parts(Arc::new(build_default()), config, history)
    }

    /// 收下从命令行传入的文件。
    ///
    /// 传入一个文件时进入「转换」页,一次传入多个文件时进入「批量」页;
    /// 不存在的路径会被忽略。这样在资源管理器里选中文件后「打开方式」选择本
    /// 软件,或者把文件拖到可执行文件上,都能直接开始工作。
    pub fn preload(&mut self, paths: Vec<PathBuf>) {
        let files: Vec<PathBuf> = paths.into_iter().filter(|path| path.is_file()).collect();

        match files.len() {
            0 => {}
            1 => {
                let name = fs::file_name(&files[0]);
                self.convert.add_inputs(files);
                self.view = View::Convert;
                self.status = format!("已载入 {name}");
                self.message_is_error = false;
            }
            _ => {
                let count = files.len();
                self.batch.add_inputs(files);
                self.view = View::Batch;
                self.status = format!("已载入 {count} 个文件,可以开始批量转换");
                self.message_is_error = false;
            }
        }
    }

    /// 直接切到指定分页。
    ///
    /// 供启动参数使用:命令行里显式指定的分页优先于 [`App::preload`] 按文件数量
    /// 选出的分页。
    pub fn show_view(&mut self, view: View) {
        self.view = view;
    }

    /// 立刻开始处理已经载入的文件。
    ///
    /// 供启动参数 `--auto-start` 使用:命令行里给了文件就直接开跑,省去一次点击,
    /// 方便从脚本或计划任务调用。载入到「转换」页的文件按单文件任务提交,载入到
    /// 「批量」页的文件按批处理提交。没有待转换的文件、或者已经有任务在跑时什么
    /// 都不做并返回 `false`。
    pub fn auto_start(&mut self) -> bool {
        if self.runner.is_some() {
            return false;
        }

        let workers = self.config.max_workers.max(1);
        let options = self.config.to_convert_options();

        if !self.convert.inputs.is_empty() {
            // 单文件走「转换」页的路径,结果要回填到转换分页。
            let files = self.convert.inputs.clone();
            self.start_batch(files, options, 1, TaskKind::Single);
            true
        } else if !self.batch.files.is_empty() {
            let files = self.batch.files.clone();
            self.batch.begin(files.len(), workers);
            self.start_batch(files, options, workers, TaskKind::Batch);
            true
        } else {
            false
        }
    }

    /// 取出后台任务事件并翻译成界面状态。
    ///
    /// 任务线程结束后再补一次 `drain`,避免最后几条事件因为时序原因被丢掉。
    fn pump_tasks(&mut self) {
        let Some(runner) = self.runner.take() else {
            return;
        };

        for event in runner.drain() {
            self.apply_event(event);
        }

        if runner.is_running() {
            self.runner = Some(runner);
        } else {
            for event in runner.drain() {
                self.apply_event(event);
            }
            self.task_kind = None;
            self.convert.busy = false;
        }
    }

    /// 处理一条后台任务事件。
    fn apply_event(&mut self, event: TaskEvent) {
        match event {
            TaskEvent::Started { total } => {
                if self.task_kind == Some(TaskKind::Batch) {
                    let workers = self.config.max_workers.max(1);
                    self.batch.begin(total, workers);
                }
            }
            TaskEvent::FileStarted { index, path } => {
                let position = index + 1;
                let total = self.active_total();
                self.status = format!("正在处理 {} ({position}/{total})", fs::file_name(&path));
                self.message_is_error = false;
                if self.task_kind == Some(TaskKind::Batch) {
                    self.batch.advance(index);
                }
            }
            TaskEvent::FileFinished { index, outcome } => {
                let outcome = *outcome;
                self.record(HistoryEntry::from_outcome(&outcome));
                self.status = outcome.summary();
                self.message_is_error = false;

                if self.task_kind == Some(TaskKind::Batch) {
                    self.batch.advance(index + 1);
                    let text = format!(
                        "{} 已转换为 {}",
                        fs::file_name(&outcome.input),
                        outcome.target_format.name()
                    );
                    self.batch.push_log(text, LogKind::Success);
                } else {
                    self.convert.report_success(outcome);
                }
            }
            TaskEvent::FileFailed {
                index,
                path,
                message,
            } => {
                let name = fs::file_name(&path);
                self.record(HistoryEntry::from_failure(
                    &path,
                    self.config.target_format,
                    &message,
                ));
                self.status = format!("{name} 转换失败:{message}");
                self.message_is_error = true;

                if self.task_kind == Some(TaskKind::Batch) {
                    self.batch.advance(index + 1);
                    let text = format!("{name} 转换失败:{message}");
                    self.batch.push_log(text, LogKind::Failure);
                } else {
                    self.convert.report_failure(format!("{name} 转换失败:{message}"));
                }
            }
            TaskEvent::Finished(summary) => {
                self.status = summary.summary_text();
                self.message_is_error = summary.failed > 0;
                if self.task_kind == Some(TaskKind::Batch) {
                    self.batch.finish(summary);
                }
                let _ = self.history.save();
            }
        }
    }

    /// 本次任务的文件总数,用于进度显示。
    fn active_total(&self) -> usize {
        match self.task_kind {
            Some(TaskKind::Batch) => self.batch.total,
            _ => self.convert.inputs.len().max(1),
        }
    }

    /// 记录一条历史。写入磁盘的动作留到整批任务结束时统一做,避免频繁 IO。
    fn record(&mut self, entry: HistoryEntry) {
        if !self.config.keep_history {
            return;
        }
        self.history.push(entry);
    }

    /// 执行分页提出的请求。
    fn execute(&mut self, requests: Vec<UiRequest>, ctx: &egui::Context) {
        for request in requests {
            match request {
                UiRequest::Status(text) => {
                    self.status = text;
                    self.message_is_error = false;
                }
                UiRequest::Error(text) => {
                    self.status = text;
                    self.message_is_error = true;
                }
                UiRequest::StartBatch {
                    files,
                    options,
                    workers,
                    kind,
                } => self.start_batch(files, *options, workers, kind),
                UiRequest::CancelBatch => {
                    if let Some(runner) = self.runner.as_ref() {
                        runner.cancel();
                        self.status = "已请求取消,正在等待当前文件结束".to_string();
                        self.message_is_error = false;
                    }
                }
                UiRequest::SaveConfig => {
                    if let Err(error) = self.config.save() {
                        self.status = format!("保存设置失败:{error}");
                        self.message_is_error = true;
                    }
                }
                UiRequest::ApplyTheme => theme::apply(ctx, self.config.theme),
                UiRequest::RevealFile(path) => {
                    if let Err(error) = fs::reveal_in_explorer(&path) {
                        self.status = format!("定位文件失败:{error}");
                        self.message_is_error = true;
                    }
                }
                UiRequest::OpenDirectory(dir) => {
                    if let Err(error) = fs::open_directory(&dir) {
                        self.status = format!("打开目录失败:{error}");
                        self.message_is_error = true;
                    }
                }
                UiRequest::SwitchTo(view) => self.view = view,
                UiRequest::ClearHistory => {
                    self.history.clear();
                    let _ = self.history.save();
                    self.status = "已清空转换历史".to_string();
                    self.message_is_error = false;
                }
            }
        }
    }

    /// 提交一批转换任务。
    fn start_batch(
        &mut self,
        files: Vec<PathBuf>,
        options: ConvertOptions,
        workers: usize,
        kind: TaskKind,
    ) {
        if files.is_empty() {
            self.status = "没有可转换的文件".to_string();
            self.message_is_error = true;
            return;
        }

        if self.runner.is_some() {
            self.status = "已有任务正在运行,请先取消或等待其结束".to_string();
            self.message_is_error = true;
            return;
        }

        if let Some(parent) = first_parent(&files) {
            self.config.last_input_dir = Some(parent.to_path_buf());
        }
        let _ = self.config.save();

        let count = files.len();
        let workers = workers.max(1);
        self.task_kind = Some(kind);
        self.convert.busy = kind == TaskKind::Single;
        self.status = format!("已提交 {count} 个文件,使用 {workers} 个线程");
        self.message_is_error = false;

        self.runner = Some(TaskRunner::spawn(
            files,
            Arc::clone(&self.registry),
            options,
            workers,
        ));
    }

    /// 处理拖放进窗口的文件:批量分页追加到列表,其它分页交给转换分页。
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }

        let count = dropped.len();
        if let Some(parent) = first_parent(&dropped) {
            self.config.last_input_dir = Some(parent.to_path_buf());
        }

        if self.view == View::Batch {
            self.batch.add_inputs(dropped);
        } else {
            self.view = View::Convert;
            if self.convert.inputs.is_empty() {
                self.convert.set_inputs(dropped);
            } else {
                self.convert.add_inputs(dropped);
            }
        }

        self.status = format!("已从拖放载入 {count} 个文件");
        self.message_is_error = false;
    }

    /// 内容区:按当前分页绘制。
    fn render_body(&mut self, ui: &mut Ui, requests: &mut Vec<UiRequest>, busy: bool) {
        let registry: &Registry = &self.registry;
        egui::CentralPanel::default().show(ui, |ui| {
            let mut ui_ctx = UiContext {
                registry,
                config: &mut self.config,
                history: &self.history,
                formats: &self.formats,
                busy,
            };

            let produced = match self.view {
                View::Convert => self.convert.render(ui, &mut ui_ctx),
                View::Batch => self.batch.render(ui, &mut ui_ctx),
                View::History => self.history_view.render(ui, &mut ui_ctx),
                View::Settings => self.settings.render(ui, &mut ui_ctx),
            };
            requests.extend(produced);
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.pump_tasks();
        self.handle_dropped_files(&ctx);

        let busy = self.runner.is_some();
        self.convert.busy = busy;

        let mut nav_target: Option<View> = None;

        egui::Panel::top("top_nav").show(ui, |ui| {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                ui.label(RichText::new(APP_NAME).strong().size(18.0));
                ui.label(RichText::new(APP_VERSION).small().weak());
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);
                for view in View::all() {
                    let response = ui
                        .selectable_label(self.view == view, view.title())
                        .on_hover_text(view.hint());
                    if response.clicked() {
                        nav_target = Some(view);
                    }
                }
            });
            ui.add_space(5.0);
        });

        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                let color = if self.message_is_error {
                    theme::DANGER
                } else {
                    theme::MUTED
                };
                ui.label(RichText::new(&self.status).small().color(color));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(6.0);
                    if busy {
                        ui.spinner();
                    }
                });
            });
            ui.add_space(3.0);
        });

        if let Some(view) = nav_target {
            self.view = view;
        }

        let mut requests = Vec::new();
        self.render_body(ui, &mut requests, busy);
        self.execute(requests, &ctx);

        if self.runner.is_some() {
            ctx.request_repaint_after(REFRESH_INTERVAL);
        }
    }
}

/// 生成程序图标:圆角蓝色底 + 白色字母 R。
///
/// 图标完全由代码计算,不依赖外部图片文件,因此在任何环境下都能构建。
pub fn build_icon() -> egui::IconData {
    const SIZE: u32 = 64;
    // 归一化坐标下:圆角半径与笔画半宽。
    const CORNER: f32 = 0.26;
    const STROKE: f32 = 0.075;

    // 用折线拼出字母 R:竖笔、上部横笔、右半圆、中横、右下斜笔。
    const STROKES: [((f32, f32), (f32, f32)); 7] = [
        ((-0.34, -0.62), (-0.34, 0.62)),
        ((-0.34, -0.62), (0.18, -0.62)),
        ((0.18, -0.62), (0.32, -0.54)),
        ((0.32, -0.54), (0.32, -0.16)),
        ((0.32, -0.16), (0.18, -0.08)),
        ((0.18, -0.08), (-0.34, -0.08)),
        ((-0.02, 0.00), (0.40, 0.62)),
    ];

    let half = SIZE as f32 / 2.0;
    let center = half - 0.5;
    let anti_alias = 2.0 / half;

    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let px = (x as f32 - center) / half;
            let py = (y as f32 - center) / half;

            let coverage =
                (0.5 - rounded_square_sdf(px, py, CORNER) / anti_alias).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }

            // 自上而下的蓝色渐变。
            let t = (py + 1.0) * 0.5;
            let base_red = lerp(0x4A as f32, 0x1E as f32, t);
            let base_green = lerp(0x8F as f32, 0x4E as f32, t);
            let base_blue = lerp(0xD8 as f32, 0x86 as f32, t);

            let mut ink = 0.0_f32;
            for (from, to) in STROKES {
                let distance = distance_to_segment(px, py, from, to);
                ink = ink.max(1.0 - (distance - STROKE).max(0.0) / anti_alias);
            }
            let ink = ink.clamp(0.0, 1.0);

            let index = ((y * SIZE + x) * 4) as usize;
            rgba[index] = lerp(base_red, 255.0, ink) as u8;
            rgba[index + 1] = lerp(base_green, 255.0, ink) as u8;
            rgba[index + 2] = lerp(base_blue, 255.0, ink) as u8;
            rgba[index + 3] = (coverage * 255.0) as u8;
        }
    }

    egui::IconData {
        rgba,
        width: SIZE,
        height: SIZE,
    }
}

/// 圆角矩形的有符号距离,负值表示落在图形内部。
fn rounded_square_sdf(px: f32, py: f32, corner: f32) -> f32 {
    let extent = 1.0 - corner;
    let dx = px.abs() - extent;
    let dy = py.abs() - extent;
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    let inside = dx.max(dy).min(0.0);
    outside + inside - corner
}

/// 点到线段的最短距离。
fn distance_to_segment(px: f32, py: f32, from: (f32, f32), to: (f32, f32)) -> f32 {
    let (ax, ay) = from;
    let (bx, by) = to;
    let dx = bx - ax;
    let dy = by - ay;
    let length_squared = dx * dx + dy * dy;

    let t = if length_squared <= f32::EPSILON {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0)
    };

    let nearest_x = ax + t * dx;
    let nearest_y = ay + t * dy;
    ((px - nearest_x).powi(2) + (py - nearest_y).powi(2)).sqrt()
}

/// 线性插值。
fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// 找出文件集中第一个文件的父目录,用作下次打开对话框的起始位置。
pub fn first_parent(files: &[PathBuf]) -> Option<&Path> {
    files.first().and_then(|path| path.parent())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::service::task::TaskSummary;

    fn build_app() -> App {
        let mut config = AppConfig::default();
        config.normalize();
        App::with_parts(
            Arc::new(build_default()),
            config,
            HistoryStore::default(),
        )
    }

    #[test]
    fn icon_has_expected_shape_and_colors() {
        let icon = build_icon();
        assert_eq!(icon.width, 64);
        assert_eq!(icon.height, 64);
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);

        let pixel = |x: u32, y: u32| {
            let index = ((y * icon.width + x) * 4) as usize;
            (
                icon.rgba[index],
                icon.rgba[index + 1],
                icon.rgba[index + 2],
                icon.rgba[index + 3],
            )
        };

        // 四角在圆角之外,应当完全透明;中心一定不透明。
        assert_eq!(pixel(0, 0).3, 0);
        assert_eq!(pixel(63, 0).3, 0);
        assert_eq!(pixel(0, 63).3, 0);
        assert_eq!(pixel(63, 63).3, 0);
        assert_eq!(pixel(32, 32).3, 255);

        // 底色是蓝色系:蓝色分量应当明显大于红色分量。
        let (red, _, blue, _) = pixel(6, 6);
        assert!(blue > red, "图标底色应当偏蓝:red={red} blue={blue}");

        // 字形是白色:至少要存在接近纯白的像素。
        assert!(icon.rgba.as_chunks::<4>().0.iter().any(|pixel| {
            pixel[0] > 240 && pixel[1] > 240 && pixel[2] > 240 && pixel[3] > 200
        }));
    }

    #[test]
    fn rounded_square_sdf_is_negative_inside() {
        assert!(rounded_square_sdf(0.0, 0.0, 0.2) < 0.0);
        assert!(rounded_square_sdf(0.5, 0.5, 0.2) < 0.0);
        assert!(rounded_square_sdf(1.2, 0.0, 0.2) > 0.0);
        assert!(rounded_square_sdf(1.0, 1.0, 0.2) > 0.0);
    }

    #[test]
    fn distance_to_degenerate_segment_is_point_distance() {
        let distance = distance_to_segment(3.0, 4.0, (0.0, 0.0), (0.0, 0.0));
        assert!((distance - 5.0).abs() < 1e-6, "distance={distance}");
    }

    #[test]
    fn distance_is_clamped_to_the_segment() {
        // 位于线段延长线上,最近点应落在端点 (1, 0)。
        let distance = distance_to_segment(4.0, 0.0, (0.0, 0.0), (1.0, 0.0));
        assert!((distance - 3.0).abs() < 1e-6, "distance={distance}");
    }

    #[test]
    fn lerp_interpolates_between_endpoints() {
        assert_eq!(lerp(0.0, 10.0, 0.0), 0.0);
        assert_eq!(lerp(0.0, 10.0, 1.0), 10.0);
        assert!((lerp(0.0, 10.0, 0.5) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn new_app_starts_on_the_convert_view() {
        let app = build_app();
        assert_eq!(app.view, View::Convert);
        assert!(app.runner.is_none());
        assert!(!app.formats.is_empty());
        assert!(!app.status.is_empty());
    }

    #[test]
    fn preload_ignores_paths_that_do_not_exist() {
        let mut app = build_app();
        app.preload(vec![PathBuf::from("Z:/definitely/missing/input.bmp")]);
        assert_eq!(app.view, View::Convert);
        assert!(app.convert.inputs.is_empty());
        assert!(app.batch.files.is_empty());
    }

    #[test]
    fn preload_puts_one_file_on_the_convert_view() {
        let dir = fs::temp_dir("preload-one");
        let input = dir.join("only.bmp");
        std::fs::write(&input, b"BM").unwrap();

        let mut app = build_app();
        app.preload(vec![input]);
        assert_eq!(app.view, View::Convert);
        assert_eq!(app.convert.inputs.len(), 1);
        assert!(app.batch.files.is_empty());
        assert!(!app.message_is_error);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preload_puts_several_files_on_the_batch_view() {
        let dir = fs::temp_dir("preload-many");
        let mut paths = Vec::new();
        for index in 0..3 {
            let path = dir.join(format!("shot-{index}.bmp"));
            std::fs::write(&path, b"BM").unwrap();
            paths.push(path);
        }

        let mut app = build_app();
        app.preload(paths);
        assert_eq!(app.view, View::Batch);
        assert_eq!(app.batch.files.len(), 3);
        assert_eq!(app.batch.total, 3);
        assert!(app.convert.inputs.is_empty());
        assert!(app.status.contains('3'), "状态栏应当提示文件数:{}", app.status);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_startup_args_reads_view_and_existing_files() {
        let dir = fs::temp_dir("startup-args");
        let good = dir.join("a.bmp");
        std::fs::write(&good, b"BM").unwrap();
        let missing = dir.join("nope.bmp");

        let options = parse_startup_args([
            "--view".to_string(),
            "history".to_string(),
            missing.to_string_lossy().into_owned(),
            good.to_string_lossy().into_owned(),
        ]);

        assert_eq!(options.view, Some(View::History));
        assert_eq!(options.files, vec![good]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_startup_args_accepts_inline_form_and_chinese_names() {
        assert_eq!(
            parse_startup_args(["--view=设置".to_string()]).view,
            Some(View::Settings)
        );
        assert_eq!(
            parse_startup_args(["--view=batch".to_string()]).view,
            Some(View::Batch)
        );
    }

    #[test]
    fn parse_startup_args_ignores_unknown_switches() {
        let options = parse_startup_args(["--nope".to_string(), "--view=nowhere".to_string()]);
        assert_eq!(options.view, None);
        assert!(options.files.is_empty());
        assert!(!options.auto_start);
    }

    #[test]
    fn parse_startup_args_reads_auto_start() {
        assert!(parse_startup_args(["--auto-start".to_string()]).auto_start);
        assert!(!parse_startup_args(["--view=batch".to_string()]).auto_start);
    }

    #[test]
    fn auto_start_does_nothing_without_pending_files() {
        let mut app = build_app();
        assert!(!app.auto_start());
        assert!(app.runner.is_none());
    }

    #[test]
    fn auto_start_launches_the_batch() {
        let dir = fs::temp_dir("auto-start");
        let mut paths = Vec::new();
        for index in 0..3 {
            let path = dir.join(format!("auto-{index}.bmp"));
            std::fs::write(&path, b"BM").unwrap();
            paths.push(path);
        }

        let mut app = build_app();
        app.preload(paths);
        assert!(app.auto_start());
        assert_eq!(app.task_kind, Some(TaskKind::Batch));
        assert_eq!(app.batch.total, 3);
        assert!(app.runner.is_some());

        if let Some(runner) = app.runner.take() {
            runner.cancel();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn auto_start_launches_the_single_conversion() {
        let dir = fs::temp_dir("auto-start-single");
        let path = dir.join("only.bmp");
        std::fs::write(&path, b"BM").unwrap();

        let mut app = build_app();
        app.preload(vec![path]);
        assert_eq!(app.view, View::Convert);
        assert!(app.auto_start());
        assert_eq!(app.task_kind, Some(TaskKind::Single));
        assert!(app.convert.busy);

        if let Some(runner) = app.runner.take() {
            runner.cancel();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn show_view_overrides_the_view_chosen_by_preload() {
        let dir = fs::temp_dir("show-view");
        let input = dir.join("only.bmp");
        std::fs::write(&input, b"BM").unwrap();

        let mut app = build_app();
        app.preload(vec![input]);
        assert_eq!(app.view, View::Convert);

        app.show_view(View::Settings);
        assert_eq!(app.view, View::Settings);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn execute_switches_view_without_side_effects() {
        let mut app = build_app();
        let ctx = egui::Context::default();

        app.execute(vec![UiRequest::SwitchTo(View::Settings)], &ctx);

        assert_eq!(app.view, View::Settings);
        assert!(!app.message_is_error);
    }

    #[test]
    fn execute_reports_errors_on_the_status_bar() {
        let mut app = build_app();
        let ctx = egui::Context::default();

        app.execute(vec![UiRequest::Error("读取失败".to_string())], &ctx);
        assert!(app.message_is_error);
        assert_eq!(app.status, "读取失败");

        app.execute(vec![UiRequest::Status("读取成功".to_string())], &ctx);
        assert!(!app.message_is_error);
        assert_eq!(app.status, "读取成功");
    }

    #[test]
    fn execute_clears_history() {
        let mut app = build_app();
        let ctx = egui::Context::default();
        app.history.push(HistoryEntry::from_failure(
            Path::new("C:/tmp/missing.bmp"),
            Format::Png,
            "无法识别的文件格式",
        ));
        assert_eq!(app.history.len(), 1);

        app.execute(vec![UiRequest::ClearHistory], &ctx);

        assert!(app.history.is_empty());
    }

    #[test]
    fn batch_events_track_progress_and_summary() {
        let mut app = build_app();
        app.task_kind = Some(TaskKind::Batch);

        app.apply_event(TaskEvent::Started { total: 4 });
        assert_eq!(app.batch.total, 4);
        assert_eq!(app.batch.processed, 0);

        app.apply_event(TaskEvent::FileStarted {
            index: 1,
            path: PathBuf::from("C:/tmp/second.bmp"),
        });
        assert_eq!(app.batch.processed, 1);
        assert!(app.status.contains("2/4"), "status={}", app.status);

        let summary = TaskSummary {
            total: 4,
            succeeded: 3,
            failed: 1,
            cancelled: false,
            elapsed: Duration::from_millis(250),
        };
        let expected = summary.summary_text();
        app.apply_event(TaskEvent::Finished(summary));

        assert_eq!(app.status, expected);
        assert!(app.message_is_error);
        assert!(app.batch.summary.is_some());
    }

    #[test]
    fn failure_events_are_recorded_in_history() {
        let mut app = build_app();
        app.task_kind = Some(TaskKind::Batch);

        app.apply_event(TaskEvent::FileFailed {
            index: 0,
            path: PathBuf::from("C:/tmp/broken.bmp"),
            message: "文件已损坏".to_string(),
        });

        assert_eq!(app.history.len(), 1);
        assert_eq!(app.history.failed(), 1);
        assert!(app.message_is_error);
        assert_eq!(app.batch.log.len(), 1);
    }

    #[test]
    fn failure_events_keep_history_empty_when_disabled() {
        let mut app = build_app();
        app.config.keep_history = false;
        app.task_kind = Some(TaskKind::Batch);

        app.apply_event(TaskEvent::FileFailed {
            index: 0,
            path: PathBuf::from("C:/tmp/broken.bmp"),
            message: "文件已损坏".to_string(),
        });

        assert!(app.history.is_empty());
    }

    #[test]
    fn single_file_events_fill_the_convert_view() {
        let mut app = build_app();
        app.task_kind = Some(TaskKind::Single);

        app.apply_event(TaskEvent::FileFailed {
            index: 0,
            path: PathBuf::from("C:/tmp/broken.bmp"),
            message: "文件已损坏".to_string(),
        });

        assert!(app.convert.error.is_some());
        assert!(!app.convert.busy);
    }

    #[test]
    fn starting_an_empty_batch_is_rejected() {
        let mut app = build_app();

        app.start_batch(
            Vec::new(),
            app.config.to_convert_options(),
            2,
            TaskKind::Batch,
        );

        assert!(app.runner.is_none());
        assert!(app.message_is_error);
        assert!(app.status.contains("没有可转换的文件"));
    }

    #[test]
    fn first_parent_finds_the_directory() {
        let files = vec![PathBuf::from("C:/tmp/a.bmp")];
        assert_eq!(first_parent(&files), Some(Path::new("C:/tmp")));
        assert_eq!(first_parent(&[]), None);
    }
}
