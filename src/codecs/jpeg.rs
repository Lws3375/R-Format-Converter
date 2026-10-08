//! JPEG baseline sequential 编解码器,仅使用 Rust 标准库与项目内的图像结构。
//!
//! 支持 8 位 Huffman DCT JPEG 的灰度和三分量图像,解码支持常见分量采样率;
//! 编码输出 4:4:4 采样,不支持渐进式、算术编码和 12 位 JPEG。

use std::f64::consts::PI;
use std::sync::OnceLock;

use crate::core::codec::{Decoder, Encoder};
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::Image;
use crate::core::options::EncodeOptions;
use crate::core::pixel::ColorType;
use crate::core::registry::Registry;

const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOS: u8 = 0xDA;
const DQT: u8 = 0xDB;
const DHT: u8 = 0xC4;
const SOF0: u8 = 0xC0;
const DRI: u8 = 0xDD;
const MAX_DECODE_BYTES: usize = 512 * 1024 * 1024;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const LUMA_QUANT: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

const CHROMA_QUANT: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7D];
const AC_LUMA_VALUES: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7,
    0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5,
    0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
    0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];
const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALUES: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33, 0x52, 0xF0,
    0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18, 0x19, 0x1A, 0x26,
    0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5,
    0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3,
    0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA,
    0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];

/// 注册 JPEG 解码器与编码器。
pub fn register(registry: &mut Registry) {
    registry.register_decoder(Box::new(JpegCodec));
    registry.register_encoder(Box::new(JpegCodec));
}

struct JpegCodec;

impl Decoder for JpegCodec {
    fn format(&self) -> Format {
        Format::Jpeg
    }

    fn sniff(&self, header: &[u8]) -> bool {
        header.starts_with(&[0xFF, SOI])
    }

    fn decode(&self, data: &[u8]) -> Result<Image> {
        decode_jpeg(data)
    }
}

impl Encoder for JpegCodec {
    fn format(&self) -> Format {
        Format::Jpeg
    }

    fn encode(&self, image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
        encode_jpeg(image, options.clamped_quality())
    }
}

#[derive(Clone)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    quant: usize,
    dc_table: usize,
    ac_table: usize,
    predictor: i32,
    samples: Vec<u8>,
    stride: usize,
}

#[derive(Clone)]
struct Huffman {
    min_code: [i32; 17],
    max_code: [i32; 17],
    value_offset: [i32; 17],
    values: Vec<u8>,
    encode: [(u16, u8); 256],
}

impl Huffman {
    fn new(counts: &[u8; 16], values: &[u8]) -> Result<Self> {
        let expected: usize = counts.iter().map(|&n| n as usize).sum();
        if expected != values.len() || expected > 256 {
            return Err(ConvertError::corrupt("JPEG Huffman 表长度无效"));
        }
        let mut table = Self {
            min_code: [-1; 17],
            max_code: [-1; 17],
            value_offset: [0; 17],
            values: values.to_vec(),
            encode: [(0, 0); 256],
        };
        let mut code = 0i32;
        let mut index = 0usize;
        for length in 1..=16 {
            let count = counts[length - 1] as usize;
            if count != 0 {
                table.min_code[length] = code;
                table.max_code[length] = code + count as i32 - 1;
                table.value_offset[length] = index as i32 - code;
                for value in 0..count {
                    let symbol = values[index + value] as usize;
                    if table.encode[symbol].1 != 0 {
                        return Err(ConvertError::corrupt("JPEG Huffman 符号重复"));
                    }
                    table.encode[symbol] = ((code + value as i32) as u16, length as u8);
                }
                index += count;
                code += count as i32;
            }
            if code > (1 << length) {
                return Err(ConvertError::corrupt("JPEG Huffman 码表过长"));
            }
            code <<= 1;
        }
        Ok(table)
    }

    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u8> {
        let mut code = 0i32;
        for length in 1..=16 {
            code = (code << 1) | reader.read_bit()? as i32;
            if self.max_code[length] >= 0
                && code >= self.min_code[length]
                && code <= self.max_code[length]
            {
                let index = (code + self.value_offset[length]) as usize;
                return self
                    .values
                    .get(index)
                    .copied()
                    .ok_or_else(|| ConvertError::corrupt("JPEG Huffman 符号索引越界"));
            }
        }
        Err(ConvertError::corrupt("JPEG Huffman 码无效"))
    }
}

