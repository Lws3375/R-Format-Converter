//! 格式枚举。
//!
//! 所有格式元信息(名称、扩展名、是否支持透明度)集中在这里定义,界面层的下拉
//! 列表直接从注册表读取,不在界面代码中硬编码格式名称。

use serde::{Deserialize, Serialize};

/// 软件支持的文件格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Format {
    /// Windows 位图。
    Bmp,
    /// Netpbm 家族(PBM/PGM/PPM/PAM)。
    Netpbm,
    /// Truevision TGA。
    Tga,
    /// Quite OK Image。
    Qoi,
    /// farbfeld,16 位 RGBA。
    Farbfeld,
    /// Windows 图标。
    Ico,
    /// 便携式网络图形。
    Png,
    /// 图形交换格式(第二阶段)。
    Gif,
    /// JPEG。
    Jpeg,
}

impl Format {
    /// 全部格式,含尚未实现的预留项。
    pub fn all() -> &'static [Format] {
        &[
            Format::Bmp,
            Format::Netpbm,
            Format::Tga,
            Format::Qoi,
            Format::Farbfeld,
            Format::Ico,
            Format::Png,
            Format::Gif,
            Format::Jpeg,
        ]
    }

    /// 稳定的内部标识,用于配置文件与日志。
    pub fn id(self) -> &'static str {
        match self {
            Format::Bmp => "bmp",
            Format::Netpbm => "netpbm",
            Format::Tga => "tga",
            Format::Qoi => "qoi",
            Format::Farbfeld => "farbfeld",
            Format::Ico => "ico",
            Format::Png => "png",
            Format::Gif => "gif",
            Format::Jpeg => "jpeg",
        }
    }

    /// 界面展示名称。
    pub fn name(self) -> &'static str {
        match self {
            Format::Bmp => "BMP 位图",
            Format::Netpbm => "Netpbm 图像",
            Format::Tga => "TGA 图像",
            Format::Qoi => "QOI 图像",
            Format::Farbfeld => "farbfeld 图像",
            Format::Ico => "ICO 图标",
            Format::Png => "PNG 图像",
            Format::Gif => "GIF 图像",
            Format::Jpeg => "JPEG 图像",
        }
    }

    /// 格式简介,展示在界面提示区。
    pub fn description(self) -> &'static str {
        match self {
            Format::Bmp => "Windows 标准位图,支持 1/4/8/16/24/32 位与 RLE 压缩",
            Format::Netpbm => "科研常用简单格式,含 PBM/PGM/PPM/PAM 四类",
            Format::Tga => "游戏与影视常用,支持调色板与 RLE 压缩",
            Format::Qoi => "为快速编解码设计的现代无损格式",
            Format::Farbfeld => "16 位每通道的高精度无损格式",
            Format::Ico => "Windows 图标容器,内部为 BMP 结构",
            Format::Png => "支持透明度与隔行的无损压缩图像格式",
            Format::Gif => "支持动画与调色板,计划于第二阶段实现",
            Format::Jpeg => "有损照片格式,不支持透明度",
        }
    }

    /// 该格式关联的全部扩展名(小写,不含点)。
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Format::Bmp => &["bmp", "dib"],
            Format::Netpbm => &["pbm", "pgm", "ppm", "pnm", "pam"],
            Format::Tga => &["tga", "targa", "icb", "vda", "vst"],
            Format::Qoi => &["qoi"],
            Format::Farbfeld => &["ff"],
            Format::Ico => &["ico"],
            Format::Png => &["png"],
            Format::Gif => &["gif"],
            Format::Jpeg => &["jpg", "jpeg", "jpe"],
        }
    }

    /// 写出文件时使用的首选扩展名。
    pub fn primary_extension(self) -> &'static str {
        match self {
            Format::Bmp => "bmp",
            Format::Netpbm => "ppm",
            Format::Tga => "tga",
            Format::Qoi => "qoi",
            Format::Farbfeld => "ff",
            Format::Ico => "ico",
            Format::Png => "png",
            Format::Gif => "gif",
            Format::Jpeg => "jpg",
        }
    }

    /// 该格式是否支持透明度。
    pub fn supports_alpha(self) -> bool {
        !matches!(self, Format::Jpeg)
    }

    /// 是否为无损格式。
    pub fn is_lossless(self) -> bool {
        !matches!(self, Format::Jpeg)
    }

    /// 该格式是否已实现。
    pub fn is_implemented(self) -> bool {
        !matches!(self, Format::Gif)
    }

    /// 由扩展名推断格式。
    pub fn from_extension(ext: &str) -> Option<Format> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        Format::all()
            .iter()
            .copied()
            .find(|format| format.extensions().contains(&ext.as_str()))
    }

    /// 由内部标识推断格式。
    pub fn from_id(id: &str) -> Option<Format> {
        let id = id.trim().to_ascii_lowercase();
        Format::all().iter().copied().find(|f| f.id() == id)
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_lookup_is_case_insensitive() {
        assert_eq!(Format::from_extension("BMP"), Some(Format::Bmp));
        assert_eq!(Format::from_extension(".jpeg"), Some(Format::Jpeg));
        assert_eq!(Format::from_extension("pgm"), Some(Format::Netpbm));
        assert_eq!(Format::from_extension("webp"), None);
    }

    #[test]
    fn id_lookup_roundtrips() {
        for format in Format::all() {
            assert_eq!(Format::from_id(format.id()), Some(*format));
        }
    }

    #[test]
    fn primary_extension_is_listed() {
        for format in Format::all() {
            assert!(
                format.extensions().contains(&format.primary_extension()),
                "{} 的首选扩展名未出现在扩展名列表中",
                format.id()
            );
        }
    }

    #[test]
    fn unimplemented_formats_are_marked_unimplemented() {
        assert!(Format::Png.is_implemented());
        assert!(!Format::Gif.is_implemented());
        assert!(Format::Jpeg.is_implemented());
        assert!(Format::Bmp.is_implemented());
    }
}
