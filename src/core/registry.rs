//! 格式注册表。
//!
//! 注册表是核心层对外的统一入口:界面只通过它查询"支持哪些格式",转换管线只
//! 通过它完成"识别格式"与"解码 / 编码"。注册表本身不保存任何编解码状态,真正的
//! 读写由 [`crate::codecs`] 委托给 `image` 库完成,因此这里是纯粹的格式清单与
//! 参数校验。

use crate::codecs;
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;

/// 软件当前支持的格式集合。
pub struct Registry {
    formats: Vec<Format>,
}

impl Registry {
    /// 用给定的格式列表建立注册表,列表顺序同时是界面上的展示顺序。
    pub fn new(formats: Vec<Format>) -> Self {
        Self { formats }
    }

    /// 注册表中已启用的格式。
    pub fn formats(&self) -> &[Format] {
        &self.formats
    }

    /// 可作为输入读取的格式。
    pub fn readable_formats(&self) -> Vec<Format> {
        self.formats.clone()
    }

    /// 可作为目标写出的格式。
    pub fn writable_formats(&self) -> Vec<Format> {
        self.formats.clone()
    }

    /// 已启用的格式数量,用于启动日志与界面"关于"信息。
    pub fn codec_count(&self) -> usize {
        self.formats.len()
    }

    /// 格式是否在支持列表中。
    pub fn is_supported(&self, format: Format) -> bool {
        self.formats.contains(&format)
    }

    /// 按文件头识别格式,无法识别或未启用时返回 `None`。
    ///
    /// TGA 没有固定魔数,这里识别不出来,调用方需要按扩展名或逐个尝试兜底。
    pub fn detect(&self, data: &[u8]) -> Option<Format> {
        codecs::detect(data).filter(|format| self.is_supported(*format))
    }

    /// 按文件头自动识别并解码。
    pub fn decode(&self, data: &[u8]) -> Result<Image> {
        match self.detect(data) {
            Some(format) => self.decode_as(data, format),
            None => Err(ConvertError::UnknownFormat),
        }
    }

    /// 按指定格式解码。
    pub fn decode_as(&self, data: &[u8], format: Format) -> Result<Image> {
        self.check(format)?;
        codecs::decode(data, format)
    }

    /// 按指定格式编码。
    pub fn encode(
        &self,
        image: &Image,
        format: Format,
        options: &EncodeOptions,
    ) -> Result<Vec<u8>> {
        self.check(format)?;
        codecs::encode(image, format, options)
    }

    /// 目标格式针对该图像建议的输出扩展名。
    pub fn output_extension(&self, image: &Image, format: Format) -> &'static str {
        codecs::output_extension(image, format)
    }

    /// 校验格式是否可用,给出与旧版一致的中文提示。
    fn check(&self, format: Format) -> Result<()> {
        if !self.is_supported(format) {
            return Err(ConvertError::UnsupportedFormat(format.name().to_string()));
        }
        if !format.is_implemented() {
            return Err(ConvertError::unsupported(format!(
                "{}的编解码功能尚未实现,请选择其它目标格式",
                format.name()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::build_default;
    use crate::core::color::ColorType;

    #[test]
    fn default_registry_lists_every_implemented_format() {
        let registry = build_default();
        for format in Format::all() {
            assert_eq!(
                registry.is_supported(*format),
                format.is_implemented(),
                "{} 的启用状态与实现状态不一致",
                format.id()
            );
        }
        assert_eq!(registry.codec_count(), 9);
    }

    #[test]
    fn unregistered_format_reports_unsupported() {
        let registry = Registry::new(vec![Format::Png]);
        let image = crate::core::image::to_color(
            &Image::ImageRgba8(image::RgbaImage::new(1, 1)),
            ColorType::Gray8,
        );
        let err = registry
            .encode(&image, Format::Gif, &EncodeOptions::default())
            .unwrap_err();
        assert!(matches!(err, ConvertError::UnsupportedFormat(_)));
    }

    #[test]
    fn implemented_format_passes_check() {
        let registry = build_default();
        let image = Image::ImageRgba8(image::RgbaImage::new(1, 1));
        assert!(registry.output_extension(&image, Format::Gif) == "gif");
        assert!(registry.is_supported(Format::Gif));
    }

    #[test]
    fn detect_ignores_formats_that_are_not_enabled() {
        let registry = Registry::new(vec![Format::Png]);
        let gif = b"GIF89a\x01\x00\x01\x00\x00\x00\x00;";
        assert!(codecs::detect(gif).is_some());
        assert!(registry.detect(gif).is_none());

        let default_reg = build_default();
        assert_eq!(default_reg.detect(gif), Some(Format::Gif));
    }
}
