//! Netpbm 家族编解码器。
//!
//! Netpbm 是一组结构极简的图像格式,常用于科研与图像处理教学:
//!
//! | 魔数 | 名称 | 说明                       |
//! |------|------|----------------------------|
//! | P1   | PBM  | ASCII 双色位图             |
//! | P2   | PGM  | ASCII 灰度图               |
//! | P3   | PPM  | ASCII 真彩图               |
//! | P4   | PBM  | 二进制双色位图             |
//! | P5   | PGM  | 二进制灰度图               |
//! | P6   | PPM  | 二进制真彩图               |
//! | P7   | PAM  | 二进制任意通道图           |
//!
//! 读取时全部支持,写出时按内存布局选择 P5(灰度)、P6(真彩)或 P7(带透明度)。
//! 头部解析需要处理 `#` 注释与任意空白字符,二进制数据前只允许出现一个空白符。

use crate::core::codec::{Decoder, Encoder};
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;
use crate::core::pixel::{ColorType, Rgba};
use crate::core::registry::Registry;
use crate::util::bytes::ByteWriter;

/// 采样值上限。
const MAX_SAMPLE: u32 = 65535;

/// 注册 Netpbm 解码器与编码器。
pub fn register(registry: &mut Registry) {
    registry.register_decoder(Box::new(NetpbmCodec));
    registry.register_encoder(Box::new(NetpbmCodec));
}

/// Netpbm 编解码器。
struct NetpbmCodec;

impl Decoder for NetpbmCodec {
    fn format(&self) -> Format {
        Format::Netpbm
    }

    fn sniff(&self, header: &[u8]) -> bool {
        header.len() >= 2 && header[0] == b'P' && matches!(header[1], b'1'..=b'7')
    }

    fn decode(&self, data: &[u8]) -> Result<Image> {
        if !self.sniff(data) {
            return Err(ConvertError::UnknownFormat);
        }
        let kind = data[1];

        if kind == b'7' {
            return decode_pam(data);
        }

        let mut tokenizer = Tokenizer::new(data);
        let _magic = tokenizer.next_token()?;
        let width = tokenizer.next_u32()?;
        let height = tokenizer.next_u32()?;

        match kind {
            b'1' | b'4' => decode_bilevel(&mut tokenizer, width, height, kind == b'4'),
            b'2' | b'5' => decode_graymap(&mut tokenizer, width, height, kind == b'5'),
            b'3' | b'6' => decode_pixmap(&mut tokenizer, width, height, kind == b'6'),
            other => Err(ConvertError::unsupported(format!(
                "Netpbm 子格式 P{}",
                other as char
            ))),
        }
    }
}

impl Encoder for NetpbmCodec {
    fn format(&self) -> Format {
        Format::Netpbm
    }

    fn extension_for(&self, image: &Image) -> &'static str {
        // Netpbm 是"一族"格式,按实际颜色模式选择最贴切的子类型扩展名。
        match image.color {
            ColorType::Gray8 => "pgm",
            ColorType::Rgb8 => "ppm",
            ColorType::GrayAlpha8 | ColorType::Rgba8 => "pam",
        }
    }

    fn encode(&self, image: &Image, _options: &EncodeOptions) -> Result<Vec<u8>> {
        match image.color {
            ColorType::Gray8 => encode_gray(image),
            ColorType::Rgb8 => encode_rgb(image),
            ColorType::GrayAlpha8 => encode_pam(image, 2, "GRAYSCALE_ALPHA"),
            ColorType::Rgba8 => encode_pam(image, 4, "RGB_ALPHA"),
        }
    }
}

/// 判断字节是否为空白符。
fn is_blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
}

/// 把字符串形式的无符号整数解析为 `u32`。
fn parse_u32(token: &[u8]) -> Option<u32> {
    std::str::from_utf8(token).ok()?.trim().parse().ok()
}

/// 校验宽高合法性。
fn validate_size(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(ConvertError::corrupt("Netpbm 宽高不能为 0"));
    }
    (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| ConvertError::corrupt("Netpbm 尺寸过大"))
}

