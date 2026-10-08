//! BMP 位图编解码器。
//!
//! 支持的读取范围:
//! * 文件头 `BITMAPCOREHEADER`(12 字节)与 `BITMAPINFOHEADER` 及以上版本(40~124 字节)
//! * 1 / 4 / 8 位调色板图,16 / 24 / 32 位真彩图
//! * `BI_RGB`、`BI_RLE8`、`BI_RLE4`、`BI_BITFIELDS`、`BI_ALPHABITFIELDS` 五种压缩方式
//! * 正高度(自下而上)与负高度(自上而下)两种行序
//!
//! 写出时统一使用 24 位或 32 位不压缩格式:24 位用于不透明图像,32 位用于含
//! 透明度的图像,灰度图则写成带 256 级灰度调色板的 8 位图。

use crate::core::codec::{Decoder, Encoder};
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;
use crate::core::pixel::{ColorType, Rgba};
use crate::core::registry::Registry;
use crate::util::bytes::{ByteReader, ByteWriter};

/// 注册 BMP 解码器与编码器。
pub fn register(registry: &mut Registry) {
    registry.register_decoder(Box::new(BmpCodec));
    registry.register_encoder(Box::new(BmpCodec));
}

/// 文件头固定长度:14 字节文件头 + 40 字节信息头。
const HEADER_LEN: usize = 54;
/// `BITMAPCOREHEADER` 长度。
const CORE_HEADER_LEN: usize = 12;
/// `BITMAPINFOHEADER` 长度。
const INFO_HEADER_LEN: usize = 40;
/// 位图信息头允许的最大长度(`BITMAPV5HEADER`)。
const MAX_INFO_HEADER_LEN: usize = 124;

/// 压缩方式常量。
mod compression {
    /// 不压缩。
    pub const RGB: u32 = 0;
    /// 8 位游程编码。
    pub const RLE8: u32 = 1;
    /// 4 位游程编码。
    pub const RLE4: u32 = 2;
    /// 通道掩码。
    pub const BITFIELDS: u32 = 3;
    /// 带透明度掩码的通道掩码。
    pub const ALPHA_BITFIELDS: u32 = 6;
}

/// 解析后的 BMP 头信息。
struct BmpHeader {
    /// 像素数据起始偏移。
    data_offset: usize,
    /// 调色板起始偏移。
    palette_offset: usize,
    /// 宽度。
    width: u32,
    /// 高度(始终为正数)。
    height: u32,
    /// 行序是否为自上而下。
    top_down: bool,
    /// 每像素位数。
    bit_count: u16,
    /// 压缩方式。
    compression: u32,
    /// 调色板单项字节数(3 或 4)。
    palette_entry_size: usize,
    /// 文件声明的调色板项数,0 表示使用满调色板。
    colors_used: u32,
    /// 通道掩码:红、绿、蓝、透明度。
    masks: [u32; 4],
}

impl BmpHeader {
    /// 每行字节数(含 4 字节对齐填充)。
    fn stride(&self) -> usize {
        align4((self.width as usize * self.bit_count as usize).div_ceil(8))
    }

    /// 调色板项数。
    fn palette_len(&self) -> usize {
        if self.bit_count > 8 {
            return 0;
        }
        let declared = if self.colors_used > 0 {
            self.colors_used as usize
        } else {
            1usize << self.bit_count
        };
        // 部分文件不填 colors_used,实际只写了用到的那几种颜色,按像素数据起点收窄。
        let available =
            self.data_offset.saturating_sub(self.palette_offset) / self.palette_entry_size.max(1);
        declared.clamp(1, 256).min(available)
    }

    /// 是否使用游程编码。
    fn is_rle(&self) -> bool {
        self.compression == compression::RLE8 || self.compression == compression::RLE4
    }
}

/// BMP 编解码器。
struct BmpCodec;

impl Decoder for BmpCodec {
    fn format(&self) -> Format {
        Format::Bmp
    }

    fn sniff(&self, header: &[u8]) -> bool {
        header.len() >= 2 && &header[..2] == b"BM"
    }

