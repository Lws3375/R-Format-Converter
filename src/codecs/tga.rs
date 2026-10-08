//! TGA(Truevision Targa)编解码器。
//!
//! 文件由 18 字节文件头、可选图像标识区、可选调色板与像素数据组成:
//!
//! | 图像类型 | 含义               |
//! |----------|--------------------|
//! | 1 / 9    | 调色板图 / RLE     |
//! | 2 / 10   | 真彩图 / RLE       |
//! | 3 / 11   | 灰度图 / RLE       |
//!
//! 16 位像素按 A1R5G5B5 解释,像素数据的行列方向由描述符字节决定。写出时使用
//! 自上而下的不压缩格式,并在文件末尾附加 `TRUEVISION-XFILE` 签名页脚。

use crate::core::codec::{Decoder, Encoder};
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;
use crate::core::pixel::{ColorType, Rgba};
use crate::core::registry::Registry;
use crate::util::bytes::{ByteReader, ByteWriter};

/// 文件头长度。
const HEADER_LEN: usize = 18;

/// 文件末尾的签名。
const FOOTER_SIGNATURE: &[u8; 18] = b"TRUEVISION-XFILE.\0";

/// 描述符位:像素自右向左排列。
const FLAG_RIGHT_TO_LEFT: u8 = 0x10;
/// 描述符位:像素自上而下排列。
const FLAG_TOP_TO_BOTTOM: u8 = 0x20;

/// 图像类型常量。
mod image_type {
    /// 无图像数据。
    pub const NONE: u8 = 0;
    /// 调色板图。
    pub const COLOR_MAPPED: u8 = 1;
    /// 真彩图。
    pub const TRUE_COLOR: u8 = 2;
    /// 灰度图。
    pub const GRAYSCALE: u8 = 3;
    /// RLE 调色板图。
    pub const COLOR_MAPPED_RLE: u8 = 9;
    /// RLE 真彩图。
    pub const TRUE_COLOR_RLE: u8 = 10;
    /// RLE 灰度图。
    pub const GRAYSCALE_RLE: u8 = 11;
}

/// 注册 TGA 解码器与编码器。
pub fn register(registry: &mut Registry) {
    registry.register_decoder(Box::new(TgaCodec));
    registry.register_encoder(Box::new(TgaCodec));
}

/// 解析后的 TGA 文件头。
struct TgaHeader {
    id_length: usize,
    color_map_type: u8,
    image_type: u8,
    color_map_first_index: usize,
    color_map_length: usize,
    color_map_entry_bytes: usize,
    width: u32,
    height: u32,
    pixel_bytes: usize,
    descriptor: u8,
}

impl TgaHeader {
    /// 是否为游程编码。
    fn is_rle(&self) -> bool {
        matches!(
            self.image_type,
            image_type::COLOR_MAPPED_RLE | image_type::TRUE_COLOR_RLE | image_type::GRAYSCALE_RLE
        )
    }

    /// 是否为调色板图。
    fn is_color_mapped(&self) -> bool {
        matches!(
            self.image_type,
            image_type::COLOR_MAPPED | image_type::COLOR_MAPPED_RLE
        )
    }

    /// 是否为灰度图。
    fn is_grayscale(&self) -> bool {
        matches!(
            self.image_type,
            image_type::GRAYSCALE | image_type::GRAYSCALE_RLE
        )
    }

    /// 声明的透明度位数。
    fn alpha_bits(&self) -> u8 {
        self.descriptor & 0x0F
    }
}

/// TGA 编解码器。
struct TgaCodec;

impl Decoder for TgaCodec {
    fn format(&self) -> Format {
        Format::Tga
    }

    fn sniff(&self, header: &[u8]) -> bool {
        // TGA 没有魔数,只能依据文件头的取值范围做保守判断。
        if header.len() < HEADER_LEN {
            return false;
        }
        let color_map_type = header[1];
        let image_type = header[2];
        let color_map_entry_bits = header[7];
        let width = u16::from_le_bytes([header[12], header[13]]);
        let height = u16::from_le_bytes([header[14], header[15]]);
        let pixel_depth = header[16];

        if !matches!(color_map_type, 0 | 1) {
            return false;
        }
        if !matches!(
            image_type,
            image_type::COLOR_MAPPED
                | image_type::TRUE_COLOR
                | image_type::GRAYSCALE
                | image_type::COLOR_MAPPED_RLE
                | image_type::TRUE_COLOR_RLE
                | image_type::GRAYSCALE_RLE
        ) {
            return false;
        }
        if width == 0 || height == 0 {
            return false;
        }
        if !matches!(pixel_depth, 8 | 15 | 16 | 24 | 32) {
            return false;
        }
        // 调色板必须声明合法的项宽,这一条同时排除了头部形似的 ICO 文件。
        if color_map_type == 1 && !matches!(color_map_entry_bits, 15 | 16 | 24 | 32) {
            return false;
        }
        if color_map_type == 0 && color_map_entry_bits != 0 {
            return false;
        }
        // 灰度类型不允许声明调色板。
        if color_map_type == 1 && image_type == image_type::GRAYSCALE {
            return false;
        }
        true
    }

