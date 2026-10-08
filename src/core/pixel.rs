//! 像素与颜色类型定义。
//!
//! 整个软件内部统一用 [`Rgba`] 表示单个像素,而 [`ColorType`] 只描述内存位图
//! 的存储布局。这样各编解码器只需负责"文件字节 <-> RGBA"的转换,图像变换算法
//! 便与具体格式完全解耦。

use serde::{Deserialize, Serialize};

/// 内存位图的颜色类型,决定每个像素占用的通道数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColorType {
    /// 8 位灰度,1 通道。
    Gray8,
    /// 8 位灰度加透明度,2 通道。
    GrayAlpha8,
    /// 24 位真彩色,3 通道。
    Rgb8,
    /// 32 位真彩色加透明度,4 通道。
    Rgba8,
}

impl ColorType {
    /// 每种颜色类型的通道数。
    pub fn channels(self) -> usize {
        match self {
            ColorType::Gray8 => 1,
            ColorType::GrayAlpha8 => 2,
            ColorType::Rgb8 => 3,
            ColorType::Rgba8 => 4,
        }
    }

    /// 中文名称,用于界面展示。
    pub fn name(self) -> &'static str {
        match self {
            ColorType::Gray8 => "灰度",
            ColorType::GrayAlpha8 => "灰度+透明",
            ColorType::Rgb8 => "真彩色",
            ColorType::Rgba8 => "真彩色+透明",
        }
    }

    /// 是否包含透明度通道。
    pub fn has_alpha(self) -> bool {
        matches!(self, ColorType::GrayAlpha8 | ColorType::Rgba8)
    }

    /// 是否为灰度布局。
    pub fn is_gray(self) -> bool {
        matches!(self, ColorType::Gray8 | ColorType::GrayAlpha8)
    }

    /// 全部颜色类型,便于界面枚举。
    pub fn all() -> [ColorType; 4] {
        [
            ColorType::Gray8,
            ColorType::GrayAlpha8,
            ColorType::Rgb8,
            ColorType::Rgba8,
        ]
    }
}

/// 归一化像素,固定 8 位 RGBA 分量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgba {
    /// 红色分量。
    pub r: u8,
    /// 绿色分量。
    pub g: u8,
    /// 蓝色分量。
    pub b: u8,
    /// 透明度分量,255 表示完全不透明。
    pub a: u8,
}

impl Rgba {
    /// 完全透明的像素。
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    /// 构造一个像素。
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// 由不透明 RGB 构造像素。
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// 由灰度值构造不透明像素。
    pub fn from_gray(value: u8) -> Self {
        Self {
            r: value,
            g: value,
            b: value,
            a: 255,
        }
    }

    /// 按 Rec.601 亮度公式计算灰度值。
    pub fn luma(self) -> u8 {
        // 使用整数权重近似 0.299 / 0.587 / 0.114,避免浮点误差。
        let value = 299 * self.r as u32 + 587 * self.g as u32 + 114 * self.b as u32;
        ((value + 500) / 1000) as u8
    }

    /// 转为灰度像素,保留原透明度。
    pub fn to_gray(self) -> Rgba {
        let value = self.luma();
        Rgba {
            r: value,
            g: value,
            b: value,
            a: self.a,
        }
    }

    /// 转为 4 字节数组。
    pub fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// 由 4 字节数组构造像素。
    pub fn from_array(bytes: [u8; 4]) -> Self {
        Self {
            r: bytes[0],
            g: bytes[1],
            b: bytes[2],
            a: bytes[3],
        }
    }

    /// 线性插值,用于双线性缩放。`t` 取值 0..=255,表示向 `other` 过渡的比例。
    pub fn lerp(self, other: Rgba, t: u32) -> Rgba {
        let mix = |a: u8, b: u8| -> u8 {
            let a = a as u32;
            let b = b as u32;
            ((a * (255 - t) + b * t) / 255) as u8
        };
        Rgba {
            r: mix(self.r, other.r),
            g: mix(self.g, other.g),
            b: mix(self.b, other.b),
            a: mix(self.a, other.a),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_counts_match_layout() {
        assert_eq!(ColorType::Gray8.channels(), 1);
        assert_eq!(ColorType::GrayAlpha8.channels(), 2);
        assert_eq!(ColorType::Rgb8.channels(), 3);
        assert_eq!(ColorType::Rgba8.channels(), 4);
    }

    #[test]
    fn luma_uses_rec601_weights() {
        assert_eq!(Rgba::from_rgb(255, 255, 255).luma(), 255);
        assert_eq!(Rgba::from_rgb(0, 0, 0).luma(), 0);
        // 纯绿在 Rec.601 下约为 150
        let green = Rgba::from_rgb(0, 255, 0).luma();
        assert!((148..=152).contains(&green), "实际值 {green}");
    }

    #[test]
    fn gray_conversion_keeps_alpha() {
        let pixel = Rgba::new(200, 100, 50, 128);
        let gray = pixel.to_gray();
        assert_eq!(gray.a, 128);
        assert_eq!(gray.r, gray.g);
        assert_eq!(gray.g, gray.b);
    }

    #[test]
    fn lerp_endpoints_are_exact() {
        let a = Rgba::from_rgb(0, 0, 0);
        let b = Rgba::from_rgb(255, 128, 64);
        assert_eq!(a.lerp(b, 0), a);
        assert_eq!(a.lerp(b, 255), b);
    }
}
