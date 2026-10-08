//! 用户配置的读写。
//!
//! 配置以 JSON 形式保存在应用数据目录下,只保存用户真正会调整的选项。读取失败
//! 时一律回退到默认值,保证软件在任何情况下都能启动。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::options::{ConvertOptions, EncodeOptions, NamingRule, TransformOptions};
use crate::util::fs::{app_data_dir, ensure_dir, read_file, write_file};

/// 配置文件名。
pub const CONFIG_FILE: &str = "config.json";

/// 界面配色方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeMode {
    /// 跟随操作系统。
    #[default]
    System,
    /// 强制浅色。
    Light,
    /// 强制深色。
    Dark,
}

impl ThemeMode {
    /// 界面展示名称。
    pub fn name(self) -> &'static str {
        match self {
            ThemeMode::System => "跟随系统",
            ThemeMode::Light => "浅色",
            ThemeMode::Dark => "深色",
        }
    }

    /// 全部配色方案,供界面枚举。
    pub fn all() -> [ThemeMode; 3] {
        [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark]
    }
}

/// 应用配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    /// 上次使用的目标格式。
    pub target_format: Format,
    /// 上次使用的输出目录,`None` 表示与源文件同目录。
    pub output_dir: Option<PathBuf>,
    /// 输出文件命名规则。
    pub naming: NamingRule,
    /// 是否允许覆盖同名文件。
    pub overwrite: bool,
    /// 图像变换参数。
    pub transform: TransformOptions,
    /// 编码参数。
    pub encode: EncodeOptions,
    /// 批量转换的并行线程数。
    pub max_workers: usize,
    /// 界面配色方案。
    pub theme: ThemeMode,
    /// 上次浏览的目录,方便下次打开对话框时定位。
    pub last_input_dir: Option<PathBuf>,
    /// 是否记录转换历史。
    pub keep_history: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            target_format: Format::Qoi,
            output_dir: None,
            naming: NamingRule::default(),
            overwrite: false,
            transform: TransformOptions::default(),
            encode: EncodeOptions::default(),
            max_workers: default_workers(),
            theme: ThemeMode::default(),
            last_input_dir: None,
            keep_history: true,
        }
    }
}

/// 默认并行线程数:取 CPU 逻辑核心数,并限制在 1~8 之间。
///
/// 图片转换是内存与磁盘混合负载,线程过多反而会因为磁盘竞争变慢。
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .clamp(1, 8)
}

impl AppConfig {
    /// 配置文件路径。
    pub fn path() -> PathBuf {
        app_data_dir().join(CONFIG_FILE)
    }

    /// 从默认位置读取配置,任何错误都回退到默认值。
    pub fn load() -> Self {
        Self::load_from(&Self::path()).unwrap_or_default()
    }

    /// 从指定文件读取配置。
    pub fn load_from(path: &Path) -> Result<Self> {
        let bytes = read_file(path)?;
        let mut config: AppConfig = serde_json::from_slice(&bytes)?;
        config.normalize();
        Ok(config)
    }

    /// 写入默认位置。
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    /// 写入指定文件。
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            ensure_dir(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        write_file(path, text.as_bytes())
    }

    /// 把可能越界的字段修正到合法范围。
    pub fn normalize(&mut self) {
        self.max_workers = self.max_workers.clamp(1, 64);
        self.encode.quality = self.encode.quality.clamp(1, 100);
        self.encode.png_compression_level = self.encode.png_compression_level.min(9);
        if let Some((width, height)) = self.transform.resize {
            // 尺寸为 0 无法生成有效图像,直接清除该设置。
            if width == 0 || height == 0 {
                self.transform.resize = None;
            } else {
                self.transform.resize = Some((width.min(MAX_DIMENSION), height.min(MAX_DIMENSION)));
            }
        }
    }