    fn decode(&self, data: &[u8]) -> Result<Image> {
        let header = parse_header(data)?;
        let palette = read_palette(data, &header)?;

        let pixels = if header.is_rle() {
            decode_rle(data, &header, &palette)?
        } else {
            decode_uncompressed(data, &header, &palette)?
        };

        build_image(&header, &palette, pixels)
    }
}

impl Encoder for BmpCodec {
    fn format(&self) -> Format {
        Format::Bmp
    }

    fn encode(&self, image: &Image, _options: &EncodeOptions) -> Result<Vec<u8>> {
        if needs_alpha_channel(image) {
            encode_true_color(image, 32)
        } else if image.color == ColorType::Gray8 {
            encode_gray8(image)
        } else {
            encode_true_color(image, 24)
        }
    }
}

/// 把 `value` 向上对齐到 4 的倍数。
fn align4(value: usize) -> usize {
    (value + 3) & !3
}

/// 判断图像是否真的用到了透明度,避免把全不透明的图写成 32 位。
fn needs_alpha_channel(image: &Image) -> bool {
    image.color.has_alpha() && (0..image.height).any(|y| {
        (0..image.width).any(|x| image.get_pixel(x, y).a != 255)
    })
}

/// 从掩码中提取并缩放到 8 位。
fn extract_channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones();
    let raw = ((value & mask) >> shift) as u64;
    let max = (1u64 << bits) - 1;
    (((raw * 255) + max / 2) / max) as u8
}

/// 解析文件头。
fn parse_header(data: &[u8]) -> Result<BmpHeader> {
    if data.len() < HEADER_LEN || &data[..2] != b"BM" {
        return Err(ConvertError::UnknownFormat);
    }

    let mut reader = ByteReader::new(data);
    reader.skip(2)?;
    let _file_size = reader.read_u32_le()?;
    let _reserved = reader.read_u32_le()?;
    let data_offset = reader.read_u32_le()? as usize;
    let dib_size = reader.read_u32_le()? as usize;

    let mut palette_entry_size = 4usize;
    let mut colors_used = 0u32;
    let mut masks = [0u32; 4];
    let width;
    let height;
    let top_down;
    let bit_count;
    let compression;

    if dib_size == CORE_HEADER_LEN {
        // 早期 12 字节头:宽高均为 16 位无符号数,行序固定自下而上。
        palette_entry_size = 3;
        width = reader.read_u16_le()? as u32;
        height = reader.read_u16_le()? as u32;
        top_down = false;
        let _planes = reader.read_u16_le()?;
        bit_count = reader.read_u16_le()?;
        compression = compression::RGB;
    } else if (INFO_HEADER_LEN..=MAX_INFO_HEADER_LEN).contains(&dib_size) {
        let raw_width = reader.read_i32_le()?;
        let raw_height = reader.read_i32_le()?;
        if raw_width <= 0 || raw_height == 0 {
            return Err(ConvertError::corrupt("BMP 宽高非法"));
        }
        width = raw_width as u32;
        if raw_height < 0 {
            height = raw_height.unsigned_abs();
            top_down = true;
        } else {
            height = raw_height as u32;
            top_down = false;
        }
        let _planes = reader.read_u16_le()?;
        bit_count = reader.read_u16_le()?;
        compression = reader.read_u32_le()?;
        let _image_size = reader.read_u32_le()?;
        let _x_pixels_per_meter = reader.read_i32_le()?;
        let _y_pixels_per_meter = reader.read_i32_le()?;
        colors_used = reader.read_u32_le()?;
        let _important_colors = reader.read_u32_le()?;

        // V4/V5 头把通道掩码放在头部内部,40 字节头则紧跟其后。
        let extension_len = dib_size - INFO_HEADER_LEN;
        let extension: &[u8] = if extension_len > 0 {
            reader.read_bytes(extension_len)?
        } else {
            &[]
        };

        if compression == compression::BITFIELDS || compression == compression::ALPHA_BITFIELDS {
            if extension.len() >= 12 {
                masks[0] = u32::from_le_bytes(extension[0..4].try_into().unwrap());
                masks[1] = u32::from_le_bytes(extension[4..8].try_into().unwrap());
                masks[2] = u32::from_le_bytes(extension[8..12].try_into().unwrap());
                if extension.len() >= 16 {
                    masks[3] = u32::from_le_bytes(extension[12..16].try_into().unwrap());
                }
            } else {
                masks[0] = reader.read_u32_le()?;
                masks[1] = reader.read_u32_le()?;
                masks[2] = reader.read_u32_le()?;
                if compression == compression::ALPHA_BITFIELDS {
                    masks[3] = reader.read_u32_le()?;
                }
            }
        }
    } else {
        return Err(ConvertError::unsupported(format!(
            "BMP 信息头长度为 {dib_size} 字节"
        )));
    }

    if !matches!(bit_count, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err(ConvertError::unsupported(format!(
            "BMP 每像素 {bit_count} 位"
        )));
    }

    // 16 位未声明掩码时按 5-5-5 处理。
    if bit_count == 16 && masks[..3].iter().all(|mask| *mask == 0) {
        masks = [0x7C00, 0x03E0, 0x001F, 0];
    }

    let palette_offset = reader.position();
    if data_offset < palette_offset {
        return Err(ConvertError::corrupt("BMP 像素数据偏移早于调色板"));
    }

    Ok(BmpHeader {
        data_offset,
        palette_offset,
        width,
        height,
        top_down,
        bit_count,
        compression,
        palette_entry_size,
        colors_used,
        masks,
    })
}

