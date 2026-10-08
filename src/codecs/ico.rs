//! ICO(Windows 图标)编解码器。
//!
//! 文件由图标目录与若干图像条目组成:
//!
//! ```text
//! ICONDIR       6 字节   reserved(2) + type(2) + count(2)
//! ICONDIRENTRY 16 字节   宽、高、色数、保留、平面数、位深、数据长度、数据偏移
//! 图像数据              BMP 的 DIB 结构(不含 14 字节文件头)或 PNG 数据流
//! ```
//!
//! 条目中的 DIB 高度是实际高度的两倍,下半部分是 XOR 位图,上半部分是 1 位的
//! AND 掩码。本模块只处理 DIB 条目;PNG 条目需要自研 PNG 解码器,当前跳过。

use crate::core::codec::{Decoder, Encoder};
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;
use crate::core::pixel::{ColorType, Rgba};
use crate::core::registry::Registry;
use crate::util::bytes::{ByteReader, ByteWriter};

/// 图标目录长度。
const ICONDIR_LEN: usize = 6;
/// 单个目录项长度。
const ICONDIRENTRY_LEN: usize = 16;
/// DIB 至少要有 BITMAPINFOHEADER 这么长。
const BITMAPINFOHEADER_LEN: usize = 40;
/// ICO 允许的最大边长。
const MAX_DIMENSION: u32 = 256;

/// ICO 容器允许的最大边长,供转换管线做编码前检查。
pub const ICO_MAX_DIMENSION: u32 = MAX_DIMENSION;

/// PNG 数据流的签名,用来识别 PNG 条目。
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// 压缩方式:不压缩。
const BI_RGB: u32 = 0;
/// 压缩方式:位域掩码。
const BI_BITFIELDS: u32 = 3;
/// 压缩方式:带透明度的位域掩码。
const BI_ALPHABITFIELDS: u32 = 6;

/// 注册 ICO 解码器与编码器。
pub fn register(registry: &mut Registry) {
    registry.register_decoder(Box::new(IcoCodec));
    registry.register_encoder(Box::new(IcoCodec));
}

/// 目录项。
#[derive(Clone, Copy, Debug)]
struct IconEntry {
    /// 实际宽度,0 表示 256。
    width: u32,
    /// 实际高度,0 表示 256。
    height: u32,
    /// 位深。
    bit_count: u16,
    /// 数据长度。
    bytes: usize,
    /// 数据偏移。
    offset: usize,
}

impl IconEntry {
    /// 条目覆盖的像素数,用于挑选最佳条目。
    fn area(&self) -> u32 {
        self.width.saturating_mul(self.height)
    }
}

/// 解析后的 DIB 头部信息。
struct DibHeader {
    width: u32,
    height: u32,
    top_down: bool,
    bit_count: u16,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    alpha_mask: u32,
    palette_offset: usize,
    palette_count: usize,
    pixels_offset: usize,
}

/// ICO 编解码器。
struct IcoCodec;

impl Decoder for IcoCodec {
    fn format(&self) -> Format {
        Format::Ico
    }

    fn sniff(&self, header: &[u8]) -> bool {
        header.len() >= ICONDIR_LEN
            && header[0] == 0
            && header[1] == 0
            && matches!(header[2], 1 | 2)
            && header[3] == 0
            && u16::from_le_bytes([header[4], header[5]]) > 0
    }

    fn decode(&self, data: &[u8]) -> Result<Image> {
        let mut reader = ByteReader::new(data);
        let reserved = reader.read_u16_le()?;
        let kind = reader.read_u16_le()?;
        let count = reader.read_u16_le()? as usize;

        if reserved != 0 {
            return Err(ConvertError::corrupt("ICO 保留字段非 0"));
        }
        if kind == 2 {
            return Err(ConvertError::unsupported("ICO 光标文件"));
        }
        if kind != 1 {
            return Err(ConvertError::unsupported(format!("ICO 类型 {kind}")));
        }
        if count == 0 {
            return Err(ConvertError::corrupt("ICO 不含任何图像条目"));
        }

        let entries = read_entries(data, count)?;
        let entry = select_entry(&entries, data)?;
        let dib = data
            .get(entry.offset..entry.offset + entry.bytes)
            .ok_or_else(|| ConvertError::corrupt("ICO 条目数据越界"))?;
        decode_dib(dib)
    }
}