    fn decode(&self, data: &[u8]) -> Result<Image> {
        let header = parse_header(data)?;
        let palette = read_palette(data, &header)?;
        let payload = pixel_payload(data, &header)?;

        let total = (header.width as usize)
            .checked_mul(header.height as usize)
            .ok_or_else(|| ConvertError::corrupt("TGA 尺寸过大"))?;

        let pixels = if header.is_rle() {
            decode_rle(payload, &header, &palette, total)?
        } else {
            decode_plain(payload, &header, &palette, total)?
        };

        build_image(&header, pixels)
    }
}

impl Encoder for TgaCodec {
    fn format(&self) -> Format {
        Format::Tga
    }

    fn encode(&self, image: &Image, _options: &EncodeOptions) -> Result<Vec<u8>> {
        match image.color {
            ColorType::Gray8 => encode_grayscale(image, false),
            ColorType::GrayAlpha8 => encode_grayscale(image, true),
            ColorType::Rgb8 => encode_true_color(image, false),
            ColorType::Rgba8 => {
                if has_transparency(image) {
                    encode_true_color(image, true)
                } else {
                    encode_true_color(image, false)
                }
            }
        }
    }
}

/// 判断图像是否包含非全不透明的像素。
fn has_transparency(image: &Image) -> bool {
    image.color.has_alpha()
        && (0..image.height).any(|y| (0..image.width).any(|x| image.get_pixel(x, y).a != 255))
}

/// 把 5 位通道扩展到 8 位。
fn expand_five_bits(value: u16) -> u8 {
    (((value & 0x1F) as u32 * 255 + 15) / 31) as u8
}

/// 解析 16 位 A1R5G5B5 像素。
fn unpack_16(value: u16, alpha_bits: u8) -> Rgba {
    let alpha = if alpha_bits > 0 {
        if value & 0x8000 != 0 {
            255
        } else {
            0
        }
    } else {
        255
    };
    Rgba::new(
        expand_five_bits(value >> 10),
        expand_five_bits(value >> 5),
        expand_five_bits(value),
        alpha,
    )
}

/// 解析文件头。
fn parse_header(data: &[u8]) -> Result<TgaHeader> {
    if data.len() < HEADER_LEN {
        return Err(ConvertError::UnknownFormat);
    }
    let mut reader = ByteReader::new(data);
    let id_length = reader.read_u8()? as usize;
    let color_map_type = reader.read_u8()?;
    let image_type = reader.read_u8()?;
    let color_map_first_index = reader.read_u16_le()? as usize;
    let color_map_length = reader.read_u16_le()? as usize;
    let color_map_entry_bits = reader.read_u8()?;
    let _x_origin = reader.read_u16_le()?;
    let _y_origin = reader.read_u16_le()?;
    let width = reader.read_u16_le()? as u32;
    let height = reader.read_u16_le()? as u32;
    let pixel_depth = reader.read_u8()?;
    let descriptor = reader.read_u8()?;

    if width == 0 || height == 0 {
        return Err(ConvertError::corrupt("TGA 宽高不能为 0"));
    }
    if image_type == image_type::NONE {
        return Err(ConvertError::unsupported("TGA 文件不含图像数据"));
    }
    if !matches!(
        image_type,
        image_type::COLOR_MAPPED
            | image_type::TRUE_COLOR
            | image_type::GRAYSCALE
            | image_type::COLOR_MAPPED_RLE
            | image_type::TRUE_COLOR_RLE
            | image_type::GRAYSCALE_RLE
    ) {
        return Err(ConvertError::unsupported(format!(
            "TGA 图像类型 {image_type}"
        )));
    }
    if !matches!(pixel_depth, 8 | 15 | 16 | 24 | 32) {
        return Err(ConvertError::unsupported(format!(
            "TGA 每像素 {pixel_depth} 位"
        )));
    }
    if color_map_type == 1 && !matches!(color_map_entry_bits, 15 | 16 | 24 | 32) {
        return Err(ConvertError::corrupt(format!(
            "TGA 调色板项宽为 {color_map_entry_bits} 位"
        )));
    }

    let pixel_bytes = (pixel_depth as usize).div_ceil(8);
    Ok(TgaHeader {
        id_length,
        color_map_type,
        image_type,
        color_map_first_index,
        color_map_length,
        color_map_entry_bytes: (color_map_entry_bits as usize).div_ceil(8),
        width,
        height,
        pixel_bytes,
        descriptor,
    })
}