    /// 由当前配置构造一次转换任务的参数。
    pub fn to_convert_options(&self) -> ConvertOptions {
        ConvertOptions {
            target: self.target_format,
            output_dir: self.output_dir.clone().unwrap_or_default(),
            naming: self.naming,
            overwrite: self.overwrite,
            transform: self.transform.clone(),
            encode: self.encode,
        }
    }

    /// 用最近一次转换的设置更新配置,便于下次启动直接复用。
    pub fn adopt(&mut self, options: &ConvertOptions) {
        self.target_format = options.target;
        self.output_dir = if options.output_dir.as_os_str().is_empty() {
            None
        } else {
            Some(options.output_dir.clone())
        };
        self.naming = options.naming;
        self.overwrite = options.overwrite;
        self.transform = options.transform.clone();
        self.encode = options.encode;
    }
}

/// 单边最大像素数,防止用户误输入天文数字导致内存耗尽。
pub const MAX_DIMENSION: u32 = 20000;

/// 把配置序列化为便于查看的 JSON 文本。
pub fn to_json(config: &AppConfig) -> Result<String> {
    serde_json::to_string_pretty(config).map_err(ConvertError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::fs::temp_dir;

    #[test]
    fn default_config_is_sane() {
        let config = AppConfig::default();
        assert!(config.max_workers >= 1 && config.max_workers <= 8);
        assert_eq!(config.encode.quality, 90);
        assert!(config.transform.is_identity());
    }

    #[test]
    fn config_round_trips_through_file() {
        let dir = temp_dir("config");
        let path = dir.join("config.json");

        let config = AppConfig {
            target_format: Format::Tga,
            output_dir: Some(PathBuf::from("D:/out")),
            overwrite: true,
            theme: ThemeMode::Dark,
            transform: TransformOptions {
                resize: Some((320, 240)),
                ..Default::default()
            },
            ..Default::default()
        };
        config.save_to(&path).unwrap();

        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded, config);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_falls_back_to_default() {
        let dir = temp_dir("config-missing");
        assert!(AppConfig::load_from(&dir.join("nope.json")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn normalize_fixes_out_of_range_values() {
        let mut config = AppConfig {
            max_workers: 0,
            encode: EncodeOptions {
                quality: 250,
                png_compression_level: 20,
            },
            transform: TransformOptions {
                resize: Some((0, 100)),
                ..Default::default()
            },
            ..Default::default()
        };
        config.normalize();
        assert_eq!(config.max_workers, 1);
        assert_eq!(config.encode.quality, 100);
        assert_eq!(config.encode.png_compression_level, 9);
        assert_eq!(config.transform.resize, None);

        config.transform.resize = Some((999_999, 100));
        config.normalize();
        assert_eq!(config.transform.resize, Some((MAX_DIMENSION, 100)));
    }

    #[test]
    fn adopt_keeps_last_used_settings() {
        let mut config = AppConfig::default();
        let mut options = ConvertOptions::new(Format::Ico, PathBuf::from("E:/icons"));
        options.overwrite = true;
        options.naming = NamingRule::AppendSuffix;
        options.transform.grayscale = true;
        config.adopt(&options);

        assert_eq!(config.target_format, Format::Ico);
        assert_eq!(config.output_dir, Some(PathBuf::from("E:/icons")));
        assert_eq!(config.naming, NamingRule::AppendSuffix);
        assert!(config.overwrite);
        assert!(config.transform.grayscale);
    }

    #[test]
    fn empty_output_dir_becomes_none() {
        let mut config = AppConfig::default();
        config.adopt(&ConvertOptions::new(Format::Bmp, PathBuf::new()));
        assert_eq!(config.output_dir, None);
        assert!(
            config
                .to_convert_options()
                .output_dir
                .as_os_str()
                .is_empty()
        );
    }

    #[test]
    fn theme_names_are_listed() {
        for theme in ThemeMode::all() {
            assert!(!theme.name().is_empty());
        }
    }

    #[test]
    fn json_output_is_readable() {
        let text = to_json(&AppConfig::default()).unwrap();
        assert!(text.contains("max_workers"));
        assert!(text.contains('\n'));
    }
}
