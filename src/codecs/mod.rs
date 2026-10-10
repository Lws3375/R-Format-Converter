//! 格式识别与编解码。
//!
//! 软件不再自行实现各格式的解析,全部读写都交给 `image` 库完成。本模块的职责
//! 只有三件事:把内部的 [`Format`] 映射成 `image::ImageFormat`;把软件自己的编码
//! 选项(JPEG 质量、PNG 压缩等级)交给对应的编码器;对 Netpbm 这种"一个格式多种
//! 子类型"的情况,按图像颜色模式决定输出扩展名。

use std::borrow::Cow;
use std::io::Cursor;

use image::ImageEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::ImageFormat;

use crate::core::color::ColorType;
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::{Image, color_type, normalize, to_color};
use crate::core::options::EncodeOptions;
use crate::core::registry::Registry;

/// ICO 图标允许的最大边长。
pub const ICO_MAX_DIMENSION: u32 = 256;

/// GIF 图像允许的最大边长 (16 位无符号整数上限)。
pub const GIF_MAX_DIMENSION: u32 = 65535;

/// 装配软件支持的格式列表。
///
/// 列表顺序同时决定界面目标格式下拉框的排列顺序,因此保持稳定;TGA 放在最后,
/// 因为它没有魔数,兜底逐个尝试时应最后才轮到它。
pub fn build_default() -> Registry {
    Registry::new(vec![
        Format::Bmp,
        Format::Netpbm,
        Format::Png,
        Format::Gif,
        Format::Qoi,
        Format::Farbfeld,
        Format::Ico,
        Format::Jpeg,
        Format::Tga,
    ])
}

/// 内部格式与 `image` 库格式之间的映射。
fn image_format(format: Format) -> ImageFormat {
    match format {
        Format::Bmp => ImageFormat::Bmp,
        Format::Netpbm => ImageFormat::Pnm,
        Format::Tga => ImageFormat::Tga,
        Format::Qoi => ImageFormat::Qoi,
        Format::Farbfeld => ImageFormat::Farbfeld,
        Format::Ico => ImageFormat::Ico,
        Format::Png => ImageFormat::Png,
        Format::Jpeg => ImageFormat::Jpeg,
        Format::Gif => ImageFormat::Gif,
    }
}

/// 按文件头识别格式。
///
/// TGA 没有固定魔数,`image` 库无法嗅探,这种情况返回 `None`,由调用方按扩展名
/// 或逐个尝试兜底。
pub fn detect(data: &[u8]) -> Option<Format> {
    let sniffed = image::guess_format(data).ok()?;
    Format::all()
        .iter()
        .copied()
        .find(|format| image_format(*format) == sniffed)
}

/// 解码为内存位图,并统一到 8 位深度。
pub fn decode(data: &[u8], format: Format) -> Result<Image> {
    let image = image::load_from_memory_with_format(data, image_format(format))
        .map_err(|error| ConvertError::corrupt(error.to_string()))?;
    if image.width() == 0 || image.height() == 0 {
        return Err(ConvertError::corrupt("图像尺寸为零"));
    }
    Ok(normalize(image))
}

/// 编码为字节流。
pub fn encode(image: &Image, format: Format, options: &EncodeOptions) -> Result<Vec<u8>> {
    let prepared = prepare_for(image, format);
    let prepared = prepared.as_ref();
    match format {
        Format::Jpeg => encode_with_jpeg(prepared, options.quality),
        Format::Png => encode_with_png(prepared, options.png_compression_level),
        other => encode_default(prepared, image_format(other)),
    }
}

/// 目标格式针对该图像建议的输出扩展名。
pub fn output_extension(image: &Image, format: Format) -> &'static str {
    match format {
        // Netpbm 是一个格式家族,按颜色模式选用更贴切的子类型扩展名。
        Format::Netpbm => match color_type(image) {
            ColorType::Gray8 => "pgm",
            ColorType::Rgb8 => "ppm",
            ColorType::GrayAlpha8 | ColorType::Rgba8 => "pam",
        },
        other => other.primary_extension(),
    }
}