/// 读取调色板。
fn read_palette(data: &[u8], header: &TgaHeader) -> Result<Vec<Rgba>> {
    if header.color_map_type != 1 || header.color_map_length == 0 {
        return Ok(Vec::new());
    }
    let start = HEADER_LEN + header.id_length;
    let total = header.color_map_length * header.color_map_entry_bytes;
    let end = start
        .checked_add(total)
        .ok_or_else(|| ConvertError::corrupt("TGA 调色板长度溢出"))?;
    if end > data.len() {
        return Err(ConvertError::corrupt("TGA 调色板数据不完整"));
    }

    let mut palette = Vec::with_capacity(header.color_map_length);
    for index in 0..header.color_map_length {
        let base = start + index * header.color_map_entry_bytes;
        let entry = &data[base..base + header.color_map_entry_bytes];
        let pixel = match header.color_map_entry_bytes {
            2 => {
                let value = u16::from_le_bytes([entry[0], entry[1]]);
                // 调色板的高位属性位在多数文件中恒为 0,统一按不透明处理。
                unpack_16(value, 0)
            }
            3 => Rgba::from_rgb(entry[2], entry[1], entry[0]),
            4 => Rgba::new(entry[2], entry[1], entry[0], entry[3]),
            other => {
                return Err(ConvertError::corrupt(format!(
                    "TGA 调色板项宽 {other} 字节"
                )))
            }
        };
        palette.push(pixel);
    }
    Ok(palette)
}

/// 定位像素数据的起点。
fn pixel_payload<'a>(data: &'a [u8], header: &TgaHeader) -> Result<&'a [u8]> {
    let palette_bytes = if header.color_map_type == 1 {
        header.color_map_length * header.color_map_entry_bytes
    } else {
        0
    };
    let start = HEADER_LEN + header.id_length + palette_bytes;
    data.get(start..)
        .ok_or_else(|| ConvertError::corrupt("TGA 像素数据偏移越界"))
}

/// 读取一个原始像素值。
fn read_pixel(
    reader: &mut ByteReader<'_>,
    header: &TgaHeader,
    palette: &[Rgba],
) -> Result<Rgba> {
    if header.is_color_mapped() {
        let raw = reader.read_bytes(header.pixel_bytes)?;
        let index = raw
            .iter()
            .enumerate()
            .fold(0usize, |acc, (shift, byte)| {
                acc | ((*byte as usize) << (shift * 8))
            });
        let slot = index.saturating_sub(header.color_map_first_index);
        palette
            .get(slot)
            .copied()
            .ok_or_else(|| ConvertError::corrupt(format!("TGA 调色板索引 {index} 越界")))
    } else if header.is_grayscale() && header.pixel_bytes == 2 {
        let raw = reader.read_bytes(2)?;
        let gray = raw[0];
        let alpha = if header.alpha_bits() > 0 { raw[1] } else { 255 };
        Ok(Rgba::new(gray, gray, gray, alpha))
    } else {
        match header.pixel_bytes {
            1 => {
                let value = reader.read_u8()?;
                Ok(Rgba::from_gray(value))
            }
            2 => Ok(unpack_16(reader.read_u16_le()?, header.alpha_bits())),
            3 => {
                let raw = reader.read_bytes(3)?;
                Ok(Rgba::from_rgb(raw[2], raw[1], raw[0]))
            }
            _ => {
                let raw = reader.read_bytes(4)?;
                Ok(Rgba::new(raw[2], raw[1], raw[0], raw[3]))
            }
        }
    }
}