/// 读取调色板。
fn read_palette(data: &[u8], header: &BmpHeader) -> Result<Vec<Rgba>> {
    let count = header.palette_len();
    if count == 0 {
        return Ok(Vec::new());
    }

    let start = header.palette_offset;
    let total = count * header.palette_entry_size;
    let end = start
        .checked_add(total)
        .ok_or_else(|| ConvertError::corrupt("BMP 调色板长度溢出"))?;
    if end > data.len() {
        return Err(ConvertError::corrupt("BMP 调色板数据不完整"));
    }

    let mut palette = Vec::with_capacity(count);
    for index in 0..count {
        let base = start + index * header.palette_entry_size;
        let b = data[base];
        let g = data[base + 1];
        let r = data[base + 2];
        // 4 字节调色板的第 4 字节通常保留为 0,不能直接当作透明度,统一按不透明处理。
        palette.push(Rgba::new(r, g, b, 255));
    }
    Ok(palette)
}

/// 取调色板颜色,越界时回退为黑色。
fn palette_color(palette: &[Rgba], index: usize) -> Rgba {
    palette.get(index).copied().unwrap_or(Rgba::from_rgb(0, 0, 0))
}

/// 解码不压缩位图。
fn decode_uncompressed(data: &[u8], header: &BmpHeader, palette: &[Rgba]) -> Result<Vec<Rgba>> {
    let width = header.width as usize;
    let height = header.height as usize;
    let stride = header.stride();
    let needed = stride
        .checked_mul(height)
        .ok_or_else(|| ConvertError::corrupt("BMP 像素数据长度溢出"))?;
    let end = header
        .data_offset
        .checked_add(needed)
        .ok_or_else(|| ConvertError::corrupt("BMP 像素数据长度溢出"))?;
    if end > data.len() {
        return Err(ConvertError::corrupt("BMP 像素数据不完整"));
    }

    let mut pixels = vec![Rgba::TRANSPARENT; width * height];
    let mut row_buffer = vec![0u8; stride];

    for y in 0..height {
        let source_row = if header.top_down { y } else { height - 1 - y };
        let start = header.data_offset + source_row * stride;
        row_buffer.copy_from_slice(&data[start..start + stride]);

        for x in 0..width {
            let pixel = match header.bit_count {
                1 => {
                    let byte = row_buffer[x / 8];
                    let bit = (byte >> (7 - (x % 8))) & 1;
                    palette_color(palette, bit as usize)
                }
                4 => {
                    let byte = row_buffer[x / 2];
                    let index = if x % 2 == 0 { byte >> 4 } else { byte & 0x0F };
                    palette_color(palette, index as usize)
                }
                8 => palette_color(palette, row_buffer[x] as usize),
                16 => {
                    let value = u16::from_le_bytes([row_buffer[x * 2], row_buffer[x * 2 + 1]]) as u32;
                    Rgba::from_rgb(
                        extract_channel(value, header.masks[0]),
                        extract_channel(value, header.masks[1]),
                        extract_channel(value, header.masks[2]),
                    )
                }
                24 => Rgba::from_rgb(
                    row_buffer[x * 3 + 2],
                    row_buffer[x * 3 + 1],
                    row_buffer[x * 3],
                ),
                32 => {
                    let base = x * 4;
                    let b = row_buffer[base];
                    let g = row_buffer[base + 1];
                    let r = row_buffer[base + 2];
                    let a = row_buffer[base + 3];
                    if header.compression == compression::BITFIELDS
                        || header.compression == compression::ALPHA_BITFIELDS
                    {
                        let value =
                            u32::from_le_bytes([b, g, r, a]);
                        Rgba::new(
                            extract_channel(value, header.masks[0]),
                            extract_channel(value, header.masks[1]),
                            extract_channel(value, header.masks[2]),
                            if header.masks[3] != 0 {
                                extract_channel(value, header.masks[3])
                            } else {
                                255
                            },
                        )
                    } else {
                        Rgba::new(r, g, b, a)
                    }
                }
                other => {
                    return Err(ConvertError::unsupported(format!(
                        "BMP 每像素 {other} 位"
                    )))
                }
            };
            pixels[y * width + x] = pixel;
        }
    }

    Ok(pixels)
}