fn decode_jpeg(data: &[u8]) -> Result<Image> {
    if data.len() < 4 || data[..2] != [0xFF, SOI] {
        return Err(ConvertError::UnknownFormat);
    }
    let mut pos = 2usize;
    let mut width = 0usize;
    let mut height = 0usize;
    let mut components = Vec::<Component>::new();
    let mut quant_tables: [Option<[u16; 64]>; 4] = [None; 4];
    let mut huffman: [[Option<Huffman>; 4]; 2] =
        std::array::from_fn(|_| std::array::from_fn(|_| None));
    let mut restart_interval = 0usize;

    loop {
        let marker = read_marker(data, &mut pos)?;
        match marker {
            EOI => return Err(ConvertError::corrupt("JPEG 缺少扫描数据")),
            DQT => parse_quant_tables(segment(data, &mut pos)?, &mut quant_tables)?,
            DHT => parse_huffman_tables(segment(data, &mut pos)?, &mut huffman)?,
            SOF0 => {
                let payload = segment(data, &mut pos)?;
                let parsed = parse_frame(payload)?;
                width = parsed.0;
                height = parsed.1;
                components = parsed.2;
            }
            DRI => {
                let payload = segment(data, &mut pos)?;
                if payload.len() != 2 {
                    return Err(ConvertError::corrupt("JPEG 重启间隔字段无效"));
                }
                restart_interval = u16::from_be_bytes([payload[0], payload[1]]) as usize;
            }
            SOS => {
                if components.is_empty() {
                    return Err(ConvertError::corrupt("JPEG 扫描缺少帧头"));
                }
                let payload = segment(data, &mut pos)?;
                parse_scan(payload, &mut components)?;
                return decode_scan(
                    data,
                    pos,
                    width,
                    height,
                    components,
                    &quant_tables,
                    &huffman,
                    restart_interval,
                );
            }
            0xC1..=0xCF if !matches!(marker, DHT | 0xC8 | 0xCC) => {
                return Err(ConvertError::unsupported(format!(
                    "暂不支持 JPEG 帧类型 0xFF{marker:02X}"
                )));
            }
            _ => {
                let _ = segment(data, &mut pos)?;
            }
        }
    }
}

fn read_marker(data: &[u8], pos: &mut usize) -> Result<u8> {
    if data.get(*pos) != Some(&0xFF) {
        return Err(ConvertError::corrupt("JPEG 标记起始字节无效"));
    }
    while data.get(*pos) == Some(&0xFF) {
        *pos += 1;
    }
    let marker = *data
        .get(*pos)
        .ok_or_else(|| ConvertError::corrupt("JPEG 标记不完整"))?;
    *pos += 1;
    Ok(marker)
}

fn segment<'a>(data: &'a [u8], pos: &mut usize) -> Result<&'a [u8]> {
    let length_bytes = data
        .get(*pos..*pos + 2)
        .ok_or_else(|| ConvertError::corrupt("JPEG 段长度不完整"))?;
    let length = u16::from_be_bytes([length_bytes[0], length_bytes[1]]) as usize;
    if length < 2 {
        return Err(ConvertError::corrupt("JPEG 段长度无效"));
    }
    let start = *pos + 2;
    let end = start
        .checked_add(length - 2)
        .filter(|&end| end <= data.len())
        .ok_or_else(|| ConvertError::corrupt("JPEG 段数据不完整"))?;
    *pos = end;
    Ok(&data[start..end])
}

fn parse_quant_tables(payload: &[u8], tables: &mut [Option<[u16; 64]>; 4]) -> Result<()> {
    let mut pos = 0usize;
    while pos < payload.len() {
        let info = payload[pos];
        pos += 1;
        let precision = info >> 4;
        let id = (info & 0x0F) as usize;
        if precision > 1 || id >= tables.len() {
            return Err(ConvertError::unsupported("JPEG 量化表精度或编号不支持"));
        }
        let bytes_per_value = if precision == 0 { 1 } else { 2 };
        let end = pos
            .checked_add(64 * bytes_per_value)
            .filter(|&end| end <= payload.len())
            .ok_or_else(|| ConvertError::corrupt("JPEG 量化表不完整"))?;
        let mut values = [0u16; 64];
        for index in 0..64 {
            let value = if precision == 0 {
                payload[pos + index] as u16
            } else {
                u16::from_be_bytes([payload[pos + index * 2], payload[pos + index * 2 + 1]])
            };
            if value == 0 {
                return Err(ConvertError::corrupt("JPEG 量化表包含 0"));
            }
            values[ZIGZAG[index]] = value;
        }
        tables[id] = Some(values);
        pos = end;
    }
    Ok(())
}

