//! PNG 图片的读取和写入。
//! 像素数据和 PNG 块在这里处理，压缩与解压交给 flate2。

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

use crate::core::codec::{Decoder, Encoder};
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;
use crate::core::pixel::{ColorType, Rgba};
use crate::core::registry::Registry;

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const ADAM7: [(u32, u32, u32, u32); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

pub fn register(registry: &mut Registry) {
    registry.register_decoder(Box::new(PngCodec));
    registry.register_encoder(Box::new(PngCodec));
}

struct PngCodec;

#[derive(Clone, Copy)]
struct Header {
    width: u32,
    height: u32,
    depth: u8,
    color_type: u8,
    interlace: u8,
}

struct ParsedPng {
    header: Header,
    palette: Vec<[u8; 3]>,
    transparency: Vec<u8>,
    transparent_gray: Option<u16>,
    transparent_rgb: Option<[u16; 3]>,
    compressed: Vec<u8>,
}

impl Decoder for PngCodec {
    fn format(&self) -> Format {
        Format::Png
    }

    fn sniff(&self, header: &[u8]) -> bool {
        header.starts_with(SIGNATURE)
    }

    fn decode(&self, data: &[u8]) -> Result<Image> {
        if !data.starts_with(SIGNATURE) {
            return Err(ConvertError::UnknownFormat);
        }
        let parsed = parse(data)?;
        decode_image(parsed)
    }
}

impl Encoder for PngCodec {
    fn format(&self) -> Format {
        Format::Png
    }

    fn encode(&self, image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
        if image.width == 0 || image.height == 0 {
            return Err(ConvertError::corrupt("PNG 宽高不能为 0"));
        }
        let alpha = match image.color {
            ColorType::GrayAlpha8 => image.data.chunks(2).any(|pixel| pixel[1] != 255),
            ColorType::Rgba8 => image.data.chunks(4).any(|pixel| pixel[3] != 255),
            ColorType::Gray8 | ColorType::Rgb8 => false,
        };
        let channels = if alpha { 4 } else { 3 };
        let row_bytes = (image.width as usize)
            .checked_mul(channels)
            .ok_or_else(|| ConvertError::corrupt("PNG 图像尺寸过大"))?;
        let source_row_bytes = image.row_bytes();
        let compression_level = options.png_compression_level.min(9) as u32;
        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::new(compression_level));
        let mut row = Vec::with_capacity(row_bytes);
        for y in 0..image.height as usize {
            compressor
                .write_all(&[0])
                .map_err(|error| ConvertError::corrupt(format!("PNG 压缩失败:{error}")))?;
            let start = y * source_row_bytes;
            let source = &image.data[start..start + source_row_bytes];
            match image.color {
                ColorType::Rgb8 => compressor.write_all(source),
                ColorType::Rgba8 if alpha => compressor.write_all(source),
                ColorType::Gray8 => {
                    row.clear();
                    for gray in source {
                        row.extend_from_slice(&[*gray, *gray, *gray]);
                    }
                    compressor.write_all(&row)
                }
                ColorType::GrayAlpha8 => {
                    row.clear();
                    for pixel in source.chunks(2) {
                        row.extend_from_slice(&[pixel[0], pixel[0], pixel[0]]);
                        if alpha {
                            row.push(pixel[1]);
                        }
                    }
                    compressor.write_all(&row)
                }
                ColorType::Rgba8 => {
                    row.clear();
                    for pixel in source.chunks(4) {
                        row.extend_from_slice(&pixel[..3]);
                    }
                    compressor.write_all(&row)
                }
            }
            .map_err(|error| ConvertError::corrupt(format!("PNG 压缩失败:{error}")))?;
        }
        let compressed = compressor
            .finish()
            .map_err(|error| ConvertError::corrupt(format!("PNG 压缩失败:{error}")))?;

        let mut output = SIGNATURE.to_vec();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&image.width.to_be_bytes());
        ihdr.extend_from_slice(&image.height.to_be_bytes());
        ihdr.extend_from_slice(&[8, if alpha { 6 } else { 2 }, 0, 0, 0]);
        append_chunk(&mut output, b"IHDR", &ihdr)?;
        append_chunk(&mut output, b"IDAT", &compressed)?;
        append_chunk(&mut output, b"IEND", &[])?;
        Ok(output)
    }
}

