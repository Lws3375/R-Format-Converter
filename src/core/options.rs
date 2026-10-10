//! 转换参数定义。
//!
//! 界面层收集到的用户选择会被整理为 [`ConvertOptions`],再交给转换管线执行。
//! 这些结构同时实现了 `serde` 的序列化,因此可以直接保存到配置文件。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::format::Format;
use crate::core::color::ColorType;

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
    /// PNG 极速编码模式(使用轻量级滤波器,大幅减少耗时)。
    #[serde(default = "default_png_fast_mode")]
    pub png_fast_mode: bool,
}

fn default_png_compression_level() -> u8 {
    1
}

fn default_png_fast_mode() -> bool {
    true
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            quality: 90,
            png_compression_level: default_png_compression_level(),
            png_fast_mode: default_png_fast_mode(),
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

/// 压缩预设方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompressPreset {
    /// 极速无损：最快速度，不修改分辨率，零画质损失。
    FastLossless,
    /// 智能均衡（推荐）：画质与体积的黄金平衡，适合日常分享与网页展示。
    #[default]
    Balanced,
    /// 极限压缩：大幅缩减体积，适合邮件附件或存储空间紧张场景。
    MaxSaving,
    /// 自定义：用户自由调节各项参数。
    Custom,
}

impl CompressPreset {
    /// 预设显示名称。
    pub fn name(self) -> &'static str {
        match self {
            CompressPreset::FastLossless => "极速无损",
            CompressPreset::Balanced => "智能均衡",
            CompressPreset::MaxSaving => "极限压缩",
            CompressPreset::Custom => "自定义",
        }
    }

    /// 预设用途说明。
    pub fn hint(self) -> &'static str {
        match self {
            CompressPreset::FastLossless => "零画质损失，最快速度优先，优化无损编码",
            CompressPreset::Balanced => "推荐！画质与体积的最佳平衡，肉眼几乎无法分辨",
            CompressPreset::MaxSaving => "体积大幅缩小 70%~90%，适合对大小要求严苛的场景",
            CompressPreset::Custom => "完全自由配置质量、格式、尺寸与压缩算法",
        }
    }

    /// 全部预设。
    pub fn all() -> [CompressPreset; 4] {
        [
            CompressPreset::FastLossless,
            CompressPreset::Balanced,
            CompressPreset::MaxSaving,
            CompressPreset::Custom,
        ]
    }
}

/// 压缩输出格式策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompressFormatStrategy {
    /// 保持原格式（如 JPEG 压缩为 JPEG，PNG 压缩为 PNG）。
    #[default]
    KeepOriginal,
    /// 转为 WebP（现代高压缩格式，兼顾透明度与超高压缩率）。
    Webp,
    /// 转为 JPEG（兼容性最好，适合各种照片）。
    Jpeg,
}

impl CompressFormatStrategy {
    /// 显示名称。
    pub fn name(self) -> &'static str {
        match self {
            CompressFormatStrategy::KeepOriginal => "保持原格式",
            CompressFormatStrategy::Webp => "转为 WebP (体积最小)",
            CompressFormatStrategy::Jpeg => "转为 JPEG (兼容性好)",
        }
    }

    /// 全部策略。
    pub fn all() -> [CompressFormatStrategy; 3] {
        [
            CompressFormatStrategy::KeepOriginal,
            CompressFormatStrategy::Webp,
            CompressFormatStrategy::Jpeg,
        ]
    }
}

/// 尺寸降采样限制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DownscaleLimit {
    /// 保持原图尺寸。
    #[default]
    Original,
    /// 限制最长边不超过 1920 像素 (全高清)。
    Max1920,
    /// 限制最长边不超过 2560 像素 (2K 分辨率)。
    Max2560,
    /// 按比例缩小至 75%。
    Scale75,
    /// 按比例缩小至 50%。
    Scale50,
    /// 自定义最长边上限。
    CustomMax(u32),
}

impl DownscaleLimit {
    /// 显示名称。
    pub fn name(self) -> &'static str {
        match self {
            DownscaleLimit::Original => "保持原图尺寸",
            DownscaleLimit::Max1920 => "限制最长边 1920px (全高清)",
            DownscaleLimit::Max2560 => "限制最长边 2560px (2K)",
            DownscaleLimit::Scale75 => "缩小至 75%",
            DownscaleLimit::Scale50 => "缩小至 50% (减半)",
            DownscaleLimit::CustomMax(_) => "自定义最长边",
        }
    }

    /// 常见预设枚举。
    pub fn all_presets() -> [DownscaleLimit; 5] {
        [
            DownscaleLimit::Original,
            DownscaleLimit::Max1920,
            DownscaleLimit::Max2560,
            DownscaleLimit::Scale75,
            DownscaleLimit::Scale50,
        ]
    }

    /// 根据原始宽高计算降采样后的目标宽高。若无需缩减则返回原尺寸。
    pub fn calculate_target_size(self, width: u32, height: u32) -> (u32, u32) {
        if width == 0 || height == 0 {
            return (width, height);
        }
        match self {
            DownscaleLimit::Original => (width, height),
            DownscaleLimit::Max1920 => Self::clamp_max_dimension(width, height, 1920),
            DownscaleLimit::Max2560 => Self::clamp_max_dimension(width, height, 2560),
            DownscaleLimit::CustomMax(max_side) => {
                Self::clamp_max_dimension(width, height, max_side.max(1))
            }
            DownscaleLimit::Scale75 => {
                let w = ((width as f64 * 0.75).round() as u32).max(1);
                let h = ((height as f64 * 0.75).round() as u32).max(1);
                (w, h)
            }
            DownscaleLimit::Scale50 => {
                let w = ((width as f64 * 0.50).round() as u32).max(1);
                let h = ((height as f64 * 0.50).round() as u32).max(1);
                (w, h)
            }
        }
    }

    fn clamp_max_dimension(width: u32, height: u32, max_side: u32) -> (u32, u32) {
        let current_max = width.max(height);
        if current_max <= max_side {
            return (width, height);
        }
        let ratio = max_side as f64 / current_max as f64;
        let new_w = ((width as f64 * ratio).round() as u32).max(1);
        let new_h = ((height as f64 * ratio).round() as u32).max(1);
        (new_w, new_h)
    }
}