fn parse_huffman_tables(payload: &[u8], tables: &mut [[Option<Huffman>; 4]; 2]) -> Result<()> {
    let mut pos = 0usize;
    while pos < payload.len() {
        let info = payload[pos];
        pos += 1;
        let class = (info >> 4) as usize;
        let id = (info & 0x0F) as usize;
        if class > 1 || id > 3 || pos + 16 > payload.len() {
            return Err(ConvertError::corrupt("JPEG Huffman 表头无效"));
        }
        let counts: [u8; 16] = payload[pos..pos + 16]
            .try_into()
            .map_err(|_| ConvertError::corrupt("JPEG Huffman 表头不完整"))?;
        pos += 16;
        let value_count: usize = counts.iter().map(|&value| value as usize).sum();
        let end = pos
            .checked_add(value_count)
            .filter(|&end| end <= payload.len())
            .ok_or_else(|| ConvertError::corrupt("JPEG Huffman 表数据不完整"))?;
        tables[class][id] = Some(Huffman::new(&counts, &payload[pos..end])?);
        pos = end;
    }
    Ok(())
}

fn parse_frame(payload: &[u8]) -> Result<(usize, usize, Vec<Component>)> {
    if payload.len() < 6 || payload[0] != 8 {
        return Err(ConvertError::unsupported("仅支持 8 位 JPEG 帧"));
    }
    let height = u16::from_be_bytes([payload[1], payload[2]]) as usize;
    let width = u16::from_be_bytes([payload[3], payload[4]]) as usize;
    let count = payload[5] as usize;
    if width == 0 || height == 0 || !(count == 1 || count == 3) || payload.len() != 6 + count * 3 {
        return Err(ConvertError::corrupt("JPEG 帧尺寸或分量数无效"));
    }
    let mut components = Vec::with_capacity(count);
    let mut ids = Vec::with_capacity(count);
    let (mut max_h, mut max_v) = (0usize, 0usize);
    let mut blocks_per_mcu = 0usize;
    for index in 0..count {
        let base = 6 + index * 3;
        let id = payload[base];
        let h = (payload[base + 1] >> 4) as usize;
        let v = (payload[base + 1] & 0x0F) as usize;
        let quant = (payload[base + 2] & 0x0F) as usize;
        if h == 0 || v == 0 || h > 4 || v > 4 || ids.contains(&id) {
            return Err(ConvertError::corrupt("JPEG 分量采样因子无效"));
        }
        ids.push(id);
        max_h = max_h.max(h);
        max_v = max_v.max(v);
        blocks_per_mcu += h * v;
        components.push(Component {
            id,
            h,
            v,
            quant,
            dc_table: 0,
            ac_table: 0,
            predictor: 0,
            samples: Vec::new(),
            stride: 0,
        });
    }
    if blocks_per_mcu > 10 {
        return Err(ConvertError::unsupported("JPEG MCU 分量块数超过规范上限"));
    }
    Ok((width, height, components))
}

fn parse_scan(payload: &[u8], components: &mut [Component]) -> Result<()> {
    if payload.len() < 4 {
        return Err(ConvertError::corrupt("JPEG 扫描头不完整"));
    }
    let count = payload[0] as usize;
    if count != components.len() || payload.len() != 1 + count * 2 + 3 {
        return Err(ConvertError::unsupported(
            "仅支持单次扫描包含全部 JPEG 分量",
        ));
    }
    let mut ordered_ids = Vec::with_capacity(count);
    for scan_index in 0..count {
        let id = payload[1 + scan_index * 2];
        let table_ids = payload[2 + scan_index * 2];
        if ordered_ids.contains(&id) {
            return Err(ConvertError::corrupt("JPEG 扫描重复引用分量"));
        }
        ordered_ids.push(id);
        let component = components
            .iter_mut()
            .find(|component| component.id == id)
            .ok_or_else(|| ConvertError::corrupt("JPEG 扫描引用未知分量"))?;
        component.dc_table = (table_ids >> 4) as usize;
        component.ac_table = (table_ids & 0x0F) as usize;
    }
    let end = 1 + count * 2;
    if payload[end] != 0 || payload[end + 1] != 63 || payload[end + 2] != 0 {
        return Err(ConvertError::unsupported("仅支持基线顺序 JPEG 扫描参数"));
    }
    components.sort_by_key(|component| {
        ordered_ids
            .iter()
            .position(|&id| id == component.id)
            .unwrap_or(usize::MAX)
    });
    Ok(())
}