fn parse(data: &[u8]) -> Result<ParsedPng> {
    let mut offset = SIGNATURE.len();
    let mut header = None;
    let mut palette = Vec::new();
    let mut transparency = Vec::new();
    let mut transparent_gray = None;
    let mut transparent_rgb = None;
    let mut compressed = Vec::new();
    let mut saw_palette = false;
    let mut saw_transparency = false;
    let mut saw_data = false;
    let mut ended_data = false;
    let mut saw_end = false;

    while offset < data.len() {
        let chunk_start = offset;
        let length_bytes = data
            .get(offset..offset + 4)
            .ok_or_else(|| ConvertError::corrupt("PNG 块长度不完整"))?;
        let length = u32::from_be_bytes(length_bytes.try_into().unwrap()) as usize;
        offset += 4;
        let chunk_type = data
            .get(offset..offset + 4)
            .ok_or_else(|| ConvertError::corrupt("PNG 块类型不完整"))?;
        offset += 4;
        let chunk_end = offset
            .checked_add(length)
            .and_then(|end| end.checked_add(4))
            .ok_or_else(|| ConvertError::corrupt("PNG 块长度溢出"))?;
        if chunk_end > data.len() {
            return Err(ConvertError::corrupt("PNG 块数据不完整"));
        }
        let chunk_data = &data[offset..offset + length];
        let expected_crc = u32::from_be_bytes(data[offset + length..chunk_end].try_into().unwrap());
        let actual_crc = crc32(&data[chunk_start + 4..offset + length]);
        if expected_crc != actual_crc {
            return Err(ConvertError::corrupt("PNG 块 CRC 校验失败"));
        }
        offset = chunk_end;

        match chunk_type {
            b"IHDR" => {
                if header.is_some() || chunk_start != SIGNATURE.len() || length != 13 {
                    return Err(ConvertError::corrupt("PNG IHDR 块位置或长度无效"));
                }
                let parsed = Header {
                    width: u32::from_be_bytes(chunk_data[0..4].try_into().unwrap()),
                    height: u32::from_be_bytes(chunk_data[4..8].try_into().unwrap()),
                    depth: chunk_data[8],
                    color_type: chunk_data[9],
                    interlace: chunk_data[12],
                };
                if parsed.width == 0 || parsed.height == 0 {
                    return Err(ConvertError::corrupt("PNG 宽高不能为 0"));
                }
                if chunk_data[10] != 0 || chunk_data[11] != 0 || parsed.interlace > 1 {
                    return Err(ConvertError::unsupported("PNG 压缩、过滤或隔行方法"));
                }
                if !valid_depth(parsed.color_type, parsed.depth) {
                    return Err(ConvertError::corrupt("PNG 色彩类型与位深组合无效"));
                }
                header = Some(parsed);
            }
            b"PLTE" => {
                let ihdr = header.ok_or_else(|| ConvertError::corrupt("PNG 缺少 IHDR"))?;
                if saw_palette
                    || saw_transparency
                    || saw_data
                    || length == 0
                    || length > 768
                    || length / 3 * 3 != length
                {
                    return Err(ConvertError::corrupt("PNG 调色板无效"));
                }
                if ihdr.color_type == 0 || ihdr.color_type == 4 {
                    return Err(ConvertError::corrupt("灰度 PNG 不允许包含调色板"));
                }
                let entries = length / 3;
                if ihdr.color_type == 3 && entries > (1usize << ihdr.depth) {
                    return Err(ConvertError::corrupt("PNG 调色板超出位深范围"));
                }
                palette = chunk_data
                    .chunks(3)
                    .map(|rgb| [rgb[0], rgb[1], rgb[2]])
                    .collect();
                saw_palette = true;
            }
            b"tRNS" => {
                let ihdr = header.ok_or_else(|| ConvertError::corrupt("PNG 缺少 IHDR"))?;
                if saw_transparency || saw_data {
                    return Err(ConvertError::corrupt("PNG tRNS 块重复或顺序无效"));
                }
                match ihdr.color_type {
                    0 if length == 2 => {
                        let value = u16::from_be_bytes(chunk_data.try_into().unwrap());
                        if ihdr.depth < 16 && value >= (1 << ihdr.depth) {
                            return Err(ConvertError::corrupt("PNG 灰度 tRNS 值超出位深范围"));
                        }
                        transparent_gray = Some(value);
                    }
                    2 if length == 6 => {
                        let values = [
                            u16::from_be_bytes(chunk_data[0..2].try_into().unwrap()),
                            u16::from_be_bytes(chunk_data[2..4].try_into().unwrap()),
                            u16::from_be_bytes(chunk_data[4..6].try_into().unwrap()),
                        ];
                        if ihdr.depth == 8 && values.iter().any(|value| *value > 255) {
                            return Err(ConvertError::corrupt("PNG RGB tRNS 值超出位深范围"));
                        }
                        transparent_rgb = Some(values);
                    }
                    3 if saw_palette && length <= palette.len() => {
                        transparency.extend_from_slice(chunk_data);
                    }
                    _ => return Err(ConvertError::corrupt("PNG tRNS 块无效")),
                }
                saw_transparency = true;
            }
            b"IDAT" => {
                let ihdr = header.ok_or_else(|| ConvertError::corrupt("PNG 缺少 IHDR"))?;
                if ended_data {
                    return Err(ConvertError::corrupt("PNG IDAT 块必须连续"));
                }
                if ihdr.color_type == 3 && !saw_palette {
                    return Err(ConvertError::corrupt("索引 PNG 缺少调色板"));
                }
                saw_data = true;
                compressed.extend_from_slice(chunk_data);
            }
            b"IEND" => {
                if length != 0 || !saw_data || header.is_none() {
                    return Err(ConvertError::corrupt("PNG IEND 块无效"));
                }
                saw_end = true;
                if offset != data.len() {
                    return Err(ConvertError::corrupt("PNG IEND 后存在多余数据"));
                }
                break;
            }
            _ => {
                header.ok_or_else(|| ConvertError::corrupt("PNG 缺少 IHDR"))?;
                if saw_data {
                    ended_data = true;
                }
                if chunk_type[0] & 0x20 == 0 {
                    return Err(ConvertError::unsupported(format!(
                        "未知 PNG 关键块 {}",
                        String::from_utf8_lossy(chunk_type)
                    )));
                }
            }
        }
    }

    let header = header.ok_or_else(|| ConvertError::corrupt("PNG 缺少 IHDR"))?;
    if !saw_end {
        return Err(ConvertError::corrupt("PNG 缺少 IEND"));
    }
    Ok(ParsedPng {
        header,
        palette,
        transparency,
        transparent_gray,
        transparent_rgb,
        compressed,
    })
}

