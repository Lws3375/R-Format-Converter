//! 颜色模式。
//!
//! [`ColorType`] 只描述内存位图每个像素占用的通道数与含义,真正的像素数据由
//! `image` 库的 `DynamicImage` 承载。界面层的"目标颜色"下拉框与转换管线共用
//! 这一枚举,因此它需要序列化以保存到配置文件。

use serde::{Deserialize, Serialize};

/// 位图颜色模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColorType {
    /// 单通道灰度。
    Gray8,
    /// 灰度加透明度。
    GrayAlpha8,
    /// 三通道真彩色。
    Rgb8,
    /// 三通道真彩色加透明度。
    Rgba8,
}

impl ColorType {
    /// 每个像素占用的字节数。
    pub fn channels(self) -> usize {
        match self {
            ColorType::Gray8 => 1,
            ColorType::GrayAlpha8 => 2,
            ColorType::Rgb8 => 3,
            ColorType::Rgba8 => 4,
        }
    }

    /// 界面展示名称。
    pub fn name(self) -> &'static str {
        match self {
            ColorType::Gray8 => "灰度",
            ColorType::GrayAlpha8 => "灰度+透明",
            ColorType::Rgb8 => "真彩色",
            ColorType::Rgba8 => "真彩色+透明",
        }
    }

    /// 是否含透明度通道。
    pub fn has_alpha(self) -> bool {
        matches!(self, ColorType::GrayAlpha8 | ColorType::Rgba8)
    }

    /// 是否为灰度模式。
    pub fn is_gray(self) -> bool {
        matches!(self, ColorType::Gray8 | ColorType::GrayAlpha8)
    }

    /// 全部模式,界面下拉框按此顺序展示。
    pub fn all() -> [ColorType; 4] {
        [
            ColorType::Gray8,
            ColorType::GrayAlpha8,
            ColorType::Rgb8,
            ColorType::Rgba8,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_counts_match_mode() {
        assert_eq!(ColorType::Gray8.channels(), 1);
        assert_eq!(ColorType::GrayAlpha8.channels(), 2);
        assert_eq!(ColorType::Rgb8.channels(), 3);
        assert_eq!(ColorType::Rgba8.channels(), 4);
    }

    #[test]
    fn alpha_and_gray_flags_are_consistent() {
        assert!(ColorType::Rgba8.has_alpha());
        assert!(!ColorType::Rgb8.has_alpha());
        assert!(ColorType::GrayAlpha8.is_gray());
        assert!(!ColorType::Rgb8.is_gray());
        assert_eq!(ColorType::all().len(), 4);
    }
}