/// 解码不压缩像素数据。
fn decode_plain(
    payload: &[u8],
    header: &TgaHeader,
    palette: &[Rgba],
    total: usize,
) -> Result<Vec<Rgba>> {
    let mut reader = ByteReader::new(payload);
    let mut pixels = Vec::with_capacity(total);
    for _ in 0..total {
        pixels.push(read_pixel(&mut reader, header, palette)?);
    }
    Ok(pixels)
}

/// 解码游程编码的像素数据。
fn decode_rle(
    payload: &[u8],
    header: &TgaHeader,
    palette: &[Rgba],
    total: usize,
) -> Result<Vec<Rgba>> {
    let mut reader = ByteReader::new(payload);
    let mut pixels: Vec<Rgba> = Vec::with_capacity(total);

    while pixels.len() < total {
        let packet = reader.read_u8()?;
        let count = (packet & 0x7F) as usize + 1;
        if packet & 0x80 != 0 {
            // 游程包:后面只跟一个像素值。
            let pixel = read_pixel(&mut reader, header, palette)?;
            for _ in 0..count {
                if pixels.len() >= total {
                    break;
                }
                pixels.push(pixel);
            }
        } else {
            // 原始包:后面跟 `count` 个像素值。
            for _ in 0..count {
                if pixels.len() >= total {
                    break;
                }
                pixels.push(read_pixel(&mut reader, header, palette)?);
            }
        }
    }

    Ok(pixels)
}

/// 依据描述符把像素流排布到位图。
fn build_image(header: &TgaHeader, pixels: Vec<Rgba>) -> Result<Image> {
    let width = header.width;
    let height = header.height;

    let has_transparency = pixels.iter().any(|pixel| pixel.a != 255);
    let all_zero_alpha = pixels.iter().all(|pixel| pixel.a == 0);
    let color = if header.is_grayscale() {
        if header.alpha_bits() == 0 || !has_transparency || all_zero_alpha {
            ColorType::Gray8
        } else {
            ColorType::GrayAlpha8
        }
    } else if all_zero_alpha {
        // 声明了透明度却全为 0,通常是软件没有填充该通道,按不透明处理。
        ColorType::Rgb8
    } else if has_transparency {
        ColorType::Rgba8
    } else {
        ColorType::Rgb8
    };

    let mut image = Image::new_zeroed(width, height, color)?;
    let right_to_left = header.descriptor & FLAG_RIGHT_TO_LEFT != 0;
    let top_to_bottom = header.descriptor & FLAG_TOP_TO_BOTTOM != 0;

    for (index, pixel) in pixels.iter().enumerate() {
        let file_row = (index / width as usize) as u32;
        let file_column = (index % width as usize) as u32;
        let x = if right_to_left {
            width - 1 - file_column
        } else {
            file_column
        };
        let y = if top_to_bottom {
            file_row
        } else {
            height - 1 - file_row
        };
        image.set_pixel(x, y, *pixel);
    }

    Ok(image)
}

/// 写出灰度图,`with_alpha` 为真时使用 16 位灰度加透明度。
fn encode_grayscale(image: &Image, with_alpha: bool) -> Result<Vec<u8>> {
    let pixel_depth = if with_alpha { 16u8 } else { 8u8 };
    let descriptor = FLAG_TOP_TO_BOTTOM | if with_alpha { 8 } else { 0 };
    let mut writer = ByteWriter::with_capacity(HEADER_LEN + image.pixel_count() * 2 + 26);
    write_header(
        &mut writer,
        image,
        image_type::GRAYSCALE,
        pixel_depth,
        0,
        0,
        0,
        descriptor,
    );

    for y in 0..image.height {
        for x in 0..image.width {
            let pixel = image.get_pixel(x, y);
            writer.write_u8(pixel.luma());
            if with_alpha {
                writer.write_u8(pixel.a);
            }
        }
    }

    write_footer(&mut writer);
    Ok(writer.into_vec())
}

/// 写出 24 位或 32 位真彩图。
fn encode_true_color(image: &Image, with_alpha: bool) -> Result<Vec<u8>> {
    let pixel_depth = if with_alpha { 32u8 } else { 24u8 };
    let descriptor = FLAG_TOP_TO_BOTTOM | if with_alpha { 8 } else { 0 };
    let bytes_per_pixel = if with_alpha { 4 } else { 3 };
    let mut writer =
        ByteWriter::with_capacity(HEADER_LEN + image.pixel_count() * bytes_per_pixel + 26);
    write_header(
        &mut writer,
        image,
        image_type::TRUE_COLOR,
        pixel_depth,
        0,
        0,
        0,
        descriptor,
    );

    for y in 0..image.height {
        for x in 0..image.width {
            let pixel = image.get_pixel(x, y);
            writer.write_u8(pixel.b);
            writer.write_u8(pixel.g);
            writer.write_u8(pixel.r);
            if with_alpha {
                writer.write_u8(pixel.a);
            }
        }
    }

    write_footer(&mut writer);
    Ok(writer.into_vec())
}

