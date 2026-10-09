//! 图形界面层。


pub mod batch_view;
pub mod convert_view;
pub mod history_view;
pub mod settings_view;
pub mod theme;
pub mod widgets;

use std::path::PathBuf;

use crate::core::format::Format;
use crate::core::options::ConvertOptions;
use crate::core::registry::Registry;
use crate::service::config::AppConfig;
use crate::service::history::HistoryStore;

/// 主窗口分页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// 单文件转换。
    Convert,
    /// 多文件批量转换。
    Batch,
    /// 转换历史。
    History,
    /// 软件设置与说明。
    Settings,
}

impl View {
    /// 全部分页,顺序与顶部导航一致。
    pub fn all() -> [View; 4] {
        [View::Convert, View::Batch, View::History, View::Settings]
    }

    /// 导航栏显示的名称。
    pub fn title(self) -> &'static str {
        match self {
            View::Convert => "转换",
            View::Batch => "批量",
            View::History => "历史",
            View::Settings => "设置",
        }
    }

    /// 悬停提示,解释该分页的用途。
    pub fn hint(self) -> &'static str {
        match self {
            View::Convert => "把一个文件转换成目标格式,可同时调整尺寸与颜色",
            View::Batch => "一次转换多个文件,使用同一套参数",
            View::History => "查看历次转换的结果与耗时",
            View::Settings => "主题、并发数等软件级设置与格式说明",
        }
    }

    /// 命令行中使用的稳定标识,与界面语言无关。
    pub fn key(self) -> &'static str {
        match self {
            View::Convert => "convert",
            View::Batch => "batch",
            View::History => "history",
            View::Settings => "settings",
        }
    }

    /// 由命令行标识或界面名称解析分页,大小写不敏感,两侧空白会被忽略。
    ///
    /// 返回 `None` 表示无法识别,调用方应当退回默认分页。
    pub fn from_key(text: &str) -> Option<View> {
        let trimmed = text.trim();
        View::all()
            .into_iter()
            .find(|view| trimmed.eq_ignore_ascii_case(view.key()) || trimmed == view.title())
    }
}

/// 正在运行的转换任务类型,决定进度与结果回填到哪个分页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// 转换分页发起的单文件任务。
    Single,
    /// 批量分页发起的多文件任务。
    Batch,
}

/// 各分页共享的上下文。
///
/// 注册表、历史与可写格式列表都是只读的,转换参数则直接读写配置对象,这样界面上的
/// 选择可以立即被持久化,重启软件后仍然保留。
pub struct UiContext<'a> {
    /// 编解码器注册表,提供格式清单与能力查询。
    pub registry: &'a Registry,
    /// 当前配置,界面可直接修改。
    pub config: &'a mut AppConfig,
    /// 历史记录,只读。
    pub history: &'a HistoryStore,
    /// 可写出的格式列表,由注册表计算一次后复用。
    pub formats: &'a [Format],
    /// 是否有任务正在后台运行。
    pub busy: bool,
}

impl UiContext<'_> {
    /// 取默认的目标格式:配置里指定的格式,若不可写出则退回到第一个可写格式。
    pub fn effective_target(&self) -> Format {
        if self.registry.is_supported(self.config.target_format) {
            self.config.target_format
        } else {
            self.formats
                .first()
                .copied()
                .unwrap_or(self.config.target_format)
        }
    }
}

/// 分页向主程序发出的请求。
#[derive(Debug)]
pub enum UiRequest {
    /// 在状态栏显示一条普通信息。
    Status(String),
    /// 在状态栏显示一条错误信息。
    Error(String),
    /// 提交一批转换任务。
    StartBatch {
        /// 待转换的文件。
        files: Vec<PathBuf>,
        /// 完整的转换参数。
        options: Box<ConvertOptions>,
        /// 并发线程数。
        workers: usize,
        /// 任务来源分页。
        kind: TaskKind,
    },
    /// 请求取消正在运行的任务。
    CancelBatch,
    /// 把当前配置写回磁盘。
    SaveConfig,
    /// 重新载入界面主题(主题被修改后调用)。
    ApplyTheme,
    /// 在资源管理器中定位文件。
    RevealFile(PathBuf),
    /// 用系统默认程序打开目录。
    OpenDirectory(PathBuf),
    /// 切换到指定分页。
    SwitchTo(View),
    /// 清空历史记录。
    ClearHistory,
}

/// 在无窗口环境下跑一帧界面代码,并取回闭包的返回值。
///
/// `egui` 会为首帧的字体等资源生成纹理增量,这些增量必须显式丢弃,否则
/// `FullOutput` 析构时会触发断言。单元测试用不到它们,统一在这里清掉。
#[cfg(test)]
pub(crate) fn run_frame<R>(ctx: &egui::Context, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let mut add = Some(add);
    let mut result: Option<R> = None;
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        if let Some(add) = add.take() {
            result = Some(add(ui));
        }
    });
    output.textures_delta.clear();
    result.expect("界面闭包应当执行一次")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_titles_are_unique_and_non_empty() {
        let mut titles: Vec<&str> = View::all().iter().map(|view| view.title()).collect();
        assert!(titles.iter().all(|title| !title.is_empty()));
        titles.sort_unstable();
        titles.dedup();
        assert_eq!(titles.len(), View::all().len());
    }

    #[test]
    fn every_view_has_a_hint() {
        for view in View::all() {
            assert!(!view.hint().is_empty(), "{view:?} 缺少用途说明");
        }
    }

    #[test]
    fn view_keys_round_trip_through_from_key() {
        for view in View::all() {
            assert_eq!(View::from_key(view.key()), Some(view));
            assert_eq!(View::from_key(view.title()), Some(view));
            assert_eq!(View::from_key(&view.key().to_ascii_uppercase()), Some(view));
            assert_eq!(View::from_key(&format!("  {}  ", view.key())), Some(view));
        }
        assert_eq!(View::from_key("nowhere"), None);
        assert_eq!(View::from_key(""), None);
    }
}
