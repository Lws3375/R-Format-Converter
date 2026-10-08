//! QOI(Quite OK Image)编解码器。
//!
//! QOI 是一种面向快速编解码设计的无损格式,解码器只需要一次线性扫描,不需要
//! 任何熵编码。格式由 14 字节文件头与若干操作码组成:
//!
//! | 操作码          | 含义                                   |
//! |-----------------|----------------------------------------|
//! | `00xxxxxx`      | 索引:直接取哈希表中的颜色             |
//! | `01xxxxxx`      | 差分:RGB 三通道各带 2 位偏移           |
//! | `10xxxxxx`      | 亮度:绿色差分加红蓝二次差分            |
//! | `11xxxxxx`      | 游程:重复上一个像素若干次             |
//! | `11111110`      | 完整 RGB                               |
//! | `11111111`      | 完整 RGBA                              |
//!
//! 哈希表共 64 项,索引公式为 `(r * 3 + g * 5 + b * 7 + a * 11) % 64`。

use crate::core::codec::{Decoder, Encoder};
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;
use crate::core::pixel::{ColorType, Rgba};
use crate::core::registry::Registry;
use crate::util::bytes::{ByteReader, ByteWriter};

/// 文件魔数。
const MAGIC: &[u8; 4] = b"qoif";
/// 哈希表容量。
const HASH_SIZE: usize = 64;
/// 游程编码允许的最大重复次数。
const MAX_RUN: usize = 62;
/// 流结束标记:7 个 0 加一个 1。
const END_MARKER: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 1];

/// 操作码掩码与取值。
mod op {
    /// 高 2 位掩码。
    pub const MASK: u8 = 0xC0;
    /// 索引。
    pub const INDEX: u8 = 0x00;
    /// 差分。
    pub const DIFF: u8 = 0x40;
    /// 亮度。
    pub const LUMA: u8 = 0x80;
    /// 游程。
    pub const RUN: u8 = 0xC0;
    /// 完整 RGB。
    pub const RGB: u8 = 0xFE;
    /// 完整 RGBA。
    pub const RGBA: u8 = 0xFF;
}

/// 注册 QOI 解码器与编码器。
pub fn register(registry: &mut Registry) {
    registry.register_decoder(Box::new(QoiCodec));
    registry.register_encoder(Box::new(QoiCodec));
}

/// QOI 编解码器。
struct QoiCodec;

impl Decoder for QoiCodec {
    fn format(&self) -> Format {
        Format::Qoi
    }

    fn sniff(&self, header: &[u8]) -> bool {
        header.starts_with(MAGIC)
    }

    fn decode(&self, data: &[u8]) -> Result<Image> {
        if !data.starts_with(MAGIC) {
            return Err(ConvertError::UnknownFormat);
        }

        let mut reader = ByteReader::new(data);
        reader.skip(MAGIC.len())?;
        let width = reader.read_u32_be()?;
        let height = reader.read_u32_be()?;
        let channels = reader.read_u8()?;
        let _colorspace = reader.read_u8()?;

        if width == 0 || height == 0 {
            return Err(ConvertError::corrupt("QOI 宽高不能为 0"));
        }
        if channels != 3 && channels != 4 {
            return Err(ConvertError::corrupt(format!("QOI 通道数为 {channels}")));
        }

        let total = (width as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| ConvertError::corrupt("QOI 尺寸过大"))?;

        // 哈希表初值为全零像素,当前像素初值为不透明黑,与格式规范一致。
        let mut index = [Rgba::TRANSPARENT; HASH_SIZE];
        let mut pixel = Rgba::new(0, 0, 0, 255);
        let mut run = 0usize;
        let mut pixels = Vec::with_capacity(total);

        while pixels.len() < total {
            if run > 0 {
                run -= 1;
            } else {
                let first = reader.read_u8()?;
                if first == op::RGB {
                    pixel.r = reader.read_u8()?;
                    pixel.g = reader.read_u8()?;
                    pixel.b = reader.read_u8()?;
                } else if first == op::RGBA {
                    pixel.r = reader.read_u8()?;
                    pixel.g = reader.read_u8()?;
                    pixel.b = reader.read_u8()?;
                    pixel.a = reader.read_u8()?;
                } else {
                    match first & op::MASK {
                        op::INDEX => {
                            pixel = index[(first & 0x3F) as usize];
                        }
                        op::DIFF => {
                            pixel.r = pixel
                                .r
                                .wrapping_add(((first >> 4) & 0x03).wrapping_sub(2));
                            pixel.g = pixel
                                .g
                                .wrapping_add(((first >> 2) & 0x03).wrapping_sub(2));
                            pixel.b = pixel.b.wrapping_add((first & 0x03).wrapping_sub(2));
                        }
                        op::LUMA => {
                            let second = reader.read_u8()?;
                            let dg = (first & 0x3F) as i32 - 32;
                            let dr = ((second >> 4) & 0x0F) as i32 - 8;
                            let db = (second & 0x0F) as i32 - 8;
                            pixel.r = wrap_add(pixel.r, dg + dr);
                            pixel.g = wrap_add(pixel.g, dg);
                            pixel.b = wrap_add(pixel.b, dg + db);
                        }
                        _ => {
                            run = (first & 0x3F) as usize;
                        }
                    }
                }
                index[hash(pixel)] = pixel;
            }

            pixels.push(pixel);
        }

        let mut image = Image::new_zeroed(width, height, ColorType::Rgba8)?;
        for (offset, pixel) in pixels.iter().enumerate() {
            image.set_pixel(
                (offset % width as usize) as u32,
                (offset / width as usize) as u32,
                *pixel,
            );
        }
        Ok(image)
    }
}