impl Encoder for IcoCodec {
    fn format(&self) -> Format {
        Format::Ico
    }

    fn encode(&self, image: &Image, _options: &EncodeOptions) -> Result<Vec<u8>> {
        encode_icon(image)
    }
}

/// 读取全部目录项。
fn read_entries(data: &[u8], count: usize) -> Result<Vec<IconEntry>> {
    let needed = ICONDIR_LEN + count * ICONDIRENTRY_LEN;
    if data.len() < needed {
        return Err(ConvertError::corrupt("ICO 目录不完整"));
    }

    let mut entries = Vec::with_capacity(count);
    let mut reader = ByteReader::new(&data[ICONDIR_LEN..needed]);
    for _ in 0..count {
        let width = dimension_from_byte(reader.read_u8()?);
        let height = dimension_from_byte(reader.read_u8()?);
        let _color_count = reader.read_u8()?;
        let _reserved = reader.read_u8()?;
        let _planes = reader.read_u16_le()?;
        let bit_count = reader.read_u16_le()?;
        let bytes = reader.read_u32_le()? as usize;
        let offset = reader.read_u32_le()? as usize;
        if offset < needed || offset + bytes > data.len() {
            return Err(ConvertError::corrupt("ICO 条目指向的数据越界"));
        }
        entries.push(IconEntry {
            width,
            height,
            bit_count,
            bytes,
            offset,
        });
    }
    Ok(entries)
}

/// 目录项中的 0 表示 256。
fn dimension_from_byte(value: u8) -> u32 {
    if value == 0 {
        256
    } else {
        value as u32
    }
}

/// 挑选面积最大的 DIB 条目,PNG 条目在自研 PNG 解码器就绪前跳过。
fn select_entry(entries: &[IconEntry], data: &[u8]) -> Result<IconEntry> {
    let mut best: Option<IconEntry> = None;
    let mut saw_png = false;

    for entry in entries {
        let head = &data[entry.offset..entry.offset + entry.bytes];
        if head.len() >= PNG_SIGNATURE.len() && head[..PNG_SIGNATURE.len()] == PNG_SIGNATURE {
            saw_png = true;
            continue;
        }
        if head.len() < BITMAPINFOHEADER_LEN {
            continue;
        }
        let better = match best {
            None => true,
            // 面积优先,面积相同再取位深更高的条目。
            Some(current) => match entry.area().cmp(&current.area()) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Equal => entry.bit_count > current.bit_count,
                std::cmp::Ordering::Less => false,
            },
        };
        if better {
            best = Some(*entry);
        }
    }

    match best {
        Some(entry) => Ok(entry),
        None if saw_png => Err(ConvertError::unsupported("ICO 仅含 PNG 条目")),
        None => Err(ConvertError::corrupt("ICO 中没有可解析的图像条目")),
    }
}

/// 向上取整到 4 的倍数。
fn align4(value: usize) -> usize {
    (value + 3) & !3
}

/// 按掩码取出通道值并线性拉伸到 0-255。
fn extract_channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 255;
    }
    let shift = mask.trailing_zeros();
    let max = (mask >> shift) as u64;
    if max == 0 {
        return 255;
    }
    let raw = ((value & mask) >> shift) as u64;
    ((raw * 255 + max / 2) / max) as u8
}