/// 把位图调整成目标格式的编码器真正接受的形状。
///
/// `image` 库只在 BMP、JPEG、TGA 上做了自动降级,其余编码器收到不支持的通道布局
/// 会直接报错,所以这里先按格式收敛一次:ICO 内嵌的 PNG 必须是 RGBA;QOI 没有灰度
/// 色彩空间,只收真彩色;farbfeld 是 16 位 RGBA 专用格式;其余格式在 8 位深度下都
/// 能容纳四种布局。
///
/// 只有需要改写位图时才复制,常见路径上仍是零拷贝借用。
fn prepare_for(image: &Image, format: Format) -> Cow<'_, Image> {
    match format {
        Format::Ico => ensure(image, ColorType::Rgba8),
        Format::Qoi | Format::Gif => match color_type(image) {
            ColorType::Gray8 => ensure(image, ColorType::Rgb8),
            ColorType::GrayAlpha8 => ensure(image, ColorType::Rgba8),
            _ => Cow::Borrowed(image),
        },
        // 8 位数据升到 16 位是位复制,往返后像素仍然精确。
        Format::Farbfeld => Cow::Owned(Image::ImageRgba16(image.to_rgba16())),
        _ => Cow::Borrowed(image),
    }
}

/// 已经是指定颜色模式时原样借用,否则转换后再交出所有权。
fn ensure(image: &Image, color: ColorType) -> Cow<'_, Image> {
    if color_type(image) == color {
        Cow::Borrowed(image)
    } else {
        Cow::Owned(to_color(image, color))
    }
}

/// 使用格式自带的默认参数编码。
fn encode_default(image: &Image, format: ImageFormat) -> Result<Vec<u8>> {
    let mut buffer = Cursor::new(Vec::new());
    image
        .write_to(&mut buffer, format)
        .map_err(|error| ConvertError::corrupt(error.to_string()))?;
    Ok(buffer.into_inner())
}

/// JPEG 使用界面上的质量参数,其余参数保持默认。
fn encode_with_jpeg(image: &Image, quality: u8) -> Result<Vec<u8>> {
    let mut buffer = Cursor::new(Vec::new());
    // 质量参数的合法范围是 1~100,越界会让编码器直接报错。
    let encoder = JpegEncoder::new_with_quality(&mut buffer, quality.clamp(1, 100));
    write_with(image, encoder)?;
    Ok(buffer.into_inner())
}

/// PNG 使用界面上的压缩等级,像素数据本身不受影响。
fn encode_with_png(image: &Image, level: u8) -> Result<Vec<u8>> {
    let mut buffer = Cursor::new(Vec::new());
    let encoder =
        PngEncoder::new_with_quality(&mut buffer, png_compression(level), FilterType::Adaptive);
    write_with(image, encoder)?;
    Ok(buffer.into_inner())
}

/// 用指定编码器写出图像。
///
/// 传入的位图应当已经过 [`prepare_for`] 处理,这里只负责把编码器自身的报错转成
/// 软件统一的错误类型。
fn write_with(image: &Image, encoder: impl ImageEncoder) -> Result<()> {
    image
        .write_with_encoder(encoder)
        .map_err(|error| ConvertError::corrupt(error.to_string()))
}