fn decode_image(png: ParsedPng) -> Result<Image> {
    let channels = samples_per_pixel(png.header.color_type);
    let passes: &[(u32, u32, u32, u32)] = if png.header.interlace == 0 {
        &[(0, 0, 1, 1)]
    } else {
        &ADAM7
    };
    let mut expected = 0usize;
    for &(x_start, y_start, x_step, y_step) in passes {
        let width = pass_size(png.header.width, x_start, x_step);
        let height = pass_size(png.header.height, y_start, y_step);
        if width == 0 || height == 0 {
            continue;
        }
        let row_bytes = row_bytes(width, channels, png.header.depth)?;
        expected = expected
            .checked_add(
                row_bytes
                    .checked_add(1)
                    .and_then(|bytes| bytes.checked_mul(height as usize))
                    .ok_or_else(|| ConvertError::corrupt("PNG 图像尺寸过大"))?,
            )
            .ok_or_else(|| ConvertError::corrupt("PNG 图像尺寸过大"))?;
    }
    let decoder = ZlibDecoder::new(png.compressed.as_slice());
    let mut raw = Vec::with_capacity(expected);
    decoder
        .take(expected as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|error| ConvertError::corrupt(format!("PNG DEFLATE 数据无效:{error}")))?;
    if raw.len() != expected {
        return Err(ConvertError::corrupt("PNG 解压数据长度与图像尺寸不符"));
    }

    let mut image = Image::new_zeroed(png.header.width, png.header.height, ColorType::Rgba8)?;
    let mut offset = 0usize;
    for &(x_start, y_start, x_step, y_step) in passes {
        let pass_width = pass_size(png.header.width, x_start, x_step);
        let pass_height = pass_size(png.header.height, y_start, y_step);
        if pass_width == 0 || pass_height == 0 {
            continue;
        }
        let line_size = row_bytes(pass_width, channels, png.header.depth)?;
        let filter_bpp = (channels * png.header.depth as usize).div_ceil(8).max(1);
        let mut previous = vec![0u8; line_size];
        for pass_y in 0..pass_height {
            let filter = raw[offset];
            offset += 1;
            let mut line = raw[offset..offset + line_size].to_vec();
            offset += line_size;
            unfilter(filter, &mut line, &previous, filter_bpp)?;
            for pass_x in 0..pass_width {
                let samples = read_samples(&line, pass_x, channels, png.header.depth);
                let pixel = to_rgba(&png, &samples)?;
                image.set_pixel(x_start + pass_x * x_step, y_start + pass_y * y_step, pixel);
            }
            previous = line;
        }
    }
    Ok(image)
}