fn decode_scan(
    data: &[u8],
    pos: usize,
    width: usize,
    height: usize,
    mut components: Vec<Component>,
    quant_tables: &[Option<[u16; 64]>; 4],
    huffman: &[[Option<Huffman>; 4]; 2],
    restart_interval: usize,
) -> Result<Image> {
    let max_h = components
        .iter()
        .map(|component| component.h)
        .max()
        .unwrap_or(1);
    let max_v = components
        .iter()
        .map(|component| component.v)
        .max()
        .unwrap_or(1);
    let mcu_columns = width.div_ceil(max_h * 8);
    let mcu_rows = height.div_ceil(max_v * 8);
    let pixel_count = width
        .checked_mul(height)
        .filter(|&count| count <= MAX_DECODE_BYTES / 4)
        .ok_or_else(|| ConvertError::corrupt("JPEG 图像尺寸超过解码限制"))?;
    let mut allocation_size = pixel_count
        .checked_mul(components.len())
        .ok_or_else(|| ConvertError::corrupt("JPEG 解码输出尺寸溢出"))?;
    for component in &mut components {
        let block_columns = mcu_columns * component.h;
        let block_rows = mcu_rows * component.v;
        component.stride = block_columns * 8;
        let sample_len = component
            .stride
            .checked_mul(block_rows * 8)
            .filter(|&len| len <= MAX_DECODE_BYTES)
            .ok_or_else(|| ConvertError::corrupt("JPEG 分量尺寸过大"))?;
        allocation_size = allocation_size
            .checked_add(sample_len)
            .filter(|&size| size <= MAX_DECODE_BYTES)
            .ok_or_else(|| ConvertError::corrupt("JPEG 解码输出超过内存限制"))?;
        component.samples = vec![0; sample_len];
        let quant = quant_tables
            .get(component.quant)
            .and_then(Option::as_ref)
            .ok_or_else(|| ConvertError::corrupt("JPEG 缺少量化表"))?;
        let _ = quant;
    }

    for component in &components {
        if huffman[0][component.dc_table].is_none() || huffman[1][component.ac_table].is_none() {
            return Err(ConvertError::corrupt("JPEG 缺少 Huffman 表"));
        }
    }
    let mut bits = BitReader::new(data, pos);
    let mut mcu_index = 0usize;
    for mcu_y in 0..mcu_rows {
        for mcu_x in 0..mcu_columns {
            for component in &mut components {
                let quant = quant_tables[component.quant]
                    .as_ref()
                    .ok_or_else(|| ConvertError::corrupt("JPEG 缺少量化表"))?;
                let dc = huffman[0][component.dc_table]
                    .as_ref()
                    .ok_or_else(|| ConvertError::corrupt("JPEG 缺少 DC Huffman 表"))?;
                let ac = huffman[1][component.ac_table]
                    .as_ref()
                    .ok_or_else(|| ConvertError::corrupt("JPEG 缺少 AC Huffman 表"))?;
                for block_y in 0..component.v {
                    for block_x in 0..component.h {
                        let coefficients =
                            decode_block(&mut bits, dc, ac, quant, &mut component.predictor)?;
                        let samples = inverse_dct(&coefficients);
                        let start_x = (mcu_x * component.h + block_x) * 8;
                        let start_y = (mcu_y * component.v + block_y) * 8;
                        for y in 0..8 {
                            let start = (start_y + y) * component.stride + start_x;
                            component.samples[start..start + 8]
                                .copy_from_slice(&samples[y * 8..y * 8 + 8]);
                        }
                    }
                }
            }
            mcu_index += 1;
            if restart_interval != 0
                && mcu_index < mcu_columns * mcu_rows
                && mcu_index % restart_interval == 0
            {
                bits.consume_restart((mcu_index / restart_interval - 1) as u8 % 8)?;
                for component in &mut components {
                    component.predictor = 0;
                }
            }
        }
    }

    if components.len() == 1 {
        let component = &components[0];
        let mut pixels = Vec::with_capacity(pixel_count);
        for y in 0..height {
            pixels.extend_from_slice(
                &component.samples[y * component.stride..y * component.stride + width],
            );
        }
        return Image::new(width as u32, height as u32, ColorType::Gray8, pixels);
    }

    let rgb_order = components
        .iter()
        .map(|component| component.id)
        .collect::<Vec<_>>();
    let direct_rgb = rgb_order == [b'R', b'G', b'B'];
    let mut pixels = Vec::with_capacity(pixel_count * 3);
    for y in 0..height {
        for x in 0..width {
            let mut channels = [0u8; 3];
            for (index, component) in components.iter().enumerate() {
                let sample_x = x * component.h / max_h;
                let sample_y = y * component.v / max_v;
                channels[index] = component.samples[sample_y * component.stride + sample_x];
            }
            if direct_rgb {
                pixels.extend_from_slice(&channels);
            } else {
                let yy = channels[0] as f64;
                let cb = channels[1] as f64 - 128.0;
                let cr = channels[2] as f64 - 128.0;
                pixels.push(clamp(yy + 1.402 * cr));
                pixels.push(clamp(yy - 0.344_136 * cb - 0.714_136 * cr));
                pixels.push(clamp(yy + 1.772 * cb));
            }
        }
    }
    Image::new(width as u32, height as u32, ColorType::Rgb8, pixels)
}

