//! 编解码器注册表。
//!
//! 注册表在启动时由 `codecs::build_default` 装配一次,之后只读使用。查找方式有两种:
//! 按格式精确匹配,或按文件头自动嗅探。

use crate::core::codec::{Decoder, Encoder};
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;

/// 嗅探文件头时需要提供的字节数。
pub const SNIFF_LEN: usize = 32;

/// 解码器与编码器的集合。
#[derive(Default)]
pub struct Registry {
    decoders: Vec<Box<dyn Decoder>>,
    encoders: Vec<Box<dyn Encoder>>,
}

impl Registry {
    /// 创建空注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个解码器。同一格式重复注册时以先注册者为准。
    pub fn register_decoder(&mut self, decoder: Box<dyn Decoder>) {
        let format = decoder.format();
        if self.decoder(format).is_none() {
            self.decoders.push(decoder);
        }
    }

    /// 注册一个编码器。同一格式重复注册时以先注册者为准。
    pub fn register_encoder(&mut self, encoder: Box<dyn Encoder>) {
        let format = encoder.format();
        if self.encoder(format).is_none() {
            self.encoders.push(encoder);
        }
    }

    /// 按格式查找解码器。
    pub fn decoder(&self, format: Format) -> Option<&dyn Decoder> {
        self.decoders
            .iter()
            .find(|decoder| decoder.format() == format)
            .map(|decoder| decoder.as_ref())
    }

    /// 按格式查找编码器。
    pub fn encoder(&self, format: Format) -> Option<&dyn Encoder> {
        self.encoders
            .iter()
            .find(|encoder| encoder.format() == format)
            .map(|encoder| encoder.as_ref())
    }

    /// 按文件头自动识别格式。
    pub fn detect(&self, data: &[u8]) -> Option<&dyn Decoder> {
        let header = &data[..data.len().min(SNIFF_LEN)];
        self.decoders
            .iter()
            .find(|decoder| decoder.sniff(header))
            .map(|decoder| decoder.as_ref())
    }

    /// 自动识别并解码。
    pub fn decode(&self, data: &[u8]) -> Result<Image> {
        let decoder = self.detect(data).ok_or(ConvertError::UnknownFormat)?;
        decoder.decode(data)
    }

    /// 按指定格式解码。
    pub fn decode_as(&self, data: &[u8], format: Format) -> Result<Image> {
        let decoder = self
            .decoder(format)
            .ok_or_else(|| ConvertError::UnsupportedFormat(format.name().to_string()))?;
        decoder.decode(data)
    }

    /// 按指定格式编码。
    pub fn encode(
        &self,
        image: &Image,
        format: Format,
        options: &EncodeOptions,
    ) -> Result<Vec<u8>> {
        let encoder = self
            .encoder(format)
            .ok_or_else(|| ConvertError::UnsupportedFormat(format.name().to_string()))?;
        encoder.encode(image, options)
    }

    /// 可读取的格式列表。
    pub fn readable_formats(&self) -> Vec<Format> {
        self.decoders.iter().map(|decoder| decoder.format()).collect()
    }

    /// 可写出的格式列表。
    pub fn writable_formats(&self) -> Vec<Format> {
        self.encoders.iter().map(|encoder| encoder.format()).collect()
    }

    /// 该格式是否既可读又可写。
    pub fn is_supported(&self, format: Format) -> bool {
        self.decoder(format).is_some() && self.encoder(format).is_some()
    }

    /// 目标格式针对该图像建议的输出扩展名。
    pub fn output_extension(&self, image: &Image, format: Format) -> &'static str {
        self.encoder(format)
            .map(|encoder| encoder.extension_for(image))
            .unwrap_or_else(|| format.primary_extension())
    }

    /// 已注册的编解码器数量,用于启动日志与界面"关于"信息。
    pub fn codec_count(&self) -> usize {
        self.decoders.len() + self.encoders.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pixel::{ColorType, Rgba};

    /// 一个仅用于测试的假编解码器,读写 1x1 的灰度图。
    struct FakeCodec;

    const FAKE_MAGIC: [u8; 4] = [0xFA, 0xCE, 0x00, 0x01];

    impl Decoder for FakeCodec {
        fn format(&self) -> Format {
            Format::Bmp
        }

        fn sniff(&self, header: &[u8]) -> bool {
            header.starts_with(&FAKE_MAGIC)
        }

        fn decode(&self, data: &[u8]) -> Result<Image> {
            let value = data.get(4).copied().ok_or(ConvertError::UnknownFormat)?;
            Image::new(1, 1, ColorType::Gray8, vec![value])
        }
    }

    impl Encoder for FakeCodec {
        fn format(&self) -> Format {
            Format::Bmp
        }

        fn encode(&self, image: &Image, _options: &EncodeOptions) -> Result<Vec<u8>> {
            let mut out = FAKE_MAGIC.to_vec();
            out.push(image.get_pixel(0, 0).r);
            Ok(out)
        }
    }

    fn registry() -> Registry {
        let mut registry = Registry::new();
        registry.register_decoder(Box::new(FakeCodec));
        registry.register_encoder(Box::new(FakeCodec));
        registry
    }

    #[test]
    fn detects_registered_magic() {
        let registry = registry();
        let data = [0xFA, 0xCE, 0x00, 0x01, 0x7F];
        let decoder = registry.detect(&data).expect("应识别出假格式");
        assert_eq!(decoder.format(), Format::Bmp);
        assert!(registry.detect(&[0x00, 0x01]).is_none());
    }

    #[test]
    fn encode_then_decode_roundtrip() {
        let registry = registry();
        let image = Image::new(1, 1, ColorType::Gray8, vec![123]).unwrap();
        let bytes = registry
            .encode(&image, Format::Bmp, &EncodeOptions::default())
            .unwrap();
        let decoded = registry.decode(&bytes).unwrap();
        assert_eq!(decoded.get_pixel(0, 0), Rgba::from_gray(123));
    }

    #[test]
    fn unregistered_format_reports_unsupported() {
        let registry = registry();
        let err = registry
            .encode(
                &Image::new(1, 1, ColorType::Gray8, vec![0]).unwrap(),
                Format::Qoi,
                &EncodeOptions::default(),
            )
            .unwrap_err();
        assert!(matches!(err, ConvertError::UnsupportedFormat(_)));
    }

    #[test]
    fn duplicate_registration_is_ignored() {
        let mut registry = Registry::new();
        registry.register_decoder(Box::new(FakeCodec));
        registry.register_decoder(Box::new(FakeCodec));
        assert_eq!(registry.readable_formats().len(), 1);
    }

    #[test]
    fn unsupported_format_is_reported_for_missing_codec() {
        let registry = registry();
        assert!(registry.is_supported(Format::Bmp));
        assert!(!registry.is_supported(Format::Tga));
        assert_eq!(registry.codec_count(), 2);
    }

    #[test]
    fn output_extension_defaults_to_primary() {
        let registry = registry();
        let image = Image::new(1, 1, ColorType::Gray8, vec![0]).unwrap();
        assert_eq!(registry.output_extension(&image, Format::Bmp), "bmp");
        assert_eq!(registry.output_extension(&image, Format::Qoi), "qoi");
    }
}