/// 解码 RLE4 / RLE8 位图。
fn decode_rle(data: &[u8], header: &BmpHeader, palette: &[Rgba]) -> Result<Vec<Rgba>> {
    let width = header.width as usize;
    let height = header.height as usize;
    let bits = header.bit_count;
    let mut pixels = vec![Rgba::TRANSPARENT; width * height];

    let mut reader = ByteReader::new(data);
    if header.data_offset > data.len() {
        return Err(ConvertError::corrupt("BMP 像素数据偏移越界"));
    }
    reader.seek(header.data_offset)?;

    // 游程编码的坐标原点在左下角,行的推进方向是自下而上。
    let mut x = 0usize;
    let mut y = 0usize;

    while reader.remaining() >= 2 {
        let count = reader.read_u8()? as usize;
        let value = reader.read_u8()?;

        if count > 0 {
            for offset in 0..count {
                let index = if bits == 8 {
                    value as usize
                } else if offset % 2 == 0 {
                    (value >> 4) as usize
                } else {
                    (value & 0x0F) as usize
                };
                plot(&mut pixels, width, height, x, y, palette_color(palette, index));
                x += 1;
            }
            continue;
        }

        match value {
            // 行结束:回到行首,上移一行。
            0 => {
                x = 0;
                y += 1;
            }
            // 位图结束。
            1 => break,
            // 增量移动。
            2 => {
                let dx = reader.read_u8()? as usize;
                let dy = reader.read_u8()? as usize;
                x += dx;
                y += dy;
            }
            // 绝对模式:后随 `value` 个未压缩像素。
            run => {
                let run = run as usize;
                let bytes_needed = if bits == 8 { run } else { run.div_ceil(2) };
                // 绝对模式的数据按 16 位字对齐。
                let padded = align2(bytes_needed);
                if reader.remaining() < padded {
                    break;
                }
                let chunk = reader.read_bytes(padded)?;
                for offset in 0..run {
                    let index = if bits == 8 {
                        chunk[offset] as usize
                    } else if offset % 2 == 0 {
                        (chunk[offset / 2] >> 4) as usize
                    } else {
                        (chunk[offset / 2] & 0x0F) as usize
                    };
                    plot(&mut pixels, width, height, x, y, palette_color(palette, index));
                    x += 1;
                }
            }
        }
    }

    Ok(pixels)
}

/// 把 `value` 向上对齐到 2 的倍数。
fn align2(value: usize) -> usize {
    (value + 1) & !1
}