fn decode_block(
    bits: &mut BitReader<'_>,
    dc_table: &Huffman,
    ac_table: &Huffman,
    quant: &[u16; 64],
    predictor: &mut i32,
) -> Result<[f64; 64]> {
    let mut coefficients = [0f64; 64];
    let dc_size = dc_table.decode(bits)? as usize;
    if dc_size > 11 {
        return Err(ConvertError::corrupt("JPEG DC 系数类别超出基线范围"));
    }
    *predictor += receive_extend(bits, dc_size)? as i32;
    coefficients[0] = *predictor as f64 * quant[0] as f64;
    let mut index = 1usize;
    while index < 64 {
        let symbol = ac_table.decode(bits)?;
        let run = (symbol >> 4) as usize;
        let size = (symbol & 0x0F) as usize;
        if size == 0 {
            if run == 0 {
                break;
            }
            if run != 15 || index + 16 > 64 {
                return Err(ConvertError::corrupt("JPEG AC 零游程无效"));
            }
            index += 16;
            continue;
        }
        index = index
            .checked_add(run)
            .filter(|&index| index < 64)
            .ok_or_else(|| ConvertError::corrupt("JPEG AC 系数游程越界"))?;
        let coefficient = receive_extend(bits, size)? as f64;
        let natural = ZIGZAG[index];
        coefficients[natural] = coefficient * quant[natural] as f64;
        index += 1;
    }
    Ok(coefficients)
}

fn receive_extend(bits: &mut BitReader<'_>, size: usize) -> Result<i32> {
    if size == 0 {
        return Ok(0);
    }
    if size > 16 {
        return Err(ConvertError::corrupt("JPEG 系数位数无效"));
    }
    let value = bits.read_bits(size)? as i32;
    let threshold = 1 << (size - 1);
    Ok(if value < threshold {
        value + 1 - (1 << size)
    } else {
        value
    })
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    byte: u8,
    bits_left: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            byte: 0,
            bits_left: 0,
        }
    }

    fn read_bit(&mut self) -> Result<u8> {
        if self.bits_left == 0 {
            let value = *self
                .data
                .get(self.pos)
                .ok_or_else(|| ConvertError::corrupt("JPEG 熵编码数据不完整"))?;
            self.pos += 1;
            if value == 0xFF {
                let stuffing = *self
                    .data
                    .get(self.pos)
                    .ok_or_else(|| ConvertError::corrupt("JPEG 字节填充不完整"))?;
                if stuffing != 0 {
                    return Err(ConvertError::corrupt("JPEG 熵编码中出现意外标记"));
                }
                self.pos += 1;
            }
            self.byte = value;
            self.bits_left = 8;
        }
        self.bits_left -= 1;
        Ok((self.byte >> self.bits_left) & 1)
    }

    fn read_bits(&mut self, count: usize) -> Result<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.read_bit()? as u32;
        }
        Ok(value)
    }

    fn consume_restart(&mut self, expected: u8) -> Result<()> {
        self.bits_left = 0;
        if self.data.get(self.pos) != Some(&0xFF) {
            return Err(ConvertError::corrupt("JPEG 缺少重启标记"));
        }
        while self.data.get(self.pos) == Some(&0xFF) {
            self.pos += 1;
        }
        let marker = *self
            .data
            .get(self.pos)
            .ok_or_else(|| ConvertError::corrupt("JPEG 重启标记不完整"))?;
        self.pos += 1;
        if marker != 0xD0 + expected {
            return Err(ConvertError::corrupt("JPEG 重启标记顺序错误"));
        }
        Ok(())
    }
}