/// 写出 18 字节文件头。
#[allow(clippy::too_many_arguments)]
fn write_header(
    writer: &mut ByteWriter,
    image: &Image,
    image_type: u8,
    pixel_depth: u8,
    color_map_type: u8,
    color_map_first_index: u16,
    color_map_length: u16,
    descriptor: u8,
) {
    writer.write_u8(0);
    writer.write_u8(color_map_type);
    writer.write_u8(image_type);
    writer.write_u16_le(color_map_first_index);
    writer.write_u16_le(color_map_length);
    writer.write_u8(if color_map_type == 1 { 24 } else { 0 });
    writer.write_u16_le(0);
    writer.write_u16_le(0);
    writer.write_u16_le(image.width as u16);
    writer.write_u16_le(image.height as u16);
    writer.write_u8(pixel_depth);
    writer.write_u8(descriptor);
}

/// 写出签名页脚。
fn write_footer(writer: &mut ByteWriter) {
    writer.write_bytes(FOOTER_SIGNATURE);
    writer.write_zeros(8);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> Result<Image> {
        TgaCodec.decode(bytes)
    }

    /// 手工构造一张 2x1 的 24 位真彩图,使用自下而上的默认行序。
    fn build_true_color_bottom_up() -> Vec<u8> {
        let mut writer = ByteWriter::new();
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u8(image_type::TRUE_COLOR);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u8(0);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u16_le(2);
        writer.write_u16_le(1);
        writer.write_u8(24);
        writer.write_u8(0);
        // BGR: 蓝、绿
        writer.write_bytes(&[255, 0, 0]);
        writer.write_bytes(&[0, 255, 0]);
        writer.into_vec()
    }

    #[test]
    fn sniff_accepts_plausible_header() {
        assert!(TgaCodec.sniff(&build_true_color_bottom_up()));
    }

    #[test]
    fn sniff_rejects_ico_like_header() {
        // ICO 目录头的前 18 字节
        let ico = [
            0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x10, 0x10, 0x00, 0x00, 0x01, 0x00, 0x20, 0x20,
            0x00, 0x00, 0x01, 0x00,
        ];
        assert!(!TgaCodec.sniff(&ico));
    }

    #[test]
    fn sniff_rejects_short_header() {
        assert!(!TgaCodec.sniff(b"\x00\x00\x02\x00"));
    }

    #[test]
    fn bottom_up_rows_are_flipped() {
        let image = decode(&build_true_color_bottom_up()).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(0, 0, 255));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_rgb(0, 255, 0));
    }

    #[test]
    fn right_to_left_descriptor_mirrors_columns() {
        let mut bytes = build_true_color_bottom_up();
        bytes[17] = FLAG_RIGHT_TO_LEFT;
        let image = decode(&bytes).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(0, 255, 0));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_rgb(0, 0, 255));
    }

    #[test]
    fn sixteen_bit_pixels_use_five_bit_channels() {
        let mut writer = ByteWriter::new();
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u8(image_type::TRUE_COLOR);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u8(0);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u16_le(1);
        writer.write_u16_le(1);
        writer.write_u8(16);
        writer.write_u8(0);
        // A1R5G5B5 全红
        writer.write_u16_le(0x7C00);
        let image = decode(&writer.into_vec()).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(255, 0, 0));
    }

    #[test]
    fn color_mapped_palette_is_applied() {
        let mut writer = ByteWriter::new();
        writer.write_u8(0);
        writer.write_u8(1);
        writer.write_u8(image_type::COLOR_MAPPED);
        writer.write_u16_le(0);
        writer.write_u16_le(2);
        writer.write_u8(24);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u16_le(2);
        writer.write_u16_le(1);
        writer.write_u8(8);
        writer.write_u8(FLAG_TOP_TO_BOTTOM);
        // 调色板:红、绿
        writer.write_bytes(&[0, 0, 255]);
        writer.write_bytes(&[0, 255, 0]);
        // 索引 1、0
        writer.write_bytes(&[1, 0]);
        let image = decode(&writer.into_vec()).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(0, 255, 0));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_rgb(255, 0, 0));
    }

    #[test]
    fn rle_run_packet_expands_pixels() {
        let mut writer = ByteWriter::new();
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u8(image_type::TRUE_COLOR_RLE);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u8(0);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u16_le(4);
        writer.write_u16_le(1);
        writer.write_u8(24);
        writer.write_u8(FLAG_TOP_TO_BOTTOM);
        // 游程包:4 个相同的蓝色像素
        writer.write_u8(0x83);
        writer.write_bytes(&[255, 0, 0]);
        let image = decode(&writer.into_vec()).unwrap();
        for x in 0..4 {
            assert_eq!(image.get_pixel(x, 0), Rgba::from_rgb(0, 0, 255));
        }
    }

    #[test]
    fn rle_raw_packet_reads_literal_pixels() {
        let mut writer = ByteWriter::new();
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u8(image_type::TRUE_COLOR_RLE);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u8(0);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u16_le(2);
        writer.write_u16_le(1);
        writer.write_u8(24);
        writer.write_u8(FLAG_TOP_TO_BOTTOM);
        // 原始包:2 个不同的像素
        writer.write_u8(0x01);
        writer.write_bytes(&[255, 0, 0]);
        writer.write_bytes(&[0, 255, 0]);
        let image = decode(&writer.into_vec()).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(0, 0, 255));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_rgb(0, 255, 0));
    }

    #[test]
    fn identifier_field_is_skipped() {
        let mut writer = ByteWriter::new();
        writer.write_u8(4);
        writer.write_u8(0);
        writer.write_u8(image_type::TRUE_COLOR);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u8(0);
        writer.write_u16_le(0);
        writer.write_u16_le(0);
        writer.write_u16_le(1);
        writer.write_u16_le(1);
        writer.write_u8(24);
        writer.write_u8(FLAG_TOP_TO_BOTTOM);
        writer.write_bytes(b"note");
        writer.write_bytes(&[10, 20, 30]);
        let image = decode(&writer.into_vec()).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(30, 20, 10));
    }

    #[test]
    fn zero_size_is_rejected() {
        let mut bytes = build_true_color_bottom_up();
        bytes[12] = 0;
        bytes[13] = 0;
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn truncated_pixels_are_rejected() {
        let mut bytes = build_true_color_bottom_up();
        bytes.truncate(HEADER_LEN + 3);
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn alpha_channel_round_trips() {
        let mut image = Image::new_zeroed(2, 2, ColorType::Rgba8).unwrap();
        image.set_pixel(0, 0, Rgba::new(1, 2, 3, 4));
        image.set_pixel(1, 0, Rgba::new(5, 6, 7, 8));
        image.set_pixel(0, 1, Rgba::new(9, 10, 11, 12));
        image.set_pixel(1, 1, Rgba::new(13, 14, 15, 16));
        let bytes = encode_true_color(&image, true).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.color, ColorType::Rgba8);
        assert_eq!(decoded.data, image.data);
    }

    #[test]
    fn opaque_rgba_is_written_as_24_bit() {
        let image = Image::filled(2, 2, ColorType::Rgba8, Rgba::from_rgb(4, 5, 6)).unwrap();
        let bytes = TgaCodec.encode(&image, &EncodeOptions::default()).unwrap();
        assert_eq!(bytes[16], 24);
        assert_eq!(decode(&bytes).unwrap().get_pixel(1, 1), Rgba::from_rgb(4, 5, 6));
    }

    #[test]
    fn grayscale_with_alpha_round_trips() {
        let image = Image::new(2, 1, ColorType::GrayAlpha8, vec![10, 200, 20, 100]).unwrap();
        let bytes = TgaCodec.encode(&image, &EncodeOptions::default()).unwrap();
        assert_eq!(bytes[16], 16);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.color, ColorType::GrayAlpha8);
        assert_eq!(decoded.data, image.data);
    }

    #[test]
    fn footer_is_written() {
        let image = Image::filled(1, 1, ColorType::Rgb8, Rgba::from_rgb(1, 1, 1)).unwrap();
        let bytes = TgaCodec.encode(&image, &EncodeOptions::default()).unwrap();
        assert_eq!(bytes.len(), HEADER_LEN + 3 + 26);
        assert_eq!(&bytes[bytes.len() - 26..bytes.len() - 8], FOOTER_SIGNATURE);
    }
}