fn to_rgba(png: &ParsedPng, samples: &[u16]) -> Result<Rgba> {
    let depth = png.header.depth;
    let scale = |sample: u16| -> u8 {
        if depth == 16 {
            (sample >> 8) as u8
        } else if depth == 8 {
            sample as u8
        } else {
            ((sample as u32 * 255 + ((1u32 << depth) - 1) / 2) / ((1u32 << depth) - 1)) as u8
        }
    };
    let pixel = match png.header.color_type {
        0 => {
            let gray = samples[0];
            let alpha = if png.transparent_gray == Some(gray) {
                0
            } else {
                255
            };
            let value = scale(gray);
            Rgba::new(value, value, value, alpha)
        }
        2 => {
            let rgb = [samples[0], samples[1], samples[2]];
            let alpha = if png.transparent_rgb == Some(rgb) {
                0
            } else {
                255
            };
            Rgba::new(scale(rgb[0]), scale(rgb[1]), scale(rgb[2]), alpha)
        }
        3 => {
            let index = samples[0] as usize;
            let color = png
                .palette
                .get(index)
                .ok_or_else(|| ConvertError::corrupt("PNG 像素引用了不存在的调色板项"))?;
            Rgba::new(
                color[0],
                color[1],
                color[2],
                png.transparency.get(index).copied().unwrap_or(255),
            )
        }
        4 => {
            let gray = scale(samples[0]);
            Rgba::new(gray, gray, gray, scale(samples[1]))
        }
        6 => Rgba::new(
            scale(samples[0]),
            scale(samples[1]),
            scale(samples[2]),
            scale(samples[3]),
        ),
        _ => return Err(ConvertError::corrupt("PNG 色彩类型无效")),
    };
    Ok(pixel)
}

fn read_samples(row: &[u8], pixel: u32, channels: usize, depth: u8) -> Vec<u16> {
    let mut samples = Vec::with_capacity(channels);
    let bits_per_pixel = channels * depth as usize;
    let bit_offset = pixel as usize * bits_per_pixel;
    for channel in 0..channels {
        let bit = bit_offset + channel * depth as usize;
        samples.push(match depth {
            16 => u16::from_be_bytes([row[bit / 8], row[bit / 8 + 1]]),
            8 => row[bit / 8] as u16,
            _ => {
                let shift = 8 - depth as usize - bit % 8;
                ((row[bit / 8] >> shift) & ((1 << depth) - 1)) as u16
            }
        });
    }
    samples
}

fn unfilter(filter: u8, row: &mut [u8], previous: &[u8], bpp: usize) -> Result<()> {
    for index in 0..row.len() {
        let left = if index >= bpp { row[index - bpp] } else { 0 };
        let above = previous[index];
        let upper_left = if index >= bpp {
            previous[index - bpp]
        } else {
            0
        };
        row[index] = match filter {
            0 => row[index],
            1 => row[index].wrapping_add(left),
            2 => row[index].wrapping_add(above),
            3 => row[index].wrapping_add(((left as u16 + above as u16) / 2) as u8),
            4 => row[index].wrapping_add(paeth(left, above, upper_left)),
            _ => {
                return Err(ConvertError::corrupt(format!(
                    "PNG 过滤器类型无效:{filter}"
                )));
            }
        };
    }
    Ok(())
}

