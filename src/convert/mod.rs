//! 转换管线。
//!
//! 本模块把"读文件 → 识别格式 → 解码 → 变换 → 编码 → 写文件"这条流程封装成
//! 一个不依赖界面的纯逻辑层,界面层与后台任务层都只调用这里的函数。
//!
//! 拆分为两个子模块:
//!
//! * [`transform`] — 只负责内存位图的几何与色彩变换;
//! * [`pipeline`] — 负责文件级的完整转换流程与结果统计。

pub mod pipeline;
pub mod transform;

pub use pipeline::{convert_bytes, convert_file, ConversionOutcome};
pub use transform::apply_transforms;
