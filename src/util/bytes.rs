//! 字节流读写工具。
//!
//! 自研编解码器需要频繁地按小端或大端读取整数,标准库只提供了定长数组转换,
//! 缺少带位置指针的游标。这里实现两个基础类型:
//!
//! * [`ByteReader`] —— 只读游标,越界时返回错误而不是 panic,保证解码器面对
//!   损坏文件时是安全的;
//! * [`ByteWriter`] —— 可增长的写入器,支持事后回填(用于先写占位、最后再补
//!   写长度或校验和的场景)。

use crate::core::error::{ConvertError, Result};

/// 只读字节游标。
pub struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    /// 在给定字节切片上创建游标,初始位置为 0。
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// 当前读取位置。
    pub fn position(&self) -> usize {
        self.pos
    }

    /// 底层数据总长度。
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 数据是否为空。
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 剩余可读字节数。
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// 底层数据的只读视图。
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// 绝对定位到指定偏移。
    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            return Err(ConvertError::corrupt(format!(
                "读取位置越界:{} 超出数据长度 {}",
                pos,
                self.data.len()
            )));
        }
        self.pos = pos;
        Ok(())
    }

    /// 跳过若干字节。
    pub fn skip(&mut self, count: usize) -> Result<()> {
        self.seek(self.pos.saturating_add(count))
    }

    /// 内部检查:确保还能读出 `count` 个字节。
    fn ensure(&self, count: usize) -> Result<()> {
        if self.remaining() < count {
            return Err(ConvertError::corrupt(format!(
                "数据不足:需要 {} 字节,剩余 {} 字节",
                count,
                self.remaining()
            )));
        }
        Ok(())
    }

    /// 读取定长字节数组。
    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.ensure(N)?;
        let mut buf = [0u8; N];
        buf.copy_from_slice(&self.data[self.pos..self.pos + N]);
        self.pos += N;
        Ok(buf)
    }

    /// 读取 `count` 个字节,返回借用切片(不拷贝)。
    pub fn read_bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        self.ensure(count)?;
        let slice = &self.data[self.pos..self.pos + count];
        self.pos += count;
        Ok(slice)
    }

    /// 读取 1 字节无符号整数。
    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_array::<1>()?[0])
    }

    /// 读取 2 字节小端无符号整数。
    pub fn read_u16_le(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.read_array::<2>()?))
    }

    /// 读取 2 字节大端无符号整数。
    pub fn read_u16_be(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.read_array::<2>()?))
    }

    /// 读取 4 字节小端无符号整数。
    pub fn read_u32_le(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read_array::<4>()?))
    }

    /// 读取 4 字节大端无符号整数。
    pub fn read_u32_be(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.read_array::<4>()?))
    }

    /// 读取 4 字节小端有符号整数(BMP 中用于表示负高度)。
    pub fn read_i32_le(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.read_array::<4>()?))
    }

    /// 读取 8 字节小端无符号整数。
    pub fn read_u64_le(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.read_array::<8>()?))
    }
}

/// 可增长的字节写入器。
#[derive(Default)]
pub struct ByteWriter {
    data: Vec<u8>,
}

impl ByteWriter {
    /// 创建一个空的写入器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建一个预留了容量的写入器,避免编码大图时反复扩容。
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    /// 已写入的字节数。
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 是否尚未写入任何内容。
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 已写入内容的只读视图。
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    /// 取出内部缓冲区。
    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }

    /// 写入 1 字节。
    pub fn write_u8(&mut self, value: u8) {
        self.data.push(value);
    }

    /// 写入 2 字节小端整数。
    pub fn write_u16_le(&mut self, value: u16) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    /// 写入 2 字节大端整数。
    pub fn write_u16_be(&mut self, value: u16) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// 写入 4 字节小端整数。
    pub fn write_u32_le(&mut self, value: u32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    /// 写入 4 字节大端整数。
    pub fn write_u32_be(&mut self, value: u32) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// 写入 4 字节小端有符号整数。
    pub fn write_i32_le(&mut self, value: i32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    /// 写入 8 字节小端整数。
    pub fn write_u64_le(&mut self, value: u64) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    /// 写入一段字节。
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
    }

    /// 写入 `count` 个 0,用于占位。
    pub fn write_zeros(&mut self, count: usize) {
        self.data.resize(self.data.len() + count, 0);
    }

    /// 把指定偏移处的 4 字节回填为小端整数。
    ///
    /// 编码器常需要先写占位、等数据写完后再补写长度字段。
    pub fn patch_u32_le(&mut self, offset: usize, value: u32) {
        let bytes = value.to_le_bytes();
        if offset + 4 <= self.data.len() {
            self.data[offset..offset + 4].copy_from_slice(&bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_reads_integers_in_both_endianness() {
        let raw = [0x01, 0x02, 0x03, 0x04, 0x05];
        let mut reader = ByteReader::new(&raw);
        assert_eq!(reader.read_u8().unwrap(), 0x01);
        assert_eq!(reader.read_u16_le().unwrap(), 0x0302);
        assert_eq!(reader.read_u16_be().unwrap(), 0x0405);
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn reader_reports_error_instead_of_panicking() {
        let raw = [0x01, 0x02];
        let mut reader = ByteReader::new(&raw);
        assert!(reader.read_u32_le().is_err());
        assert_eq!(reader.position(), 0);
    }

    #[test]
    fn writer_round_trips_with_reader() {
        let mut writer = ByteWriter::new();
        writer.write_u8(0xAA);
        writer.write_u16_be(0x1234);
        writer.write_u32_le(0xDEAD_BEEF);
        writer.write_bytes(b"end");

        let bytes = writer.into_vec();
        let mut reader = ByteReader::new(&bytes);
        assert_eq!(reader.read_u8().unwrap(), 0xAA);
        assert_eq!(reader.read_u16_be().unwrap(), 0x1234);
        assert_eq!(reader.read_u32_le().unwrap(), 0xDEAD_BEEF);
        assert_eq!(reader.read_bytes(3).unwrap(), b"end");
    }

    #[test]
    fn writer_patches_placeholder() {
        let mut writer = ByteWriter::new();
        writer.write_u32_le(0);
        writer.write_bytes(&[1, 2, 3, 4]);
        writer.patch_u32_le(0, 4);
        assert_eq!(writer.as_slice()[..4], [4, 0, 0, 0]);
    }
}