fn paeth(left: u8, above: u8, upper_left: u8) -> u8 {
    let p = left as i32 + above as i32 - upper_left as i32;
    let dl = (p - left as i32).abs();
    let da = (p - above as i32).abs();
    let dul = (p - upper_left as i32).abs();
    if dl <= da && dl <= dul {
        left
    } else if da <= dul {
        above
    } else {
        upper_left
    }
}

fn valid_depth(color_type: u8, depth: u8) -> bool {
    match color_type {
        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
        2 => matches!(depth, 8 | 16),
        3 => matches!(depth, 1 | 2 | 4 | 8),
        4 | 6 => matches!(depth, 8 | 16),
        _ => false,
    }
}

fn samples_per_pixel(color_type: u8) -> usize {
    match color_type {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => unreachable!("IHDR 已验证色彩类型"),
    }
}

fn row_bytes(width: u32, channels: usize, depth: u8) -> Result<usize> {
    (width as usize)
        .checked_mul(channels)
        .and_then(|bits| bits.checked_mul(depth as usize))
        .and_then(|bits| bits.checked_add(7))
        .map(|bits| bits / 8)
        .ok_or_else(|| ConvertError::corrupt("PNG 图像尺寸过大"))
}

fn pass_size(size: u32, start: u32, step: u32) -> u32 {
    if size <= start {
        0
    } else {
        (size - start).div_ceil(step)
    }
}