fn encode_jpeg(image: &Image, quality: u8) -> Result<Vec<u8>> {
    if image.width == 0 || image.height == 0 {
        return Err(ConvertError::corrupt("JPEG 宽高不能为 0"));
    }
    if image.width > u16::MAX as u32 || image.height > u16::MAX as u32 {
        return Err(ConvertError::unsupported(
            "JPEG 单边尺寸不能超过 65535 像素",
        ));
    }
    let expected_len = (image.width as usize)
        .checked_mul(image.height as usize)
        .and_then(|count| count.checked_mul(image.color.channels()))
        .ok_or_else(|| ConvertError::corrupt("JPEG 图像尺寸过大"))?;
    if image.data.len() != expected_len {
        return Err(ConvertError::corrupt("JPEG 像素数据长度不符"));
    }
    let grayscale = image.color.is_gray();
    let components = if grayscale { 1 } else { 3 };
    let luma_quant = scaled_quant(&LUMA_QUANT, quality);
    let chroma_quant = scaled_quant(&CHROMA_QUANT, quality);
    let dc_luma = Huffman::new(&DC_LUMA_BITS, &DC_VALUES)?;
    let dc_chroma = Huffman::new(&DC_CHROMA_BITS, &DC_VALUES)?;
    let ac_luma = Huffman::new(&AC_LUMA_BITS, &AC_LUMA_VALUES)?;
    let ac_chroma = Huffman::new(&AC_CHROMA_BITS, &AC_CHROMA_VALUES)?;
    let mut output = Vec::new();
    output.extend_from_slice(&[0xFF, SOI]);
    write_segment(
        &mut output,
        0xE0,
        &[b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0],
    )?;

    let mut dqt = Vec::with_capacity(if grayscale { 65 } else { 130 });
    dqt.push(0);
    for &index in &ZIGZAG {
        dqt.push(luma_quant[index] as u8);
    }
    if !grayscale {
        dqt.push(1);
        for &index in &ZIGZAG {
            dqt.push(chroma_quant[index] as u8);
        }
    }
    write_segment(&mut output, DQT, &dqt)?;

    let mut frame = vec![
        8,
        (image.height >> 8) as u8,
        image.height as u8,
        (image.width >> 8) as u8,
        image.width as u8,
        components,
        1,
        0x11,
        0,
    ];
    if !grayscale {
        frame.extend_from_slice(&[2, 0x11, 1, 3, 0x11, 1]);
    }
    write_segment(&mut output, SOF0, &frame)?;

    let mut dht = Vec::new();
    append_huffman_definition(&mut dht, 0x00, &DC_LUMA_BITS, &DC_VALUES);
    append_huffman_definition(&mut dht, 0x10, &AC_LUMA_BITS, &AC_LUMA_VALUES);
    if !grayscale {
        append_huffman_definition(&mut dht, 0x01, &DC_CHROMA_BITS, &DC_VALUES);
        append_huffman_definition(&mut dht, 0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALUES);
    }
    write_segment(&mut output, DHT, &dht)?;

    let mut scan = vec![components];
    scan.extend_from_slice(&[1, 0x00]);
    if !grayscale {
        scan.extend_from_slice(&[2, 0x11, 3, 0x11]);
    }
    scan.extend_from_slice(&[0, 63, 0]);
    write_segment(&mut output, SOS, &scan)?;

    let mut writer = BitWriter::new();
    let mut predictors = [0i32; 3];
    let mcu_columns = (image.width as usize).div_ceil(8);
    let mcu_rows = (image.height as usize).div_ceil(8);
    let width = image.width as usize;
    let height = image.height as usize;
    for block_y in 0..mcu_rows {
        for block_x in 0..mcu_columns {
            let mut samples = [[0f64; 64]; 3];
            for y in 0..8 {
                for x in 0..8 {
                    let px = (block_x * 8 + x).min(width - 1) as u32;
                    let py = (block_y * 8 + y).min(height - 1) as u32;
                    let pixel = image.get_pixel(px, py);
                    let offset = y * 8 + x;
                    if grayscale {
                        samples[0][offset] = pixel.luma() as f64 - 128.0;
                    } else {
                        samples[0][offset] = 0.299 * pixel.r as f64
                            + 0.587 * pixel.g as f64
                            + 0.114 * pixel.b as f64
                            - 128.0;
                        samples[1][offset] = -0.168_736 * pixel.r as f64
                            - 0.331_264 * pixel.g as f64
                            + 0.5 * pixel.b as f64;
                        samples[2][offset] = 0.5 * pixel.r as f64
                            - 0.418_688 * pixel.g as f64
                            - 0.081_312 * pixel.b as f64;
                    }
                }
            }
            for component in 0..components as usize {
                let quant = if component == 0 {
                    &luma_quant
                } else {
                    &chroma_quant
                };
                let coefficients = forward_dct(&samples[component], quant);
                let dc = if component == 0 { &dc_luma } else { &dc_chroma };
                let ac = if component == 0 { &ac_luma } else { &ac_chroma };
                encode_block(
                    &mut writer,
                    &coefficients,
                    dc,
                    ac,
                    &mut predictors[component],
                )?;
            }
        }
    }
    output.extend_from_slice(&writer.finish());
    output.extend_from_slice(&[0xFF, EOI]);
    Ok(output)
}