/// 校验采样值上限。
fn validate_maxval(maxval: u32) -> Result<()> {
    if maxval == 0 || maxval > MAX_SAMPLE {
        return Err(ConvertError::corrupt(format!("Netpbm 采样上限为 {maxval}")));
    }
    Ok(())
}

/// 把 `0..=maxval` 的采样值线性映射到 `0..=255`。
fn scale_sample(value: u32, maxval: u32) -> u8 {
    ((value * 255 + maxval / 2) / maxval).min(255) as u8
}

/// 按空白与 `#` 注释切分头部字段的游标。
struct Tokenizer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    /// 在给定数据上创建游标。
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// 跳过空白符与注释行。
    fn skip_blanks(&mut self) {
        loop {
            while self.pos < self.data.len() && is_blank(self.data[self.pos]) {
                self.pos += 1;
            }
            if self.pos < self.data.len() && self.data[self.pos] == b'#' {
                while self.pos < self.data.len() && self.data[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
    }

    /// 读取下一个字段。
    fn next_token(&mut self) -> Result<&'a [u8]> {
        self.skip_blanks();
        if self.pos >= self.data.len() {
            return Err(ConvertError::corrupt("Netpbm 头部数据不完整"));
        }
        let start = self.pos;
        while self.pos < self.data.len()
            && !is_blank(self.data[self.pos])
            && self.data[self.pos] != b'#'
        {
            self.pos += 1;
        }
        Ok(&self.data[start..self.pos])
    }

    /// 读取下一个字段并解析为整数。
    fn next_u32(&mut self) -> Result<u32> {
        let token = self.next_token()?;
        parse_u32(token).ok_or_else(|| {
            ConvertError::corrupt(format!(
                "Netpbm 数值字段非法:{}",
                String::from_utf8_lossy(token)
            ))
        })
    }

    /// 跳过一个空白符,用于定位二进制数据的起点。
    fn skip_one_blank(&mut self) -> Result<()> {
        if self.pos < self.data.len() && is_blank(self.data[self.pos]) {
            self.pos += 1;
            Ok(())
        } else {
            Err(ConvertError::corrupt("Netpbm 二进制数据前缺少空白符"))
        }
    }

    /// 剩余的原始字节。
    fn rest(&self) -> &'a [u8] {
        &self.data[self.pos.min(self.data.len())..]
    }
}

/// 解码 P1 / P4 双色位图。
fn decode_bilevel(
    tokenizer: &mut Tokenizer<'_>,
    width: u32,
    height: u32,
    binary: bool,
) -> Result<Image> {
    let total = validate_size(width, height)?;
    let mut image = Image::new_zeroed(width, height, ColorType::Gray8)?;

    if binary {
        tokenizer.skip_one_blank()?;
        let payload = tokenizer.rest();
        let row_bytes = (width as usize).div_ceil(8);
        let needed = row_bytes
            .checked_mul(height as usize)
            .ok_or_else(|| ConvertError::corrupt("Netpbm 像素数据长度溢出"))?;
        if payload.len() < needed {
            return Err(ConvertError::corrupt("Netpbm 像素数据不完整"));
        }
        for y in 0..height as usize {
            let row = &payload[y * row_bytes..(y + 1) * row_bytes];
            for x in 0..width as usize {
                // PBM 中 1 表示黑色,0 表示白色。
                let bit = (row[x / 8] >> (7 - (x % 8))) & 1;
                let value = if bit == 1 { 0 } else { 255 };
                image.set_pixel(x as u32, y as u32, Rgba::from_gray(value));
            }
        }
    } else {
        for index in 0..total {
            let value = tokenizer.next_u32()?;
            if value > 1 {
                return Err(ConvertError::corrupt(format!("PBM 采样值 {value} 非法")));
            }
            let gray = if value == 1 { 0 } else { 255 };
            image.set_pixel(
                (index % width as usize) as u32,
                (index / width as usize) as u32,
                Rgba::from_gray(gray),
            );
        }
    }

    Ok(image)
}