/// 解析 DIB 头部。
fn parse_dib_header(dib: &[u8]) -> Result<DibHeader> {
    if dib.len() < BITMAPINFOHEADER_LEN {
        return Err(ConvertError::corrupt("ICO 的 DIB 头部不完整"));
    }
    let mut reader = ByteReader::new(dib);
    let header_len = reader.read_u32_le()? as usize;
    let width = reader.read_i32_le()?;
    let raw_height = reader.read_i32_le()?;
    let _planes = reader.read_u16_le()?;
    let bit_count = reader.read_u16_le()?;
    let compression = reader.read_u32_le()?;
    let _image_size = reader.read_u32_le()?;
    let _x_ppm = reader.read_i32_le()?;
    let _y_ppm = reader.read_i32_le()?;
    let color_used = reader.read_u32_le()?;
    let _color_important = reader.read_u32_le()?;

    if header_len < BITMAPINFOHEADER_LEN || header_len > dib.len() {
        return Err(ConvertError::corrupt("ICO 的 DIB 头部长度异常"));
    }
    if width <= 0 || raw_height == 0 {
        return Err(ConvertError::corrupt("ICO 的 DIB 尺寸无效"));
    }
    if !matches!(bit_count, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err(ConvertError::unsupported(format!("ICO 位深 {bit_count}")));
    }
    if !matches!(compression, BI_RGB | BI_BITFIELDS | BI_ALPHABITFIELDS) {
        return Err(ConvertError::unsupported(format!(
            "ICO 压缩方式 {compression}"
        )));
    }

    let top_down = raw_height < 0;
    let total_height = raw_height.unsigned_abs();
    if total_height % 2 != 0 {
        return Err(ConvertError::corrupt("ICO 的 DIB 高度应为实际高度的两倍"));
    }
    let height = total_height / 2;
    let width = width as u32;

    let (mut red_mask, mut green_mask, mut blue_mask, mut alpha_mask) = default_masks(bit_count);
    let mut cursor = header_len;

    if matches!(compression, BI_BITFIELDS | BI_ALPHABITFIELDS) {
        if header_len >= 52 {
            // BITMAPV4/V5 头把掩码内嵌在头部里。
            red_mask = read_u32_at(dib, 40)?;
            green_mask = read_u32_at(dib, 44)?;
            blue_mask = read_u32_at(dib, 48)?;
            if header_len >= 56 {
                alpha_mask = read_u32_at(dib, 52)?;
            }
        } else {
            let mask_bytes = if compression == BI_ALPHABITFIELDS { 16 } else { 12 };
            if dib.len() < cursor + mask_bytes {
                return Err(ConvertError::corrupt("ICO 位域掩码不完整"));
            }
            red_mask = read_u32_at(dib, cursor)?;
            green_mask = read_u32_at(dib, cursor + 4)?;
            blue_mask = read_u32_at(dib, cursor + 8)?;
            if mask_bytes == 16 {
                alpha_mask = read_u32_at(dib, cursor + 12)?;
            }
            cursor += mask_bytes;
        }
    }

    let palette_count = if bit_count <= 8 {
        if color_used != 0 {
            color_used as usize
        } else {
            1usize << bit_count
        }
    } else {
        0
    };
    let palette_offset = cursor;
    cursor += palette_count * 4;

    Ok(DibHeader {
        width,
        height,
        top_down,
        bit_count,
        red_mask,
        green_mask,
        blue_mask,
        alpha_mask,
        palette_offset,
        palette_count,
        pixels_offset: cursor,
    })
}

