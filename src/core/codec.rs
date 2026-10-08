//! 编解码器接口。
//!
//! 每种格式只需实现 [`Decoder`] 与 [`Encoder`] 两个 trait,再在 `codecs::build_default`
//! 中注册一行,即可被整个软件识别。界面层不关心任何具体格式的实现细节。

use crate::core::error::Result;
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;

/// 解码器:把文件字节还原为内存位图。
pub trait Decoder: Send + Sync {
    /// 该解码器负责的格式。
    fn format(&self) -> Format;

    /// 该格式关联的扩展名。
    fn extensions(&self) -> &'static [&'static str] {
        self.format().extensions()
    }

    /// 嗅探文件头,判断是否为本格式。
    ///
    /// 实现时只允许读取 `header` 中的内容,不得假设长度,数据不足时应返回 `false`。
    fn sniff(&self, header: &[u8]) -> bool;

    /// 解析完整文件数据。
    fn decode(&self, data: &[u8]) -> Result<Image>;
}

/// 编码器:把内存位图写为文件字节。
pub trait Encoder: Send + Sync {
    /// 该编码器负责的格式。
    fn format(&self) -> Format;

    /// 该格式关联的扩展名。
    fn extensions(&self) -> &'static [&'static str] {
        self.format().extensions()
    }

    /// 针对具体图像给出建议的输出扩展名。
    ///
    /// 多数格式的扩展名与内容无关,默认返回首选扩展名;Netpbm 这类"一个格式多种
    /// 子类型"的情况会按实际颜色模式返回 `pgm` / `ppm` / `pam`。
    fn extension_for(&self, _image: &Image) -> &'static str {
        self.format().primary_extension()
    }

    /// 编码为字节流。
    fn encode(&self, image: &Image, options: &EncodeOptions) -> Result<Vec<u8>>;
}