/// 解码 P2 / P5 灰度图。
fn decode_graymap(
    tokenizer: &mut Tokenizer<'_>,
    width: u32,
    height: u32,
    binary: bool,
) -> Result<Image> {
    let total = validate_size(width, height)?;
    let maxval = tokenizer.next_u32()?;
    validate_maxval(maxval)?;

    let mut image = Image::new_zeroed(width, height, ColorType::Gray8)?;

    if binary {
        tokenizer.skip_one_blank()?;
        let payload = tokenizer.rest();
        let sample_bytes = if maxval < 256 { 1 } else { 2 };
        let needed = total
            .checked_mul(sample_bytes)
            .ok_or_else(|| ConvertError::corrupt("Netpbm 像素数据长度溢出"))?;
        if payload.len() < needed {
            return Err(ConvertError::corrupt("Netpbm 像素数据不完整"));
        }
        for index in 0..total {
            let value = if sample_bytes == 1 {
                payload[index] as u32
            } else {
                u16::from_be_bytes([payload[index * 2], payload[index * 2 + 1]]) as u32
            };
            image.set_pixel(
                (index % width as usize) as u32,
                (index / width as usize) as u32,
                Rgba::from_gray(scale_sample(value, maxval)),
            );
        }
    } else {
        for index in 0..total {
            let value = tokenizer.next_u32()?;
            if value > maxval {
                return Err(ConvertError::corrupt(format!("PGM 采样值 {value} 超限")));
            }
            image.set_pixel(
                (index % width as usize) as u32,
                (index / width as usize) as u32,
                Rgba::from_gray(scale_sample(value, maxval)),
            );
        }
    }

    Ok(image)
}

/// 解码 P3 / P6 真彩图。
fn decode_pixmap(
    tokenizer: &mut Tokenizer<'_>,
    width: u32,
    height: u32,
    binary: bool,
) -> Result<Image> {
    let total = validate_size(width, height)?;
    let maxval = tokenizer.next_u32()?;
    validate_maxval(maxval)?;

    let mut image = Image::new_zeroed(width, height, ColorType::Rgb8)?;

    if binary {
        tokenizer.skip_one_blank()?;
        let payload = tokenizer.rest();
        let sample_bytes = if maxval < 256 { 1 } else { 2 };
        let needed = total
            .checked_mul(3)
            .and_then(|count| count.checked_mul(sample_bytes))
            .ok_or_else(|| ConvertError::corrupt("Netpbm 像素数据长度溢出"))?;
        if payload.len() < needed {
            return Err(ConvertError::corrupt("Netpbm 像素数据不完整"));
        }
        for index in 0..total {
            let base = index * 3 * sample_bytes;
            let mut channel = [0u8; 3];
            for (offset, slot) in channel.iter_mut().enumerate() {
                let value = if sample_bytes == 1 {
                    payload[base + offset] as u32
                } else {
                    let at = base + offset * 2;
                    u16::from_be_bytes([payload[at], payload[at + 1]]) as u32
                };
                *slot = scale_sample(value, maxval);
            }
            image.set_pixel(
                (index % width as usize) as u32,
                (index / width as usize) as u32,
                Rgba::from_rgb(channel[0], channel[1], channel[2]),
            );
        }
    } else {
        for index in 0..total {
            let mut channel = [0u8; 3];
            for slot in channel.iter_mut() {
                let value = tokenizer.next_u32()?;
                if value > maxval {
                    return Err(ConvertError::corrupt(format!("PPM 采样值 {value} 超限")));
                }
                *slot = scale_sample(value, maxval);
            }
            image.set_pixel(
                (index % width as usize) as u32,
                (index / width as usize) as u32,
                Rgba::from_rgb(channel[0], channel[1], channel[2]),
            );
        }
    }

    Ok(image)
}

