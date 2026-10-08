//! 核心层模块声明与全局常量。
//!
//! 核心层不依赖界面框架,只描述"图像是什么"以及"如何编解码",因此可以脱离
//! GUI 单独进行单元测试。

pub mod codec;
pub mod error;
pub mod format;
pub mod image;
pub mod options;
pub mod pixel;
pub mod registry;

/// 软件名称,用于窗口标题与日志。
pub const APP_NAME: &str = "R格式转换器";

/// 软件全称,软著登记与源码文档页眉使用。
pub const APP_FULL_NAME: &str = "R格式转换器软件";

/// 软件简称。
pub const APP_SHORT_NAME: &str = "R转换器";

/// 版本号,与软著登记材料保持一致。
pub const APP_VERSION: &str = "V1.0";

/// 窗口标题,形如 `R格式转换器 V1.0`。
pub fn window_title() -> String {
    format!("{APP_NAME} {APP_VERSION}")
}