fn scaled_quant(base: &[u8; 64], quality: u8) -> [u16; 64] {
    let quality = quality.clamp(1, 100) as i32;
    let scale = if quality < 50 {
        5000 / quality
    } else {
        200 - quality * 2
    };
    std::array::from_fn(|index| ((base[index] as i32 * scale + 50) / 100).clamp(1, 255) as u16)
}

fn append_huffman_definition(out: &mut Vec<u8>, info: u8, counts: &[u8; 16], values: &[u8]) {
    out.push(info);
    out.extend_from_slice(counts);
    out.extend_from_slice(values);
}

fn write_segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) -> Result<()> {
    let length = payload
        .len()
        .checked_add(2)
        .filter(|&length| length <= u16::MAX as usize)
        .ok_or_else(|| ConvertError::corrupt("JPEG 段超过最大长度"))? as u16;
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

fn forward_dct(samples: &[f64; 64], quant: &[u16; 64]) -> [i32; 64] {
    let cosines = cosine_table();
    let mut horizontal = [0.0; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut sum = 0.0;
            for x in 0..8 {
                sum += samples[y * 8 + x] * cosines[u][x];
            }
            horizontal[y * 8 + u] = sum;
        }
    }

    let mut output = [0i32; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut sum = 0.0;
            for y in 0..8 {
                sum += horizontal[y * 8 + u] * cosines[v][y];
            }
            let cu = if u == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
            let cv = if v == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
            let coefficient = 0.25 * cu * cv * sum;
            output[v * 8 + u] = (coefficient / quant[v * 8 + u] as f64).round() as i32;
        }
    }
    output
}

fn inverse_dct(coefficients: &[f64; 64]) -> [u8; 64] {
    let cosines = cosine_table();
    let mut vertical = [0.0; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut sum = 0.0;
            for v in 0..8 {
                let cv = if v == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
                sum += cv * coefficients[v * 8 + u] * cosines[v][y];
            }
            vertical[y * 8 + u] = sum;
        }
    }

    let mut output = [0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0.0;
            for u in 0..8 {
                let cu = if u == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
                sum += cu * vertical[y * 8 + u] * cosines[u][x];
            }
            output[y * 8 + x] = clamp(0.25 * sum + 128.0);
        }
    }
    output
}

fn cosine_table() -> &'static [[f64; 8]; 8] {
    static COSINES: OnceLock<[[f64; 8]; 8]> = OnceLock::new();
    COSINES.get_or_init(|| {
        std::array::from_fn(|frequency| {
            std::array::from_fn(|sample| {
                (((2 * sample + 1) as f64 * frequency as f64 * PI) / 16.0).cos()
            })
        })
    })
}

fn encode_block(
    writer: &mut BitWriter,
    coefficients: &[i32; 64],
    dc: &Huffman,
    ac: &Huffman,
    predictor: &mut i32,
) -> Result<()> {
    let difference = coefficients[0] - *predictor;
    *predictor = coefficients[0];
    let category = magnitude_bits(difference);
    writer.write_symbol(dc, category as u8)?;
    if category != 0 {
        writer.write_bits(amplitude_bits(difference, category), category)?;
    }

    let mut zero_run = 0usize;
    for &natural_index in ZIGZAG.iter().skip(1) {
        let value = coefficients[natural_index];
        if value == 0 {
            zero_run += 1;
            continue;
        }
        while zero_run >= 16 {
            writer.write_symbol(ac, 0xF0)?;
            zero_run -= 16;
        }
        let size = magnitude_bits(value);
        if size > 10 {
            return Err(ConvertError::corrupt("JPEG AC 系数超出基线范围"));
        }
        writer.write_symbol(ac, ((zero_run as u8) << 4) | size as u8)?;
        writer.write_bits(amplitude_bits(value, size), size)?;
        zero_run = 0;
    }
    if zero_run != 0 {
        writer.write_symbol(ac, 0)?;
    }
    Ok(())
}

