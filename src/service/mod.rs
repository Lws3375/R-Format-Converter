//! 后台服务层。
//!
//! 界面层只负责收集用户操作,真正的重活(批量转换、配置读写、历史记录维护)都在
//! 这一层完成。这样界面在转换大图时不会卡住,也便于单独测试业务逻辑。
//!
//! * [`task`] — 基于标准库线程与通道的批量转换调度;
//! * [`config`] — 用户配置的读写;
//! * [`history`] — 转换历史的读写与统计。

pub mod config;
pub mod history;
pub mod task;

pub use config::{AppConfig, ThemeMode, CONFIG_FILE};
pub use history::{HistoryEntry, HistoryStore, HISTORY_FILE, MAX_HISTORY_ENTRIES};
pub use task::{TaskEvent, TaskRunner, TaskSummary};