/// 解码 P7(PAM)。头部为若干 `键 值` 行,以 `ENDHDR` 结束。
fn decode_pam(data: &[u8]) -> Result<Image> {
    let mut pos = 2usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut depth = 0u32;
    let mut maxval = 0u32;
    let mut tuple = String::new();
    let mut saw_end = false;

    while pos < data.len() {
        let start = pos;
        while pos < data.len() && data[pos] != b'\n' {
            pos += 1;
        }
        let line = &data[start..pos];
        pos += 1;

        let line = trim(line);
        if line.is_empty() || line[0] == b'#' {
            continue;
        }

        let split = line
            .iter()
            .position(|byte| is_blank(*byte))
            .unwrap_or(line.len());
        let key = &line[..split];
        let value = trim(&line[split..]);

        if key.eq_ignore_ascii_case(b"WIDTH") {
            width = parse_u32(value).unwrap_or(0);
        } else if key.eq_ignore_ascii_case(b"HEIGHT") {
            height = parse_u32(value).unwrap_or(0);
        } else if key.eq_ignore_ascii_case(b"DEPTH") {
            depth = parse_u32(value).unwrap_or(0);
        } else if key.eq_ignore_ascii_case(b"MAXVAL") {
            maxval = parse_u32(value).unwrap_or(0);
        } else if key.eq_ignore_ascii_case(b"TUPLTYPE") {
            tuple = String::from_utf8_lossy(value).to_string();
        } else if key.eq_ignore_ascii_case(b"ENDHDR") {
            saw_end = true;
            break;
        }
    }

    if !saw_end {
        return Err(ConvertError::corrupt("PAM 头部缺少 ENDHDR"));
    }
    let total = validate_size(width, height)?;
    validate_maxval(maxval)?;
    let color = match depth {
        1 => ColorType::Gray8,
        2 => ColorType::GrayAlpha8,
        3 => ColorType::Rgb8,
        4 => ColorType::Rgba8,
        other => {
            return Err(ConvertError::unsupported(format!(
                "PAM 通道数 {other}(元组类型 {tuple})"
            )))
        }
    };

    let sample_bytes = if maxval < 256 { 1 } else { 2 };
    let payload = &data[pos.min(data.len())..];
    let needed = total
        .checked_mul(depth as usize)
        .and_then(|count| count.checked_mul(sample_bytes))
        .ok_or_else(|| ConvertError::corrupt("PAM 像素数据长度溢出"))?;
    if payload.len() < needed {
        return Err(ConvertError::corrupt("PAM 像素数据不完整"));
    }

    let mut image = Image::new_zeroed(width, height, color)?;
    let channels = depth as usize;
    for index in 0..total {
        let base = index * channels * sample_bytes;
        let mut sample = [0u8; 4];
        for (offset, slot) in sample[..channels].iter_mut().enumerate() {
            let value = if sample_bytes == 1 {
                payload[base + offset] as u32
            } else {
                let at = base + offset * 2;
                u16::from_be_bytes([payload[at], payload[at + 1]]) as u32
            };
            *slot = scale_sample(value, maxval);
        }
        let pixel = match channels {
            1 => Rgba::from_gray(sample[0]),
            2 => Rgba::new(sample[0], sample[0], sample[0], sample[1]),
            3 => Rgba::from_rgb(sample[0], sample[1], sample[2]),
            _ => Rgba::new(sample[0], sample[1], sample[2], sample[3]),
        };
        image.set_pixel(
            (index % width as usize) as u32,
            (index / width as usize) as u32,
            pixel,
        );
    }

    Ok(image)
}

/// 去掉首尾空白符。
fn trim(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && is_blank(bytes[start]) {
        start += 1;
    }
    while end > start && is_blank(bytes[end - 1]) {
        end -= 1;
    }
    &bytes[start..end]
}

/// 写出 P5 灰度图。
fn encode_gray(image: &Image) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::with_capacity(32 + image.pixel_count());
    writer.write_bytes(format!("P5\n{} {}\n255\n", image.width, image.height).as_bytes());
    for y in 0..image.height {
        for x in 0..image.width {
            writer.write_u8(image.get_pixel(x, y).luma());
        }
    }
    Ok(writer.into_vec())
}

