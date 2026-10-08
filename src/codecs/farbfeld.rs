//! farbfeld 编解码器。
//!
//! farbfeld 的结构非常简单:8 字节魔数 `farbfeld`,随后是大端 32 位宽高,再往后
//! 是每个通道 16 位的 RGBA 像素。它本身不压缩,因此适合作为无损中转格式。
//!
//! 本软件内部使用 8 位通道,读取时取 16 位通道的高字节,写出时把 8 位通道复制
//! 到高字节与低字节(即 `v * 257`),这样往返转换不会有任何精度损失。

use crate::core::codec::{Decoder, Encoder};
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;
use crate::core::pixel::{ColorType, Rgba};
use crate::core::registry::Registry;
use crate::util::bytes::{ByteReader, ByteWriter};

/// farbfeld 魔数。
const MAGIC: &[u8; 8] = b"farbfeld";

/// 注册 farbfeld 解码器与编码器。
pub fn register(registry: &mut Registry) {
    registry.register_decoder(Box::new(FarbfeldCodec));
    registry.register_encoder(Box::new(FarbfeldCodec));
}

/// farbfeld 编解码器。
struct FarbfeldCodec;

impl Decoder for FarbfeldCodec {
    fn format(&self) -> Format {
        Format::Farbfeld
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
        if width == 0 || height == 0 {
            return Err(ConvertError::corrupt("farbfeld 宽高不能为 0"));
        }

        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|count| count.checked_mul(8))
            .ok_or_else(|| ConvertError::corrupt("farbfeld 尺寸过大"))?;
        if reader.remaining() < expected {
            return Err(ConvertError::corrupt(format!(
                "farbfeld 像素数据不完整:需要 {expected} 字节,剩余 {} 字节",
                reader.remaining()
            )));
        }

        let mut image = Image::new_zeroed(width, height, ColorType::Rgba8)?;
        for y in 0..height {
            for x in 0..width {
                let chunk = reader.read_bytes(8)?;
                // 每个通道 16 位大端,取高字节即可映射到 8 位。
                image.set_pixel(
                    x,
                    y,
                    Rgba::new(chunk[0], chunk[2], chunk[4], chunk[6]),
                );
            }
        }

        Ok(image)
    }
}

impl Encoder for FarbfeldCodec {
    fn format(&self) -> Format {
        Format::Farbfeld
    }

    fn encode(&self, image: &Image, _options: &EncodeOptions) -> Result<Vec<u8>> {
        let pixel_count = image.pixel_count();
        let mut writer = ByteWriter::with_capacity(MAGIC.len() + 8 + pixel_count * 8);

        writer.write_bytes(MAGIC);
        writer.write_u32_be(image.width);
        writer.write_u32_be(image.height);

        for y in 0..image.height {
            for x in 0..image.width {
                let pixel = image.get_pixel(x, y);
                for channel in pixel.to_array() {
                    // 8 位值重复填充高低两个字节,保证精度无损。
                    writer.write_u8(channel);
                    writer.write_u8(channel);
                }
            }
        }

        Ok(writer.into_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_checks_magic() {
        assert!(FarbfeldCodec.sniff(MAGIC));
        // 魔数只是文件头的前缀,后面还有内容同样成立。
        assert!(FarbfeldCodec.sniff(b"farbfelds"));
        assert!(!FarbfeldCodec.sniff(b"farbfel"));
        assert!(!FarbfeldCodec.sniff(b"FARBFELD"));
        assert!(!FarbfeldCodec.sniff(b""));
    }

    #[test]
    fn header_is_big_endian() {
        let image = Image::filled(1, 2, ColorType::Rgba8, Rgba::new(1, 2, 3, 4)).unwrap();
        let bytes = FarbfeldCodec
            .encode(&image, &EncodeOptions::default())
            .unwrap();
        assert_eq!(&bytes[..8], MAGIC);
        assert_eq!(&bytes[8..12], &[0, 0, 0, 1]);
        assert_eq!(&bytes[12..16], &[0, 0, 0, 2]);
        // 每个通道高低字节相同
        assert_eq!(&bytes[16..24], &[1, 1, 2, 2, 3, 3, 4, 4]);
    }

    #[test]
    fn zero_size_is_rejected() {
        let mut writer = ByteWriter::new();
        writer.write_bytes(MAGIC);
        writer.write_u32_be(0);
        writer.write_u32_be(4);
        assert!(FarbfeldCodec.decode(&writer.into_vec()).is_err());
    }

    #[test]
    fn truncated_pixels_are_rejected() {
        let mut writer = ByteWriter::new();
        writer.write_bytes(MAGIC);
        writer.write_u32_be(2);
        writer.write_u32_be(2);
        writer.write_bytes(&[0u8; 8]);
        assert!(FarbfeldCodec.decode(&writer.into_vec()).is_err());
    }

    #[test]
    fn round_trip_is_exact() {
        let source = crate::codecs::tests::sample_image();
        let bytes = FarbfeldCodec
            .encode(&source, &EncodeOptions::default())
            .unwrap();
        let decoded = FarbfeldCodec.decode(&bytes).unwrap();
        assert_eq!(decoded.data, source.data);
        assert_eq!(decoded.color, ColorType::Rgba8);
    }
}