/// 读取指定偏移处的 32 位小端整数。
fn read_u32_at(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| ConvertError::corrupt("ICO 数据越界"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// 各种位深的默认通道掩码。
fn default_masks(bit_count: u16) -> (u32, u32, u32, u32) {
    match bit_count {
        16 => (0x7C00, 0x03E0, 0x001F, 0),
        32 => (0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000),
        _ => (0, 0, 0, 0),
    }
}

/// 解码 DIB 数据。
fn decode_dib(dib: &[u8]) -> Result<Image> {
    let header = parse_dib_header(dib)?;
    let width = header.width;
    let height = header.height;

    let palette = read_palette(dib, &header)?;

    let bytes_per_pixel = (header.bit_count as usize).div_ceil(8);
    let xor_stride = align4(width as usize * bytes_per_pixel);
    let xor_size = xor_stride
        .checked_mul(height as usize)
        .ok_or_else(|| ConvertError::corrupt("ICO 位图过大"))?;
    let and_stride = align4((width as usize).div_ceil(8));
    let and_offset = header.pixels_offset + xor_size;

    let xor = dib
        .get(header.pixels_offset..header.pixels_offset + xor_size)
        .ok_or_else(|| ConvertError::corrupt("ICO 的 XOR 位图不完整"))?;
    // 少数 32 位图标省略了 AND 掩码,此时按全不透明处理。
    let and = dib
        .get(and_offset..and_offset + and_stride * height as usize)
        .unwrap_or(&[]);

    let mut pixels: Vec<Rgba> = Vec::with_capacity((width * height) as usize);
    for row in 0..height as usize {
        for column in 0..width as usize {
            let base = row * xor_stride + column * bytes_per_pixel;
            let value = read_pixel_value(&xor[base..base + bytes_per_pixel]);
            let pixel = match header.bit_count {
                32 => {
                    let alpha = if header.alpha_mask != 0 {
                        extract_channel(value, header.alpha_mask)
                    } else {
                        (value >> 24) as u8
                    };
                    Rgba::new(
                        extract_channel(value, header.red_mask),
                        extract_channel(value, header.green_mask),
                        extract_channel(value, header.blue_mask),
                        alpha,
                    )
                }
                24 => Rgba::from_rgb(
                    (value >> 16) as u8,
                    (value >> 8) as u8,
                    value as u8,
                ),
                16 => Rgba::new(
                    extract_channel(value, header.red_mask),
                    extract_channel(value, header.green_mask),
                    extract_channel(value, header.blue_mask),
                    if header.alpha_mask != 0 {
                        extract_channel(value, header.alpha_mask)
                    } else {
                        255
                    },
                ),
                8 => palette
                    .get(value as usize)
                    .copied()
                    .ok_or_else(|| ConvertError::corrupt("ICO 调色板索引越界"))?,
                4 => {
                    let index = ((value >> ((column % 2) * 4)) & 0x0F) as usize;
                    palette
                        .get(index)
                        .copied()
                        .ok_or_else(|| ConvertError::corrupt("ICO 调色板索引越界"))?
                }
                _ => {
                    let index = ((value >> (7 - column % 8)) & 0x01) as usize;
                    palette
                        .get(index)
                        .copied()
                        .ok_or_else(|| ConvertError::corrupt("ICO 调色板索引越界"))?
                }
            };
            pixels.push(pixel);
        }
    }

    // 32 位图标若真正写入了透明度,则以该通道为准,否则回退到 AND 掩码。
    let alpha_present = header.bit_count == 32 && pixels.iter().any(|pixel| pixel.a != 0);
    if !alpha_present {
        apply_and_mask(&mut pixels, and, width, height, and_stride);
    }

    build_image(&header, pixels)
}

/// 把若干字节按小端拼成像素值。
fn read_pixel_value(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .enumerate()
        .fold(0u32, |acc, (shift, byte)| acc | ((*byte as u32) << (shift * 8)))
}

/// 读取调色板。
fn read_palette(dib: &[u8], header: &DibHeader) -> Result<Vec<Rgba>> {
    if header.palette_count == 0 {
        return Ok(Vec::new());
    }
    let start = header.palette_offset;
    let end = start + header.palette_count * 4;
    if end > dib.len() {
        return Err(ConvertError::corrupt("ICO 调色板不完整"));
    }
    let mut palette = Vec::with_capacity(header.palette_count);
    for index in 0..header.palette_count {
        let base = start + index * 4;
        palette.push(Rgba::from_rgb(dib[base + 2], dib[base + 1], dib[base]));
    }
    Ok(palette)
}

/// 用 AND 掩码补齐透明度,掩码位为 1 表示该像素完全透明。
fn apply_and_mask(
    pixels: &mut [Rgba],
    and: &[u8],
    width: u32,
    height: u32,
    and_stride: usize,
) {
    if and.is_empty() {
        return;
    }
    for row in 0..height as usize {
        for column in 0..width as usize {
            let byte = and[row * and_stride + column / 8];
            let transparent = (byte >> (7 - column % 8)) & 0x01 == 1;
            let index = row * width as usize + column;
            pixels[index].a = if transparent { 0 } else { 255 };
        }
    }
}

/// 依据像素内容选择最合适的颜色类型并填充位图。
fn build_image(header: &DibHeader, pixels: Vec<Rgba>) -> Result<Image> {
    let has_alpha = pixels.iter().any(|pixel| pixel.a != 255);
    let color = if has_alpha {
        ColorType::Rgba8
    } else {
        ColorType::Rgb8
    };
    let mut image = Image::new_zeroed(header.width, header.height, color)?;

    for (index, pixel) in pixels.iter().enumerate() {
        let row = (index / header.width as usize) as u32;
        let column = (index % header.width as usize) as u32;
        let y = if header.top_down {
            row
        } else {
            header.height - 1 - row
        };
        image.set_pixel(column, y, *pixel);
    }

    Ok(image)
}

/// 写出一个 32 位 DIB 条目。
fn encode_icon(image: &Image) -> Result<Vec<u8>> {
    if image.width == 0 || image.height == 0 {
        return Err(ConvertError::corrupt("图标尺寸不能为 0"));
    }
    if image.width > MAX_DIMENSION || image.height > MAX_DIMENSION {
        return Err(ConvertError::unsupported("ICO 单个条目最大支持 256×256"));
    }

    let width = image.width as usize;
    let height = image.height as usize;
    let xor_stride = align4(width * 4);
    let and_stride = align4(width.div_ceil(8));
    let dib_len = BITMAPINFOHEADER_LEN + xor_stride * height + and_stride * height;

    let mut dib = ByteWriter::with_capacity(dib_len);
    dib.write_u32_le(BITMAPINFOHEADER_LEN as u32);
    dib.write_i32_le(image.width as i32);
    // DIB 高度是实际高度的两倍:下半部分为 XOR 位图,上半部分为 AND 掩码。
    dib.write_i32_le((image.height * 2) as i32);
    dib.write_u16_le(1);
    dib.write_u16_le(32);
    dib.write_u32_le(BI_RGB);
    dib.write_u32_le((xor_stride * height + and_stride * height) as u32);
    dib.write_i32_le(0);
    dib.write_i32_le(0);
    dib.write_u32_le(0);
    dib.write_u32_le(0);

    // XOR 位图自下而上排列。
    for y in (0..image.height).rev() {
        let row_start = dib.len();
        for x in 0..image.width {
            let pixel = image.get_pixel(x, y);
            dib.write_u8(pixel.b);
            dib.write_u8(pixel.g);
            dib.write_u8(pixel.r);
            dib.write_u8(pixel.a);
        }
        let written = dib.len() - row_start;
        dib.write_zeros(xor_stride - written);
    }
    // AND 掩码全为 0,透明度完全交给 32 位通道。
    dib.write_zeros(and_stride * height);

    let dib = dib.into_vec();
    let data_offset = ICONDIR_LEN + ICONDIRENTRY_LEN;

    let mut writer = ByteWriter::with_capacity(data_offset + dib.len());
    writer.write_u16_le(0);
    writer.write_u16_le(1);
    writer.write_u16_le(1);
    writer.write_u8(dimension_to_byte(image.width));
    writer.write_u8(dimension_to_byte(image.height));
    writer.write_u8(0);
    writer.write_u8(0);
    writer.write_u16_le(1);
    writer.write_u16_le(32);
    writer.write_u32_le(dib.len() as u32);
    writer.write_u32_le(data_offset as u32);
    writer.write_bytes(&dib);
    Ok(writer.into_vec())
}

/// 256 在目录项中写作 0。
fn dimension_to_byte(value: u32) -> u8 {
    if value >= 256 {
        0
    } else {
        value as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> Result<Image> {
        IcoCodec.decode(bytes)
    }

    /// 手工拼一个单条目 32 位 ICO。
    fn build_32bit_icon(pixels: &[[u8; 4]], width: usize, height: usize) -> Vec<u8> {
        let xor_stride = align4(width * 4);
        let and_stride = align4(width.div_ceil(8));
        let mut dib = ByteWriter::new();
        dib.write_u32_le(40);
        dib.write_i32_le(width as i32);
        dib.write_i32_le((height * 2) as i32);
        dib.write_u16_le(1);
        dib.write_u16_le(32);
        dib.write_u32_le(BI_RGB);
        dib.write_u32_le(0);
        dib.write_i32_le(0);
        dib.write_i32_le(0);
        dib.write_u32_le(0);
        dib.write_u32_le(0);
        for row in (0..height).rev() {
            for column in 0..width {
                let pixel = pixels[row * width + column];
                dib.write_bytes(&pixel);
            }
            dib.write_zeros(xor_stride - width * 4);
        }
        dib.write_zeros(and_stride * height);
        let dib = dib.into_vec();

        let mut writer = ByteWriter::new();
        writer.write_u16_le(0);
        writer.write_u16_le(1);
        writer.write_u16_le(1);
        writer.write_u8(width as u8);
        writer.write_u8(height as u8);
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u16_le(1);
        writer.write_u16_le(32);
        writer.write_u32_le(dib.len() as u32);
        writer.write_u32_le(22);
        writer.write_bytes(&dib);
        writer.into_vec()
    }

    #[test]
    fn sniff_recognises_icon_header() {
        assert!(IcoCodec.sniff(&build_32bit_icon(&[[0, 0, 0, 255]], 1, 1)));
    }

    #[test]
    fn sniff_rejects_reserved_field() {
        let mut bytes = build_32bit_icon(&[[0, 0, 0, 255]], 1, 1);
        bytes[0] = 7;
        assert!(!IcoCodec.sniff(&bytes));
    }

    #[test]
    fn sniff_rejects_empty_directory() {
        let bytes = [0u8, 0, 1, 0, 0, 0];
        assert!(!IcoCodec.sniff(&bytes));
    }

    #[test]
    fn thirty_two_bit_pixels_keep_alpha() {
        let pixels = [[1, 2, 3, 4], [5, 6, 7, 8]];
        let image = decode(&build_32bit_icon(&pixels, 2, 1)).unwrap();
        assert_eq!(image.color, ColorType::Rgba8);
        assert_eq!(image.get_pixel(0, 0), Rgba::new(3, 2, 1, 4));
        assert_eq!(image.get_pixel(1, 0), Rgba::new(7, 6, 5, 8));
    }

    #[test]
    fn rows_are_flipped_from_bottom_up_order() {
        // 文件里的第一行是图像的最后一行,解码后应当还原成图像顺序
        let pixels = [[10, 0, 0, 255], [20, 0, 0, 255]];
        let image = decode(&build_32bit_icon(&pixels, 1, 2)).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(0, 0, 10));
        assert_eq!(image.get_pixel(0, 1), Rgba::from_rgb(0, 0, 20));
    }

    #[test]
    fn zero_alpha_falls_back_to_and_mask() {
        let mut bytes = build_32bit_icon(&[[9, 9, 9, 0], [9, 9, 9, 0]], 2, 1);
        // AND 掩码位于 XOR 位图之后:首行首字节的最高位对应第一个像素
        let and_offset = 22 + 40 + 8;
        bytes[and_offset] = 0x80;
        let image = decode(&bytes).unwrap();
        assert_eq!(image.color, ColorType::Rgba8);
        assert_eq!(image.get_pixel(0, 0).a, 0);
        assert_eq!(image.get_pixel(1, 0).a, 255);
    }

    #[test]
    fn four_bit_palette_is_decoded() {
        let mut dib = ByteWriter::new();
        dib.write_u32_le(40);
        dib.write_i32_le(2);
        dib.write_i32_le(2);
        dib.write_u16_le(1);
        dib.write_u16_le(4);
        dib.write_u32_le(BI_RGB);
        dib.write_u32_le(0);
        dib.write_i32_le(0);
        dib.write_i32_le(0);
        // 只用到两种颜色,显式声明以缩小调色板
        dib.write_u32_le(2);
        dib.write_u32_le(0);
        // 调色板:红、绿
        dib.write_bytes(&[0, 0, 255, 0]);
        dib.write_bytes(&[0, 255, 0, 0]);
        // 一行,两个像素各占半个字节
        dib.write_bytes(&[0x01, 0, 0, 0]);
        let dib = dib.into_vec();

        let mut writer = ByteWriter::new();
        writer.write_u16_le(0);
        writer.write_u16_le(1);
        writer.write_u16_le(1);
        writer.write_u8(2);
        writer.write_u8(1);
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u16_le(1);
        writer.write_u16_le(4);
        writer.write_u32_le(dib.len() as u32);
        writer.write_u32_le(22);
        writer.write_bytes(&dib);

        let image = decode(&writer.into_vec()).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(0, 255, 0));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_rgb(255, 0, 0));
    }

    #[test]
    fn sixteen_bit_bitfields_are_honoured() {
        let mut dib = ByteWriter::new();
        dib.write_u32_le(40);
        dib.write_i32_le(1);
        dib.write_i32_le(2);
        dib.write_u16_le(1);
        dib.write_u16_le(16);
        dib.write_u32_le(BI_BITFIELDS);
        dib.write_u32_le(0);
        dib.write_i32_le(0);
        dib.write_i32_le(0);
        dib.write_u32_le(0);
        dib.write_u32_le(0);
        dib.write_u32_le(0x7C00);
        dib.write_u32_le(0x03E0);
        dib.write_u32_le(0x001F);
        // 纯绿
        dib.write_u16_le(0x03E0);
        dib.write_zeros(2);
        dib.write_zeros(4);
        let dib = dib.into_vec();

        let mut writer = ByteWriter::new();
        writer.write_u16_le(0);
        writer.write_u16_le(1);
        writer.write_u16_le(1);
        writer.write_u8(1);
        writer.write_u8(1);
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u16_le(1);
        writer.write_u16_le(16);
        writer.write_u32_le(dib.len() as u32);
        writer.write_u32_le(22);
        writer.write_bytes(&dib);

        let image = decode(&writer.into_vec()).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(0, 255, 0));
    }

    #[test]
    fn cursor_files_are_rejected() {
        let mut bytes = build_32bit_icon(&[[0, 0, 0, 255]], 1, 1);
        bytes[2] = 2;
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn png_entries_are_skipped_in_favour_of_dib_entries() {
        // 第一个条目是 PNG(面积更大),第二个是可解析的 32 位 DIB
        let dib_icon = build_32bit_icon(&[[1, 1, 1, 255]], 1, 1);
        let dib = &dib_icon[22..];

        let mut entries = ByteWriter::new();
        // PNG 条目:声明 256×256,面积远大于 DIB 条目
        entries.write_u8(0);
        entries.write_u8(0);
        entries.write_u8(0);
        entries.write_u8(0);
        entries.write_u16_le(1);
        entries.write_u16_le(32);
        entries.write_u32_le(24);
        entries.write_u32_le(38);
        // DIB 条目
        entries.write_u8(1);
        entries.write_u8(1);
        entries.write_u8(0);
        entries.write_u8(0);
        entries.write_u16_le(1);
        entries.write_u16_le(32);
        entries.write_u32_le(dib.len() as u32);
        entries.write_u32_le(62);
        entries.write_bytes(&PNG_SIGNATURE);
        entries.write_zeros(16);
        entries.write_bytes(dib);

        let mut bytes = dib_icon[..6].to_vec();
        bytes[4] = 2;
        bytes.extend_from_slice(&entries.into_vec());

        let image = decode(&bytes).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(1, 1, 1));
    }

    #[test]
    fn zero_entry_count_is_rejected() {
        let mut bytes = build_32bit_icon(&[[0, 0, 0, 255]], 1, 1);
        bytes[4] = 0;
        bytes[5] = 0;
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn truncated_directory_is_rejected() {
        let bytes = [0u8, 0, 1, 0, 1, 0, 1];
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn png_only_icons_report_unsupported() {
        let mut writer = ByteWriter::new();
        writer.write_u16_le(0);
        writer.write_u16_le(1);
        writer.write_u16_le(1);
        writer.write_u8(4);
        writer.write_u8(4);
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u16_le(1);
        writer.write_u16_le(32);
        writer.write_u32_le(24);
        writer.write_u32_le(22);
        writer.write_bytes(&PNG_SIGNATURE);
        writer.write_zeros(16);
        let error = decode(&writer.into_vec()).unwrap_err();
        assert!(matches!(error, ConvertError::UnsupportedFeature(_)));
    }

    #[test]
    fn encode_writes_a_readable_single_entry_icon() {
        let mut image = Image::new_zeroed(3, 2, ColorType::Rgba8).unwrap();
        image.set_pixel(0, 0, Rgba::new(1, 2, 3, 255));
        image.set_pixel(2, 1, Rgba::new(4, 5, 6, 200));
        let bytes = IcoCodec.encode(&image, &EncodeOptions::default()).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 1);

        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.color, ColorType::Rgba8);
        assert_eq!(decoded.to_rgba().data, image.to_rgba().data);
    }

    #[test]
    fn oversize_images_are_rejected() {
        let image = Image::new_zeroed(300, 2, ColorType::Rgb8).unwrap();
        assert!(IcoCodec.encode(&image, &EncodeOptions::default()).is_err());
    }
}