fn magnitude_bits(value: i32) -> usize {
    if value == 0 {
        0
    } else {
        (32 - value.unsigned_abs().leading_zeros()) as usize
    }
}

fn amplitude_bits(value: i32, count: usize) -> u32 {
    if value >= 0 {
        value as u32
    } else {
        ((1i32 << count) - 1 + value) as u32
    }
}

struct BitWriter {
    output: Vec<u8>,
    byte: u8,
    bits: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            output: Vec::new(),
            byte: 0,
            bits: 0,
        }
    }

    fn write_symbol(&mut self, table: &Huffman, symbol: u8) -> Result<()> {
        let (code, length) = table.encode[symbol as usize];
        if length == 0 {
            return Err(ConvertError::corrupt(format!(
                "JPEG Huffman 表缺少符号 0x{symbol:02X}"
            )));
        }
        self.write_bits(code as u32, length as usize)
    }

    fn write_bits(&mut self, value: u32, count: usize) -> Result<()> {
        if count > 16 {
            return Err(ConvertError::corrupt("JPEG 熵编码位数超过上限"));
        }
        for bit in (0..count).rev() {
            self.byte = (self.byte << 1) | ((value >> bit) as u8 & 1);
            self.bits += 1;
            if self.bits == 8 {
                self.push_byte(self.byte);
                self.byte = 0;
                self.bits = 0;
            }
        }
        Ok(())
    }

    fn push_byte(&mut self, value: u8) {
        self.output.push(value);
        if value == 0xFF {
            self.output.push(0);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bits != 0 {
            let byte = (self.byte << (8 - self.bits)) | ((1 << (8 - self.bits)) - 1);
            self.push_byte(byte);
        }
        self.output
    }
}

fn clamp(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pixel::Rgba;

    #[test]
    fn sniff_checks_start_of_image_marker() {
        assert!(JpegCodec.sniff(&[0xFF, SOI]));
        assert!(!JpegCodec.sniff(&[0xFF]));
        assert!(!JpegCodec.sniff(&[SOI, 0xFF]));
    }

    #[test]
    fn rgb_image_roundtrips_with_lossy_color() {
        let source = Image::filled(16, 16, ColorType::Rgb8, Rgba::from_rgb(30, 120, 220)).unwrap();
        let encoded = JpegCodec
            .encode(&source, &EncodeOptions::default())
            .unwrap();
        assert!(encoded.starts_with(&[0xFF, SOI]));
        assert!(encoded.ends_with(&[0xFF, EOI]));

        let decoded = JpegCodec.decode(&encoded).unwrap();
        assert_eq!((decoded.width, decoded.height), (16, 16));
        let pixel = decoded.get_pixel(8, 8);
        assert!((pixel.r as i16 - 30).abs() <= 4);
        assert!((pixel.g as i16 - 120).abs() <= 4);
        assert!((pixel.b as i16 - 220).abs() <= 4);
        assert_eq!(pixel.a, 255);
    }

    #[test]
    fn grayscale_is_preserved() {
        let source = Image::filled(9, 10, ColorType::Gray8, Rgba::from_gray(96)).unwrap();
        let encoded = JpegCodec
            .encode(&source, &EncodeOptions::default())
            .unwrap();
        let decoded = JpegCodec.decode(&encoded).unwrap();
        assert_eq!(decoded.color, ColorType::Gray8);
        assert_eq!((decoded.width, decoded.height), (9, 10));
        assert!((decoded.data[0] as i16 - 96).abs() <= 2);
    }

    #[test]
    fn alpha_is_discarded_during_encoding() {
        let source = Image::filled(4, 4, ColorType::Rgba8, Rgba::new(40, 80, 120, 10)).unwrap();
        let encoded = JpegCodec
            .encode(&source, &EncodeOptions::default())
            .unwrap();
        assert_eq!(JpegCodec.decode(&encoded).unwrap().color, ColorType::Rgb8);
    }

    #[test]
    fn invalid_and_truncated_data_returns_errors() {
        assert!(JpegCodec.decode(&[]).is_err());
        assert!(JpegCodec.decode(&[0xFF, SOI]).is_err());
        let image = Image::filled(1, 1, ColorType::Gray8, Rgba::from_gray(0)).unwrap();
        let mut encoded = JpegCodec.encode(&image, &EncodeOptions::default()).unwrap();
        encoded.truncate(encoded.len() - 4);
        assert!(JpegCodec.decode(&encoded).is_err());
    }
}