/// 把界面上的 0~9 压缩等级映射到 `image` 库的压缩档位。
///
/// 0 表示完全不压缩,9 表示最大压缩;像素数据无损,改变的只是文件体积与耗时。
fn png_compression(level: u8) -> CompressionType {
    match level {
        0 => CompressionType::Uncompressed,
        1..=3 => CompressionType::Fast,
        4..=7 => CompressionType::Default,
        _ => CompressionType::Best,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一张内容确定的测试图,覆盖多种颜色与半透明像素。
    pub(crate) fn sample_image() -> Image {
        let mut buffer = image::RgbaImage::new(4, 3);
        let palette = [
            image::Rgba([255, 0, 0, 255]),
            image::Rgba([0, 255, 0, 255]),
            image::Rgba([0, 0, 255, 255]),
            image::Rgba([255, 255, 255, 255]),
            image::Rgba([128, 64, 32, 255]),
            image::Rgba([10, 20, 30, 128]),
        ];
        for y in 0..buffer.height() {
            for x in 0..buffer.width() {
                let index = (y * buffer.width() + x) as usize % palette.len();
                buffer.put_pixel(x, y, palette[index]);
            }
        }
        Image::ImageRgba8(buffer)
    }

    #[test]
    fn default_registry_covers_all_implemented_formats() {
        let registry = build_default();
        for format in Format::all() {
            assert_eq!(
                registry.is_supported(*format),
                format.is_implemented(),
                "{} 的注册状态与实现状态不一致",
                format.id()
            );
        }
    }

    #[test]
    fn every_lossless_format_round_trips_pixel_exact() {
        let registry = build_default();
        let source = sample_image();
        for format in Format::all()
            .iter()
            .filter(|f| f.is_implemented() && f.is_lossless())
        {
            let encoded = registry
                .encode(&source, *format, &EncodeOptions::default())
                .unwrap_or_else(|err| panic!("{} 编码失败:{err}", format.id()));
            let decoded = registry
                .decode_as(&encoded, *format)
                .unwrap_or_else(|err| panic!("{} 解码失败:{err}", format.id()));
            assert_eq!(
                (decoded.width(), decoded.height()),
                (source.width(), source.height()),
                "{} 尺寸不一致",
                format.id()
            );
            assert_eq!(
                decoded.to_rgba8().as_raw(),
                source.to_rgba8().as_raw(),
                "{} 往返后像素不一致",
                format.id()
            );
        }
    }

    #[test]
    fn magic_detection_finds_every_format_that_has_one() {
        let registry = build_default();
        let source = sample_image();
        for format in registry.writable_formats() {
            let encoded = registry
                .encode(&source, format, &EncodeOptions::default())
                .unwrap();
            let detected = registry.detect(&encoded);
            if format == Format::Tga {
                // TGA 没有魔数,只能靠扩展名或逐个尝试兜底。
                assert_eq!(detected, None, "TGA 不应被文件头嗅探识别");
            } else {
                assert_eq!(detected, Some(format), "{} 的文件头未被识别", format.id());
            }
        }
    }

    #[test]
    fn jpeg_quality_changes_output_size() {
        let registry = build_default();
        let source = sample_image();
        let low = registry
            .encode(
                &source,
                Format::Jpeg,
                &EncodeOptions {
                    quality: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        let high = registry
            .encode(
                &source,
                Format::Jpeg,
                &EncodeOptions {
                    quality: 95,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(low.len() < high.len(), "低质量 JPEG 应比高质量更小");
    }

    #[test]
    fn png_compression_level_changes_output_size() {
        let registry = build_default();
        let source = sample_image();
        let fast = registry
            .encode(
                &source,
                Format::Png,
                &EncodeOptions {
                    png_compression_level: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        let best = registry
            .encode(
                &source,
                Format::Png,
                &EncodeOptions {
                    png_compression_level: 9,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(best.len() <= fast.len(), "最高压缩等级不应产生更大文件");
    }

    #[test]
    fn netpbm_output_extension_follows_color_mode() {
        let gray = Image::ImageLuma8(image::GrayImage::new(2, 2));
        assert_eq!(output_extension(&gray, Format::Netpbm), "pgm");
        assert_eq!(output_extension(&gray, Format::Png), "png");

        let rgba = Image::ImageRgba8(image::RgbaImage::new(2, 2));
        assert_eq!(output_extension(&rgba, Format::Netpbm), "pam");

        let rgb = Image::ImageRgb8(image::RgbImage::new(2, 2));
        assert_eq!(output_extension(&rgb, Format::Netpbm), "ppm");
    }

    #[test]
    fn corrupt_data_is_reported_not_panicking() {
        let err = decode(&[0u8; 64], Format::Bmp).unwrap_err();
        assert!(matches!(err, ConvertError::Corrupt(_)));
    }

    #[test]
    fn gif_round_trip_preserves_dimensions_and_approximate_colors() {
        let registry = build_default();
        let source = sample_image();
        let encoded = registry
            .encode(&source, Format::Gif, &EncodeOptions::default())
            .expect("GIF 编码失败");
        let detected = registry.detect(&encoded);
        assert_eq!(detected, Some(Format::Gif));
        let decoded = registry
            .decode_as(&encoded, Format::Gif)
            .expect("GIF 解码失败");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (source.width(), source.height())
        );
    }
}