/// 写出 P6 真彩图。
fn encode_rgb(image: &Image) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::with_capacity(32 + image.pixel_count() * 3);
    writer.write_bytes(format!("P6\n{} {}\n255\n", image.width, image.height).as_bytes());
    for y in 0..image.height {
        for x in 0..image.width {
            let pixel = image.get_pixel(x, y);
            writer.write_u8(pixel.r);
            writer.write_u8(pixel.g);
            writer.write_u8(pixel.b);
        }
    }
    Ok(writer.into_vec())
}

/// 写出 P7(PAM),用于承载带透明度的图像。
fn encode_pam(image: &Image, depth: usize, tuple: &str) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::with_capacity(128 + image.pixel_count() * depth);
    writer.write_bytes(b"P7\n");
    writer.write_bytes(format!("WIDTH {}\n", image.width).as_bytes());
    writer.write_bytes(format!("HEIGHT {}\n", image.height).as_bytes());
    writer.write_bytes(format!("DEPTH {depth}\n").as_bytes());
    writer.write_bytes(b"MAXVAL 255\n");
    writer.write_bytes(format!("TUPLTYPE {tuple}\n").as_bytes());
    writer.write_bytes(b"ENDHDR\n");

    for y in 0..image.height {
        for x in 0..image.width {
            let pixel = image.get_pixel(x, y);
            if depth == 2 {
                writer.write_u8(pixel.luma());
                writer.write_u8(pixel.a);
            } else {
                writer.write_u8(pixel.r);
                writer.write_u8(pixel.g);
                writer.write_u8(pixel.b);
                writer.write_u8(pixel.a);
            }
        }
    }

    Ok(writer.into_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> Result<Image> {
        NetpbmCodec.decode(bytes)
    }

    #[test]
    fn sniff_accepts_p1_to_p7() {
        for kind in b'1'..=b'7' {
            let header = [b'P', kind];
            assert!(NetpbmCodec.sniff(&header), "P{} 未被识别", kind as char);
        }
        assert!(!NetpbmCodec.sniff(b"P8"));
        assert!(!NetpbmCodec.sniff(b"P"));
    }

    #[test]
    fn ascii_bilevel_inverts_polarity() {
        // P1 中 1 为黑色
        let image = decode(b"P1\n3 1\n1 0 1\n").unwrap();
        assert_eq!(image.color, ColorType::Gray8);
        assert_eq!(image.get_pixel(0, 0), Rgba::from_gray(0));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_gray(255));
        assert_eq!(image.get_pixel(2, 0), Rgba::from_gray(0));
    }

    #[test]
    fn comments_are_ignored() {
        let image = decode(b"P2\n# comment here\n2 1\n15\n0 15\n").unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_gray(0));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_gray(255));
    }

    #[test]
    fn ascii_graymap_scales_maxval() {
        let image = decode(b"P2 2 1 3\n0 3\n").unwrap();
        assert_eq!(image.get_pixel(1, 0), Rgba::from_gray(255));
    }

    #[test]
    fn ascii_pixmap_reads_three_samples() {
        let image = decode(b"P3\n1 1\n255\n255 0 128\n").unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(255, 0, 128));
    }

    #[test]
    fn binary_bilevel_pads_rows() {
        // 宽度 3,每行占 1 字节,两行
        let image = decode(b"P4\n3 2\n\xA0\x40").unwrap();
        // 第一行 101 -> 黑 白 黑
        assert_eq!(image.get_pixel(0, 0), Rgba::from_gray(0));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_gray(255));
        assert_eq!(image.get_pixel(2, 0), Rgba::from_gray(0));
        // 第二行 010 -> 白 黑 白
        assert_eq!(image.get_pixel(0, 1), Rgba::from_gray(255));
        assert_eq!(image.get_pixel(1, 1), Rgba::from_gray(0));
    }

    #[test]
    fn binary_graymap_reads_sixteen_bit_samples() {
        let mut data = b"P5\n2 1\n65535\n".to_vec();
        data.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]);
        let image = decode(&data).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_gray(0));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_gray(255));
    }

    #[test]
    fn binary_pixmap_reads_rgb_triples() {
        let mut data = b"P6\n1 2\n255\n".to_vec();
        data.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let image = decode(&data).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(1, 2, 3));
        assert_eq!(image.get_pixel(0, 1), Rgba::from_rgb(4, 5, 6));
    }

    #[test]
    fn pam_reads_rgba() {
        let mut data = b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
            .to_vec();
        data.extend_from_slice(&[9, 8, 7, 6]);
        let image = decode(&data).unwrap();
        assert_eq!(image.color, ColorType::Rgba8);
        assert_eq!(image.get_pixel(0, 0), Rgba::new(9, 8, 7, 6));
    }

    #[test]
    fn pam_reads_grayscale_alpha() {
        let mut data =
            b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 2\nMAXVAL 255\nTUPLTYPE GRAYSCALE_ALPHA\nENDHDR\n"
                .to_vec();
        data.extend_from_slice(&[100, 255, 200, 0]);
        let image = decode(&data).unwrap();
        assert_eq!(image.color, ColorType::GrayAlpha8);
        assert_eq!(image.get_pixel(0, 0), Rgba::new(100, 100, 100, 255));
        assert_eq!(image.get_pixel(1, 0), Rgba::new(200, 200, 200, 0));
    }

    #[test]
    fn pam_without_endhdr_is_rejected() {
        assert!(decode(b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 3\nMAXVAL 255\n").is_err());
    }

    #[test]
    fn pam_with_unsupported_depth_is_rejected() {
        assert!(decode(
            b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 5\nMAXVAL 255\nTUPLTYPE CMYK\nENDHDR\n\0\0\0\0\0"
        )
        .is_err());
    }

    #[test]
    fn invalid_maxval_is_rejected() {
        assert!(decode(b"P2\n1 1\n0\n0\n").is_err());
        assert!(decode(b"P2\n1 1\n70000\n0\n").is_err());
    }

    #[test]
    fn truncated_binary_data_is_rejected() {
        assert!(decode(b"P6\n4 4\n255\n\x01\x02\x03").is_err());
    }

    #[test]
    fn zero_size_is_rejected() {
        assert!(decode(b"P1\n0 4\n").is_err());
    }

    #[test]
    fn gray_image_encodes_as_p5() {
        let mut image = Image::new_zeroed(2, 1, ColorType::Gray8).unwrap();
        image.set_pixel(0, 0, Rgba::from_gray(0));
        image.set_pixel(1, 0, Rgba::from_gray(255));
        let bytes = NetpbmCodec.encode(&image, &EncodeOptions::default()).unwrap();
        assert!(bytes.starts_with(b"P5\n2 1\n255\n"));
        assert_eq!(NetpbmCodec.decode(&bytes).unwrap().data, image.data);
    }

    #[test]
    fn rgb_image_encodes_as_p6() {
        let image = Image::filled(2, 2, ColorType::Rgb8, Rgba::from_rgb(3, 4, 5)).unwrap();
        let bytes = NetpbmCodec.encode(&image, &EncodeOptions::default()).unwrap();
        assert!(bytes.starts_with(b"P6\n2 2\n255\n"));
        assert_eq!(NetpbmCodec.decode(&bytes).unwrap().data, image.data);
    }

    #[test]
    fn rgba_image_encodes_as_pam() {
        let image = Image::filled(1, 1, ColorType::Rgba8, Rgba::new(1, 2, 3, 4)).unwrap();
        let bytes = NetpbmCodec.encode(&image, &EncodeOptions::default()).unwrap();
        assert!(bytes.starts_with(b"P7\n"));
        assert_eq!(NetpbmCodec.decode(&bytes).unwrap().data, image.data);
    }

    #[test]
    fn gray_alpha_image_encodes_as_pam_depth_two() {
        let image =
            Image::new(1, 1, ColorType::GrayAlpha8, vec![77, 128]).unwrap();
        let bytes = NetpbmCodec.encode(&image, &EncodeOptions::default()).unwrap();
        assert!(bytes.starts_with(b"P7\n"));
        let decoded = NetpbmCodec.decode(&bytes).unwrap();
        assert_eq!(decoded.color, ColorType::GrayAlpha8);
        assert_eq!(decoded.data, image.data);
    }
}