/// 把左下角坐标系中的像素写入自上而下的缓冲区。
fn plot(pixels: &mut [Rgba], width: usize, height: usize, x: usize, y: usize, color: Rgba) {
    if x >= width || y >= height {
        return;
    }
    pixels[(height - 1 - y) * width + x] = color;
}

/// 依据位深与内容选择内存布局。
fn choose_color_type(header: &BmpHeader, palette: &[Rgba], pixels: &[Rgba]) -> ColorType {
    match header.bit_count {
        1 | 4 | 8 => {
            let gray = !palette.is_empty()
                && palette
                    .iter()
                    .all(|pixel| pixel.r == pixel.g && pixel.g == pixel.b);
            if gray {
                ColorType::Gray8
            } else {
                ColorType::Rgb8
            }
        }
        32 => {
            // 部分软件写出的 32 位图并未使用透明度通道,此时按不透明处理。
            if pixels.iter().any(|pixel| pixel.a != 0) {
                ColorType::Rgba8
            } else {
                ColorType::Rgb8
            }
        }
        _ => ColorType::Rgb8,
    }
}

/// 把像素序列整理为位图。
fn build_image(header: &BmpHeader, palette: &[Rgba], pixels: Vec<Rgba>) -> Result<Image> {
    let color = choose_color_type(header, palette, &pixels);
    let mut image = Image::new_zeroed(header.width, header.height, color)?;
    for (index, pixel) in pixels.iter().enumerate() {
        let x = (index % header.width as usize) as u32;
        let y = (index / header.width as usize) as u32;
        image.set_pixel(x, y, *pixel);
    }
    Ok(image)
}

/// 写出 24 位或 32 位不压缩位图。
fn encode_true_color(image: &Image, bit_count: u16) -> Result<Vec<u8>> {
    let width = image.width;
    let height = image.height;
    let bytes_per_pixel = (bit_count / 8) as usize;
    let stride = align4(width as usize * bytes_per_pixel);
    let pixel_bytes = stride * height as usize;
    let data_offset = HEADER_LEN;

    let mut writer = ByteWriter::with_capacity(data_offset + pixel_bytes);
    write_headers(
        &mut writer,
        width,
        height,
        bit_count,
        data_offset as u32,
        (data_offset + pixel_bytes) as u32,
    );

    let mut row = vec![0u8; stride];
    for y in (0..height).rev() {
        row.fill(0);
        for x in 0..width {
            let pixel = image.get_pixel(x, y);
            let base = x as usize * bytes_per_pixel;
            if bit_count == 32 {
                row[base] = pixel.b;
                row[base + 1] = pixel.g;
                row[base + 2] = pixel.r;
                row[base + 3] = pixel.a;
            } else {
                row[base] = pixel.b;
                row[base + 1] = pixel.g;
                row[base + 2] = pixel.r;
            }
        }
        writer.write_bytes(&row);
    }

    Ok(writer.into_vec())
}

/// 写出带 256 级灰度调色板的 8 位位图。
fn encode_gray8(image: &Image) -> Result<Vec<u8>> {
    let width = image.width;
    let height = image.height;
    let stride = align4(width as usize);
    let pixel_bytes = stride * height as usize;
    let palette_bytes = 256 * 4;
    let data_offset = HEADER_LEN + palette_bytes;

    let mut writer = ByteWriter::with_capacity(data_offset + pixel_bytes);
    write_headers(
        &mut writer,
        width,
        height,
        8,
        data_offset as u32,
        (data_offset + pixel_bytes) as u32,
    );

    for level in 0..256u32 {
        writer.write_u8(level as u8);
        writer.write_u8(level as u8);
        writer.write_u8(level as u8);
        writer.write_u8(0);
    }

    let mut row = vec![0u8; stride];
    for y in (0..height).rev() {
        row.fill(0);
        for x in 0..width {
            row[x as usize] = image.get_pixel(x, y).luma();
        }
        writer.write_bytes(&row);
    }

    Ok(writer.into_vec())
}