/// 图片压缩专有参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressOptions {
    /// 压缩预设。
    pub preset: CompressPreset,
    /// 目标格式策略。
    pub format_strategy: CompressFormatStrategy,
    /// 有损质量 (1-100)。
    pub quality: u8,
    /// PNG 压缩等级 (0-9)。
    pub png_level: u8,
    /// 是否开启 PNG 快速编码 (启用快速滤波器大幅提速)。
    pub png_fast_mode: bool,
    /// 尺寸降采样限制。
    pub downscale: DownscaleLimit,
    /// 输出目录，为空时默认与源文件同目录。
    pub output_dir: PathBuf,
    /// 命名规则。
    pub naming: NamingRule,
    /// 是否允许覆盖同名文件。
    pub overwrite: bool,
}

impl Default for CompressOptions {
    fn default() -> Self {
        Self::from_preset(CompressPreset::Balanced)
    }
}

impl CompressOptions {
    /// 根据预设构造默认参数。
    pub fn from_preset(preset: CompressPreset) -> Self {
        match preset {
            CompressPreset::FastLossless => Self {
                preset,
                format_strategy: CompressFormatStrategy::KeepOriginal,
                quality: 100,
                png_level: 1,
                png_fast_mode: true,
                downscale: DownscaleLimit::Original,
                output_dir: PathBuf::new(),
                naming: NamingRule::AppendSuffix,
                overwrite: false,
            },
            CompressPreset::Balanced => Self {
                preset,
                format_strategy: CompressFormatStrategy::KeepOriginal,
                quality: 82,
                png_level: 3,
                png_fast_mode: true,
                downscale: DownscaleLimit::Max2560,
                output_dir: PathBuf::new(),
                naming: NamingRule::AppendSuffix,
                overwrite: false,
            },
            CompressPreset::MaxSaving => Self {
                preset,
                format_strategy: CompressFormatStrategy::KeepOriginal,
                quality: 65,
                png_level: 7,
                png_fast_mode: false,
                downscale: DownscaleLimit::Max1920,
                output_dir: PathBuf::new(),
                naming: NamingRule::AppendSuffix,
                overwrite: false,
            },
            CompressPreset::Custom => Self {
                preset,
                format_strategy: CompressFormatStrategy::KeepOriginal,
                quality: 80,
                png_level: 3,
                png_fast_mode: true,
                downscale: DownscaleLimit::Original,
                output_dir: PathBuf::new(),
                naming: NamingRule::AppendSuffix,
                overwrite: false,
            },
        }
    }

    /// 应用指定预设（覆盖质量、降采样等核心配置）。
    pub fn apply_preset(&mut self, preset: CompressPreset) {
        if preset == CompressPreset::Custom {
            self.preset = CompressPreset::Custom;
            return;
        }
        let template = Self::from_preset(preset);
        self.preset = preset;
        self.quality = template.quality;
        self.png_level = template.png_level;
        self.png_fast_mode = template.png_fast_mode;
        self.downscale = template.downscale;
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

    #[test]
    fn downscale_limit_calculates_correct_dimensions() {
        // 横图 4000x3000
        let (w, h) = DownscaleLimit::Max1920.calculate_target_size(4000, 3000);
        assert_eq!(w, 1920);
        assert_eq!(h, 1440);

        // 竖图 3000x4000
        let (w, h) = DownscaleLimit::Max2560.calculate_target_size(3000, 4000);
        assert_eq!(h, 2560);
        assert_eq!(w, 1920);

        // 小图 800x600 保持不变
        let (w, h) = DownscaleLimit::Max1920.calculate_target_size(800, 600);
        assert_eq!((w, h), (800, 600));

        // 百分比缩放
        let (w, h) = DownscaleLimit::Scale50.calculate_target_size(1000, 800);
        assert_eq!((w, h), (500, 400));
    }

    #[test]
    fn compress_options_preset_and_json_roundtrip() {
        let mut options = CompressOptions::default();
        assert_eq!(options.preset, CompressPreset::Balanced);

        options.apply_preset(CompressPreset::MaxSaving);
        assert_eq!(options.preset, CompressPreset::MaxSaving);
        assert_eq!(options.quality, 65);
        assert_eq!(options.downscale, DownscaleLimit::Max1920);

        let json = serde_json::to_string(&options).unwrap();
        let parsed: CompressOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, options);
    }
}
