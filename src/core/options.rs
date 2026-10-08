//! 转换参数定义。
//!
//! 界面层收集到的用户选择会被整理为 [`ConvertOptions`],再交给转换管线执行。
//! 这些结构同时实现了 `serde` 的序列化,因此可以直接保存到配置文件。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::format::Format;
use crate::core::pixel::ColorType;

/// 缩放算法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResizeMode {
    /// 最近邻,速度快,保留硬边缘。
    Nearest,
    /// 双线性,过渡平滑。
    Bilinear,
}

impl ResizeMode {
    /// 界面展示名称。
    pub fn name(self) -> &'static str {
        match self {
            ResizeMode::Nearest => "最近邻",
            ResizeMode::Bilinear => "双线性",
        }
    }

    /// 全部算法,供界面枚举。
    pub fn all() -> [ResizeMode; 2] {
        [ResizeMode::Nearest, ResizeMode::Bilinear]
    }
}

/// 旋转角度(顺时针)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Rotation {
    /// 不旋转。
    #[default]
    None,
    /// 顺时针 90 度。
    Deg90,
    /// 顺时针 180 度。
    Deg180,
    /// 顺时针 270 度。
    Deg270,
}

impl Rotation {
    /// 界面展示名称。
    pub fn name(self) -> &'static str {
        match self {
            Rotation::None => "不旋转",
            Rotation::Deg90 => "顺时针 90°",
            Rotation::Deg180 => "顺时针 180°",
            Rotation::Deg270 => "顺时针 270°",
        }
    }

    /// 全部角度,供界面枚举。
    pub fn all() -> [Rotation; 4] {
        [
            Rotation::None,
            Rotation::Deg90,
            Rotation::Deg180,
            Rotation::Deg270,
        ]
    }
}

/// 输出文件命名规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NamingRule {
    /// 沿用原文件名,仅替换扩展名。
    #[default]
    KeepOriginal,
    /// 在原文件名后追加 `_converted`。
    AppendSuffix,
    /// 在原文件名后追加时间戳。
    Timestamp,
}

impl NamingRule {
    /// 界面展示名称。
    pub fn name(self) -> &'static str {
        match self {
            NamingRule::KeepOriginal => "沿用原文件名",
            NamingRule::AppendSuffix => "追加 _converted 后缀",
            NamingRule::Timestamp => "追加时间戳",
        }
    }

    /// 全部规则,供界面枚举。
    pub fn all() -> [NamingRule; 3] {
        [
            NamingRule::KeepOriginal,
            NamingRule::AppendSuffix,
            NamingRule::Timestamp,
        ]
    }
}

/// 编码参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodeOptions {
    /// 有损格式的质量(1-100)。无损格式忽略该值。
    pub quality: u8,
    /// PNG 压缩等级(0-9),等级越低越快,通常文件也越大。
    #[serde(default = "default_png_compression_level")]
    pub png_compression_level: u8,
}

fn default_png_compression_level() -> u8 {
    1
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            quality: 90,
            png_compression_level: default_png_compression_level(),
        }
    }
}

impl EncodeOptions {
    /// 把质量值约束到合法范围。
    pub fn clamped_quality(&self) -> u8 {
        self.quality.clamp(1, 100)
    }
}

/// 图像变换参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformOptions {
    /// 目标尺寸,`None` 表示保持原尺寸。
    pub resize: Option<(u32, u32)>,
    /// 缩放算法。
    pub resize_mode: ResizeMode,
    /// 是否转为灰度。
    pub grayscale: bool,
    /// 是否水平镜像。
    pub flip_horizontal: bool,
    /// 是否垂直镜像。
    pub flip_vertical: bool,
    /// 旋转角度。
    pub rotate: Rotation,
    /// 目标颜色类型,`None` 表示保持原样。
    pub color: Option<ColorType>,
}

impl Default for TransformOptions {
    fn default() -> Self {
        Self {
            resize: None,
            resize_mode: ResizeMode::Bilinear,
            grayscale: false,
            flip_horizontal: false,
            flip_vertical: false,
            rotate: Rotation::None,
            color: None,
        }
    }
}

impl TransformOptions {
    /// 是否未做任何设置(此时可跳过整个变换阶段)。
    pub fn is_identity(&self) -> bool {
        self.resize.is_none()
            && !self.grayscale
            && !self.flip_horizontal
            && !self.flip_vertical
            && self.rotate == Rotation::None
            && self.color.is_none()
    }

    /// 生成面向用户的变换摘要,用于任务列表展示。
    pub fn summary(&self) -> String {
        if self.is_identity() {
            return "无变换".to_string();
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some((width, height)) = self.resize {
            parts.push(format!(
                "缩放至 {width}×{height}({})",
                self.resize_mode.name()
            ));
        }
        if self.grayscale {
            parts.push("灰度".to_string());
        }
        if self.flip_horizontal {
            parts.push("水平镜像".to_string());
        }
        if self.flip_vertical {
            parts.push("垂直镜像".to_string());
        }
        if self.rotate != Rotation::None {
            parts.push(self.rotate.name().to_string());
        }
        if let Some(color) = self.color {
            parts.push(format!("颜色模式 {}", color.name()));
        }
        parts.join("、")
    }
}

/// 一次转换任务的完整参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertOptions {
    /// 目标格式。
    pub target: Format,
    /// 输出目录。
    pub output_dir: PathBuf,
    /// 命名规则。
    pub naming: NamingRule,
    /// 是否允许覆盖同名文件。
    pub overwrite: bool,
    /// 图像变换参数。
    pub transform: TransformOptions,
    /// 编码参数。
    pub encode: EncodeOptions,
}

impl ConvertOptions {
    /// 以指定目标格式和输出目录构造默认参数。
    pub fn new(target: Format, output_dir: PathBuf) -> Self {
        Self {
            target,
            output_dir,
            naming: NamingRule::default(),
            overwrite: false,
            transform: TransformOptions::default(),
            encode: EncodeOptions::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_transform_is_identity() {
        assert!(TransformOptions::default().is_identity());
        assert_eq!(TransformOptions::default().summary(), "无变换");
    }

    #[test]
    fn summary_lists_enabled_steps() {
        let options = TransformOptions {
            resize: Some((800, 600)),
            grayscale: true,
            rotate: Rotation::Deg90,
            ..Default::default()
        };
        let summary = options.summary();
        assert!(summary.contains("800×600"));
        assert!(summary.contains("灰度"));
        assert!(summary.contains("顺时针 90°"));
        assert!(!options.is_identity());
    }

    #[test]
    fn quality_is_clamped() {
        let options = EncodeOptions {
            quality: 0,
            ..Default::default()
        };
        assert_eq!(options.clamped_quality(), 1);
        let options = EncodeOptions {
            quality: 200,
            ..Default::default()
        };
        assert_eq!(options.clamped_quality(), 100);
    }

    #[test]
    fn old_encode_options_json_uses_default_png_compression() {
        let options: EncodeOptions = serde_json::from_str(r#"{"quality":90}"#).unwrap();
        assert_eq!(options.png_compression_level, 1);
    }

    #[test]
    fn convert_options_roundtrip_through_json() {
        let options = ConvertOptions::new(Format::Qoi, PathBuf::from("C:/out"));
        let text = serde_json::to_string(&options).unwrap();
        let parsed: ConvertOptions = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, options);
    }
}