fn append_chunk(output: &mut Vec<u8>, kind: &[u8; 4], contents: &[u8]) -> Result<()> {
    let length = u32::try_from(contents.len())
        .map_err(|_| ConvertError::corrupt("PNG 块长度超出格式限制"))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(contents);
    let crc_start = output.len() - contents.len() - kind.len();
    let checksum = crc32(&output[crc_start..]);
    output.extend_from_slice(&checksum.to_be_bytes());
    Ok(())
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb88320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::tests::sample_image;

    fn fixture_png(
        width: u32,
        height: u32,
        depth: u8,
        color_type: u8,
        interlace: u8,
        before_data: &[(&[u8; 4], &[u8])],
        raw: &[u8],
    ) -> Vec<u8> {
        let mut output = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[depth, color_type, 0, 0, interlace]);
        append_chunk(&mut output, b"IHDR", &ihdr).unwrap();
        for (kind, contents) in before_data {
            append_chunk(&mut output, kind, contents).unwrap();
        }
        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
        compressor.write_all(raw).unwrap();
        append_chunk(&mut output, b"IDAT", &compressor.finish().unwrap()).unwrap();
        append_chunk(&mut output, b"IEND", &[]).unwrap();
        output
    }

    #[test]
    fn sniff_checks_signature() {
        assert!(PngCodec.sniff(SIGNATURE));
        assert!(!PngCodec.sniff(b"\x89PNG"));
        assert!(!PngCodec.sniff(b"not png"));
    }

    #[test]
    fn round_trip_keeps_transparency_and_dimensions() {
        let source = sample_image();
        let encoded = PngCodec.encode(&source, &EncodeOptions::default()).unwrap();
        let decoded = PngCodec.decode(&encoded).unwrap();
        assert_eq!(
            (decoded.width, decoded.height),
            (source.width, source.height)
        );
        assert_eq!(decoded.data, source.to_rgba().data);
    }

    #[test]
    fn opaque_images_are_written_as_rgb() {
        let source = Image::filled(2, 1, ColorType::Rgb8, Rgba::from_rgb(4, 5, 6)).unwrap();
        let encoded = PngCodec.encode(&source, &EncodeOptions::default()).unwrap();
        assert_eq!(encoded[25], 2);
        assert_eq!(
            PngCodec.decode(&encoded).unwrap().get_pixel(1, 0),
            Rgba::from_rgb(4, 5, 6)
        );
    }

    #[test]
    fn every_color_layout_round_trips() {
        let rgba = sample_image();
        for color in ColorType::all() {
            let source = rgba.convert_to(color);
            let encoded = PngCodec.encode(&source, &EncodeOptions::default()).unwrap();
            let decoded = PngCodec.decode(&encoded).unwrap();
            assert_eq!(
                decoded.data,
                source.to_rgba().data,
                "{} 往返后像素不一致",
                color.name()
            );
        }
    }

    #[test]
    fn opaque_alpha_layouts_are_encoded_without_alpha() {
        for color in [ColorType::GrayAlpha8, ColorType::Rgba8] {
            let source = Image::filled(2, 1, color, Rgba::from_rgb(8, 9, 10)).unwrap();
            let encoded = PngCodec.encode(&source, &EncodeOptions::default()).unwrap();
            assert_eq!(encoded[25], 2, "{} 应写成 RGB PNG", color.name());
            assert_eq!(
                PngCodec.decode(&encoded).unwrap().data,
                source.to_rgba().data
            );
        }
    }

    #[test]
    fn compression_level_changes_size_without_changing_pixels() {
        let source = Image::filled(256, 256, ColorType::Rgb8, Rgba::from_rgb(24, 48, 96)).unwrap();
        let uncompressed = PngCodec
            .encode(
                &source,
                &EncodeOptions {
                    png_compression_level: 0,
                    ..Default::default()
                },
            )
            .unwrap();
        let compressed = PngCodec
            .encode(
                &source,
                &EncodeOptions {
                    png_compression_level: 9,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(compressed.len() < uncompressed.len());
        assert_eq!(
            PngCodec.decode(&compressed).unwrap().data,
            source.to_rgba().data
        );
    }

    #[test]
    fn rejects_corrupt_chunk_crc() {
        let source = Image::filled(1, 1, ColorType::Rgb8, Rgba::from_rgb(1, 2, 3)).unwrap();
        let mut encoded = PngCodec.encode(&source, &EncodeOptions::default()).unwrap();
        encoded[29] ^= 1;
        assert!(PngCodec.decode(&encoded).is_err());
    }

    #[test]
    fn all_png_filters_restore_expected_bytes() {
        let expected = [10u8, 20, 30, 40];
        for filter in 0..=4 {
            let mut encoded_row = expected;
            let previous = [3u8, 9, 15, 21];
            for index in 0..encoded_row.len() {
                let left = if index == 0 { 0 } else { expected[index - 1] };
                let above = previous[index];
                let upper_left = if index == 0 { 0 } else { previous[index - 1] };
                let predictor = match filter {
                    0 => 0,
                    1 => left,
                    2 => above,
                    3 => ((left as u16 + above as u16) / 2) as u8,
                    _ => paeth(left, above, upper_left),
                };
                encoded_row[index] = encoded_row[index].wrapping_sub(predictor);
            }
            unfilter(filter, &mut encoded_row, &previous, 1).unwrap();
            assert_eq!(encoded_row, expected);
        }
    }

    #[test]
    fn decodes_packed_grayscale_and_indexed_transparency() {
        let grayscale = fixture_png(4, 1, 1, 0, 0, &[], &[0, 0b1010_0000]);
        let image = PngCodec.decode(&grayscale).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(255, 255, 255));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_rgb(0, 0, 0));

        let palette = [255, 0, 0, 0, 255, 0];
        let transparency = [255, 0];
        let indexed = fixture_png(
            2,
            1,
            1,
            3,
            0,
            &[(b"PLTE", &palette), (b"tRNS", &transparency)],
            &[0, 0b0100_0000],
        );
        let image = PngCodec.decode(&indexed).unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(255, 0, 0));
        assert_eq!(image.get_pixel(1, 0), Rgba::new(0, 255, 0, 0));
    }

    #[test]
    fn decodes_adam7_interlaced_pixels() {
        let (width, height) = (5, 4);
        let mut raw = Vec::new();
        for &(x_start, y_start, x_step, y_step) in &ADAM7 {
            let pass_width = pass_size(width, x_start, x_step);
            let pass_height = pass_size(height, y_start, y_step);
            for pass_y in 0..pass_height {
                raw.push(0);
                for pass_x in 0..pass_width {
                    let x = x_start + pass_x * x_step;
                    let y = y_start + pass_y * y_step;
                    raw.extend_from_slice(&[x as u8, y as u8, (x + y) as u8]);
                }
            }
        }
        let bytes = fixture_png(width, height, 8, 2, 1, &[], &raw);
        let image = PngCodec.decode(&bytes).unwrap();
        for y in 0..height {
            for x in 0..width {
                assert_eq!(
                    image.get_pixel(x, y),
                    Rgba::from_rgb(x as u8, y as u8, (x + y) as u8)
                );
            }
        }
    }
}