impl Encoder for QoiCodec {
    fn format(&self) -> Format {
        Format::Qoi
    }

    fn encode(&self, image: &Image, _options: &EncodeOptions) -> Result<Vec<u8>> {
        let channels = if uses_alpha(image) { 4u8 } else { 3u8 };

        let mut writer = ByteWriter::with_capacity(14 + image.pixel_count() * 4 + 8);
        writer.write_bytes(MAGIC);
        writer.write_u32_be(image.width);
        writer.write_u32_be(image.height);
        writer.write_u8(channels);
        // 色彩空间 0 表示 sRGB 且 Alpha 为线性。
        writer.write_u8(0);

        let mut index = [Rgba::TRANSPARENT; HASH_SIZE];
        let mut previous = Rgba::new(0, 0, 0, 255);
        let mut run = 0usize;
        let total = image.pixel_count();

        for offset in 0..total {
            let x = (offset % image.width as usize) as u32;
            let y = (offset / image.width as usize) as u32;
            let mut pixel = image.get_pixel(x, y);
            if channels == 3 {
                pixel.a = 255;
            }

            if pixel == previous {
                run += 1;
                // 游程写满或到达图像末尾时立即输出。
                if run == MAX_RUN || offset == total - 1 {
                    writer.write_u8(op::RUN | (run as u8 - 1));
                    run = 0;
                }
            } else {
                if run > 0 {
                    writer.write_u8(op::RUN | (run as u8 - 1));
                    run = 0;
                }

                let slot = hash(pixel);
                if index[slot] == pixel {
                    writer.write_u8(op::INDEX | slot as u8);
                } else {
                    index[slot] = pixel;

                    if pixel.a == previous.a {
                        let dr = pixel.r as i32 - previous.r as i32;
                        let dg = pixel.g as i32 - previous.g as i32;
                        let db = pixel.b as i32 - previous.b as i32;
                        let dr_dg = dr - dg;
                        let db_dg = db - dg;

                        if (-2..=1).contains(&dr)
                            && (-2..=1).contains(&dg)
                            && (-2..=1).contains(&db)
                        {
                            writer.write_u8(
                                op::DIFF
                                    | (((dr + 2) as u8) << 4)
                                    | (((dg + 2) as u8) << 2)
                                    | ((db + 2) as u8),
                            );
                        } else if (-32..=31).contains(&dg)
                            && (-8..=7).contains(&dr_dg)
                            && (-8..=7).contains(&db_dg)
                        {
                            writer.write_u8(op::LUMA | ((dg + 32) as u8));
                            writer.write_u8((((dr_dg + 8) as u8) << 4) | ((db_dg + 8) as u8));
                        } else {
                            writer.write_u8(op::RGB);
                            writer.write_u8(pixel.r);
                            writer.write_u8(pixel.g);
                            writer.write_u8(pixel.b);
                        }
                    } else {
                        writer.write_u8(op::RGBA);
                        writer.write_u8(pixel.r);
                        writer.write_u8(pixel.g);
                        writer.write_u8(pixel.b);
                        writer.write_u8(pixel.a);
                    }
                }
                previous = pixel;
            }
        }

        writer.write_bytes(&END_MARKER);
        Ok(writer.into_vec())
    }
}