/// 写出文件头与信息头。
fn write_headers(
    writer: &mut ByteWriter,
    width: u32,
    height: u32,
    bit_count: u16,
    data_offset: u32,
    file_size: u32,
) {
    writer.write_bytes(b"BM");
    writer.write_u32_le(file_size);
    writer.write_u32_le(0);
    writer.write_u32_le(data_offset);

    writer.write_u32_le(INFO_HEADER_LEN as u32);
    writer.write_i32_le(width as i32);
    writer.write_i32_le(height as i32);
    writer.write_u16_le(1);
    writer.write_u16_le(bit_count);
    writer.write_u32_le(compression::RGB);
    writer.write_u32_le(0);
    // 每米 2835 像素,约等于 72 DPI。
    writer.write_i32_le(2835);
    writer.write_i32_le(2835);
    writer.write_u32_le(0);
    writer.write_u32_le(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一张 2x2 的 1 位调色板 BMP(黑白各两个像素)。
    fn build_mono_bmp(top_down: bool) -> Vec<u8> {
        let stride = 4usize; // 2 像素 / 8 = 1 字节,补齐到 4 字节
        let data_offset = HEADER_LEN + 8;
        let mut writer = ByteWriter::new();
        writer.write_bytes(b"BM");
        writer.write_u32_le((data_offset + stride * 2) as u32);
        writer.write_u32_le(0);
        writer.write_u32_le(data_offset as u32);
        writer.write_u32_le(INFO_HEADER_LEN as u32);
        writer.write_i32_le(2);
        writer.write_i32_le(if top_down { -2 } else { 2 });
        writer.write_u16_le(1);
        writer.write_u16_le(1);
        writer.write_u32_le(compression::RGB);
        writer.write_u32_le(0);
        writer.write_i32_le(0);
        writer.write_i32_le(0);
        writer.write_u32_le(2);
        writer.write_u32_le(0);
        // 调色板:黑、白
        writer.write_bytes(&[0, 0, 0, 0]);
        writer.write_bytes(&[255, 255, 255, 0]);
        // 第一行(文件中先出现的行):黑 白 -> 0b0100_0000
        writer.write_bytes(&[0x40, 0, 0, 0]);
        // 第二行:白 黑 -> 0b1000_0000
        writer.write_bytes(&[0x80, 0, 0, 0]);
        writer.into_vec()
    }

    #[test]
    fn mono_bottom_up_decodes_correctly() {
        let codec = BmpCodec;
        let image = codec.decode(&build_mono_bmp(false)).unwrap();
        assert_eq!(image.color, ColorType::Gray8);
        assert_eq!(image.get_pixel(0, 0), Rgba::from_gray(255));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_gray(0));
        assert_eq!(image.get_pixel(0, 1), Rgba::from_gray(0));
        assert_eq!(image.get_pixel(1, 1), Rgba::from_gray(255));
    }

    #[test]
    fn mono_top_down_decodes_correctly() {
        let codec = BmpCodec;
        let image = codec.decode(&build_mono_bmp(true)).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_gray(0));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_gray(255));
    }

    #[test]
    fn sniff_requires_bm_magic() {
        let codec = BmpCodec;
        assert!(codec.sniff(b"BM"));
        assert!(!codec.sniff(b"B"));
        assert!(!codec.sniff(b"ZZ"));
    }

    #[test]
    fn truncated_file_reports_error() {
        let codec = BmpCodec;
        let mut data = build_mono_bmp(false);
        data.truncate(HEADER_LEN + 10);
        assert!(codec.decode(&data).is_err());
    }

    #[test]
    fn header_only_file_reports_error() {
        let codec = BmpCodec;
        assert!(codec.decode(b"BM").is_err());
    }

    #[test]
    fn encode_uses_24_bit_for_opaque_images() {
        let image = Image::filled(3, 2, ColorType::Rgb8, Rgba::from_rgb(9, 8, 7)).unwrap();
        let bytes = encode_true_color(&image, 24).unwrap();
        assert_eq!(bytes[28], 24); // 位深字段位于偏移 28
        let decoded = BmpCodec.decode(&bytes).unwrap();
        assert_eq!(decoded.get_pixel(2, 1), Rgba::from_rgb(9, 8, 7));
    }

    #[test]
    fn opaque_rgba_does_not_need_alpha_channel() {
        let image = Image::filled(2, 2, ColorType::Rgba8, Rgba::from_rgb(1, 2, 3)).unwrap();
        assert!(!needs_alpha_channel(&image));
    }

    #[test]
    fn semi_transparent_rgba_needs_alpha_channel() {
        let mut image = Image::new_zeroed(2, 2, ColorType::Rgba8).unwrap();
        image.set_pixel(0, 0, Rgba::new(1, 2, 3, 128));
        assert!(needs_alpha_channel(&image));
    }

    #[test]
    fn gray_image_round_trips_through_palette() {
        let mut image = Image::new_zeroed(3, 2, ColorType::Gray8).unwrap();
        for y in 0..2 {
            for x in 0..3 {
                image.set_pixel(x, y, Rgba::from_gray((x * 40 + y * 10) as u8));
            }
        }
        let bytes = encode_gray8(&image).unwrap();
        let decoded = BmpCodec.decode(&bytes).unwrap();
        assert_eq!(decoded.color, ColorType::Gray8);
        assert_eq!(decoded.data, image.data);
    }

    #[test]
    fn rle8_decodes_runs_and_absolute_mode() {
        let mut writer = ByteWriter::new();
        writer.write_bytes(b"BM");
        writer.write_u32_le(0);
        writer.write_u32_le(0);
        writer.write_u32_le((HEADER_LEN + 16) as u32);
        writer.write_u32_le(INFO_HEADER_LEN as u32);
        writer.write_i32_le(4);
        writer.write_i32_le(1);
        writer.write_u16_le(1);
        writer.write_u16_le(8);
        writer.write_u32_le(compression::RLE8);
        writer.write_u32_le(0);
        writer.write_i32_le(0);
        writer.write_i32_le(0);
        writer.write_u32_le(0);
        writer.write_u32_le(0);
        // 调色板:索引 0 为黑,索引 1 为红,索引 2 为绿,索引 3 为蓝
        writer.write_bytes(&[0, 0, 0, 0]);
        writer.write_bytes(&[0, 0, 255, 0]);
        writer.write_bytes(&[0, 255, 0, 0]);
        writer.write_bytes(&[255, 0, 0, 0]);
        // 编码数据:先 1 个索引 1 的像素,再绝对模式写出 3 个像素
        writer.write_u8(1);
        writer.write_u8(1);
        writer.write_u8(0);
        writer.write_u8(3);
        writer.write_bytes(&[2, 3, 0]);
        writer.write_u8(0);
        writer.write_u8(1);
        let bytes = writer.into_vec();

        let image = BmpCodec.decode(&bytes).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(255, 0, 0));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_rgb(0, 255, 0));
        assert_eq!(image.get_pixel(2, 0), Rgba::from_rgb(0, 0, 255));
        assert_eq!(image.get_pixel(3, 0), Rgba::from_rgb(0, 0, 0));
    }

    #[test]
    fn sixteen_bit_defaults_to_555_masks() {
        let mut writer = ByteWriter::new();
        writer.write_bytes(b"BM");
        writer.write_u32_le(0);
        writer.write_u32_le(0);
        writer.write_u32_le(HEADER_LEN as u32);
        writer.write_u32_le(INFO_HEADER_LEN as u32);
        writer.write_i32_le(1);
        writer.write_i32_le(1);
        writer.write_u16_le(1);
        writer.write_u16_le(16);
        writer.write_u32_le(compression::RGB);
        writer.write_u32_le(0);
        writer.write_i32_le(0);
        writer.write_i32_le(0);
        writer.write_u32_le(0);
        writer.write_u32_le(0);
        // 5-5-5 全红:0b11111_00000_00000,行尾补齐到 4 字节
        writer.write_u16_le(0x7C00);
        writer.write_u16_le(0);

        let image = BmpCodec.decode(&writer.into_vec()).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(255, 0, 0));
    }
}
