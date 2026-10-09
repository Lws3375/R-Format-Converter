//! 包含软件的全部业务逻辑:核心数据模型、格式编解码、转换管线、后台任务
//! 调度与界面实现。二进制入口 `main.rs` 只负责初始化日志并启动窗口。
//!
//! 模块划分:
//! - [`core`] — 不依赖界面框架的基础类型(位图、格式、错误、转换选项)
//! - [`codecs`] — 基于 `image` 库的格式识别与编解码
//! - [`convert`] — 图像变换与转换管线
//! - [`service`] — 后台任务调度、配置与历史记录持久化
//! - [`ui`] — 基于 egui 的图形界面
//! - [`app`] — 窗口骨架、任务调度与界面请求的执行
//! - [`util`] — 文件与日志等通用工具

pub mod app;
pub mod codecs;
pub mod convert;
pub mod core;
pub mod service;
pub mod ui;
pub mod util;

pub use core::{APP_FULL_NAME, APP_NAME, APP_SHORT_NAME, APP_VERSION};