/// 计算哈希槽位。
fn hash(pixel: Rgba) -> usize {
    let value = pixel.r as usize * 3
        + pixel.g as usize * 5
        + pixel.b as usize * 7
        + pixel.a as usize * 11;
    value % HASH_SIZE
}

/// 带环绕的加法,保证通道在 0~255 之间循环。
fn wrap_add(value: u8, delta: i32) -> u8 {
    (value as i32 + delta).rem_euclid(256) as u8
}

/// 判断图像是否真的包含半透明像素。
fn uses_alpha(image: &Image) -> bool {
    image.color.has_alpha()
        && (0..image.height).any(|y| (0..image.width).any(|x| image.get_pixel(x, y).a != 255))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_checks_magic() {
        assert!(QoiCodec.sniff(b"qoif"));
        assert!(!QoiCodec.sniff(b"qoi"));
        assert!(!QoiCodec.sniff(b"png"));
    }

    #[test]
    fn hash_is_within_table() {
        for r in [0u8, 1, 128, 255] {
            for a in [0u8, 255] {
                assert!(hash(Rgba::new(r, r, r, a)) < HASH_SIZE);
            }
        }
    }

    #[test]
    fn wrap_add_cycles() {
        assert_eq!(wrap_add(0, -1), 255);
        assert_eq!(wrap_add(255, 1), 0);
        assert_eq!(wrap_add(10, 5), 15);
    }

    #[test]
    fn solid_image_compresses_to_a_run() {
        let image = Image::filled(64, 1, ColorType::Rgb8, Rgba::from_rgb(7, 8, 9)).unwrap();
        let bytes = QoiCodec.encode(&image, &EncodeOptions::default()).unwrap();
        // 14 字节文件头 + 一次 RGB + 一段游程 + 8 字节结束标记
        assert!(bytes.len() <= 14 + 4 + 2 + 8, "实际长度 {}", bytes.len());
        let decoded = QoiCodec.decode(&bytes).unwrap();
        assert_eq!(decoded.get_pixel(63, 0), Rgba::from_rgb(7, 8, 9));
    }

    #[test]
    fn channels_field_reflects_alpha_usage() {
        let opaque = Image::filled(2, 2, ColorType::Rgb8, Rgba::from_rgb(1, 1, 1)).unwrap();
        let bytes = QoiCodec.encode(&opaque, &EncodeOptions::default()).unwrap();
        assert_eq!(bytes[12], 3);

        let mut alpha = Image::new_zeroed(2, 2, ColorType::Rgba8).unwrap();
        alpha.set_pixel(0, 0, Rgba::new(1, 1, 1, 128));
        let bytes = QoiCodec.encode(&alpha, &EncodeOptions::default()).unwrap();
        assert_eq!(bytes[12], 4);
    }

    #[test]
    fn invalid_channel_count_is_rejected() {
        let mut writer = ByteWriter::new();
        writer.write_bytes(MAGIC);
        writer.write_u32_be(1);
        writer.write_u32_be(1);
        writer.write_u8(2);
        writer.write_u8(0);
        writer.write_bytes(&END_MARKER);
        assert!(QoiCodec.decode(&writer.into_vec()).is_err());
    }

    #[test]
    fn truncated_stream_is_rejected() {
        let mut writer = ByteWriter::new();
        writer.write_bytes(MAGIC);
        writer.write_u32_be(8);
        writer.write_u32_be(8);
        writer.write_u8(4);
        writer.write_u8(0);
        writer.write_u8(op::RGB);
        writer.write_u8(1);
        assert!(QoiCodec.decode(&writer.into_vec()).is_err());
    }

    #[test]
    fn run_of_max_length_is_split() {
        let image = Image::filled(200, 1, ColorType::Rgb8, Rgba::from_rgb(0, 0, 0)).unwrap();
        let bytes = QoiCodec.encode(&image, &EncodeOptions::default()).unwrap();
        let decoded = QoiCodec.decode(&bytes).unwrap();
        assert_eq!(decoded.width, 200);
        assert_eq!(decoded.get_pixel(199, 0), Rgba::from_rgb(0, 0, 0));
    }
}
