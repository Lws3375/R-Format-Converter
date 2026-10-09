//! 文件级转换流程。
//!
//! 一次转换由六步组成:读入文件 → 识别来源格式 → 解码为内存位图 → 应用变换 →
//! 编码为目标格式 → 写出文件。整个过程不依赖界面,因此既可以被单文件转换按钮
//! 直接调用,也可以放在后台线程里批量执行。
//!
//! 解码时并不单纯依赖扩展名:先按扩展名尝试,再按文件头嗅探,最后逐个解码器兜底,
//! 这样即使文件被改错扩展名或缺少魔数也能尽量读出来。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::codecs::ICO_MAX_DIMENSION;
use crate::convert::transform::apply_transforms;
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::core::image::{Image, has_semi_transparent_pixel};
use crate::core::options::ConvertOptions;
use crate::core::registry::Registry;
use crate::util::fs::{build_output_path, extension_of, read_file, write_file};

/// 一次转换的完整结果。
#[derive(Debug, Clone)]
pub struct ConversionOutcome {
    /// 输入文件路径。
    pub input: PathBuf,
    /// 实际写出的文件路径。
    pub output: PathBuf,
    /// 识别出的来源格式。
    pub source_format: Format,
    /// 目标格式。
    pub target_format: Format,
    /// 输出图像宽度。
    pub width: u32,
    /// 输出图像高度。
    pub height: u32,
    /// 输入文件大小(字节)。
    pub source_bytes: u64,
    /// 输出文件大小(字节)。
    pub output_bytes: u64,
    /// 本次转换耗时。
    pub elapsed: Duration,
    /// 需要提醒用户的信息,例如体积明显增大。
    pub notes: Vec<String>,
}

impl ConversionOutcome {
    /// 输出体积相对输入的变化百分比。输入为空时返回 0。
    pub fn size_change_percent(&self) -> f64 {
        if self.source_bytes == 0 {
            return 0.0;
        }
        (self.output_bytes as f64 - self.source_bytes as f64) / self.source_bytes as f64 * 100.0
    }

    /// 一句话结果描述,可直接显示在历史记录与状态栏。
    pub fn summary(&self) -> String {
        format!(
            "{} → {} · {}×{} · {} → {} · 用时 {} ms",
            self.source_format.name(),
            self.target_format.name(),
            self.width,
            self.height,
            crate::util::fs::human_size(self.source_bytes),
            crate::util::fs::human_size(self.output_bytes),
            self.elapsed.as_millis()
        )
    }
}

/// 转换一个文件。
///
/// 输出目录为空时写入输入文件所在目录;目标文件已存在且 `overwrite` 为 `false`
/// 时自动追加序号,不会覆盖用户的已有文件。
pub fn convert_file(
    input: &Path,
    registry: &Registry,
    options: &ConvertOptions,
) -> Result<ConversionOutcome> {
    let started = Instant::now();

    let data = read_file(input)?;
    let source_bytes = data.len() as u64;

    let (image, source_format) = decode_input(registry, input, &data)?;
    let image = apply_transforms(image, &options.transform)?;

    preflight(&image, options.target)?;
    let encoded = registry.encode(&image, options.target, &options.encode)?;

    let output_dir = if options.output_dir.as_os_str().is_empty() {
        input
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        options.output_dir.clone()
    };

    let extension = registry.output_extension(&image, options.target);
    let output = build_output_path(
        input,
        &output_dir,
        extension,
        options.naming,
        options.overwrite,
    );
    write_file(&output, &encoded)?;

    let output_bytes = encoded.len() as u64;
    let notes = collect_notes(&image, source_format, options.target, source_bytes, output_bytes);

    log::info!(
        "转换完成:{} -> {}({}×{},{} 字节)",
        input.display(),
        output.display(),
        image.width(),
        image.height(),
        output_bytes
    );

    Ok(ConversionOutcome {
        input: input.to_path_buf(),
        output,
        source_format,
        target_format: options.target,
        width: image.width(),
        height: image.height(),
        source_bytes,
        output_bytes,
        elapsed: started.elapsed(),
        notes,
    })
}

/// 只做内存转换,不读写文件。供界面预览与单元测试使用。
pub fn convert_bytes(data: &[u8], registry: &Registry, options: &ConvertOptions) -> Result<Vec<u8>> {
    let (image, _) = decode_input(registry, Path::new("memory"), data)?;
    let image = apply_transforms(image, &options.transform)?;
    preflight(&image, options.target)?;
    registry.encode(&image, options.target, &options.encode)
}

/// 识别并解码输入数据,返回位图与来源格式。
pub fn decode_input(registry: &Registry, path: &Path, data: &[u8]) -> Result<(Image, Format)> {
    if data.is_empty() {
        return Err(ConvertError::corrupt("文件内容为空"));
    }

    let mut problems: Vec<String> = Vec::new();

    // 第一步:扩展名匹配到的格式通常最可靠。未启用的格式(如 GIF)直接跳过,
    // 以免把"不支持的格式"误报成"文件损坏"。
    if let Some(format) = Format::from_extension(&extension_of(path))
        && registry.is_supported(format)
    {
        match registry.decode_as(data, format) {
            Ok(image) => return Ok((image, format)),
            Err(err) => problems.push(format!("按扩展名「{}」解析失败:{}", format.name(), err)),
        }
    }

    // 第二步:按文件头嗅探,这一步不依赖文件名。
    if let Some(format) = registry.detect(data) {
        match registry.decode_as(data, format) {
            Ok(image) => return Ok((image, format)),
            Err(err) => problems.push(format!("按文件头识别为 {} 后解析失败:{}", format.name(), err)),
        }
    }

    // 第三步:逐个格式尝试,处理扩展名错误且没有魔数的文件(如 TGA)。
    for format in registry.readable_formats() {
        if let Ok(image) = registry.decode_as(data, format) {
            log::warn!("{} 的扩展名与文件头均未匹配,已按 {} 解析", path.display(), format.name());
            return Ok((image, format));
        }
    }

    if problems.is_empty() {
        Err(ConvertError::UnknownFormat)
    } else {
        Err(ConvertError::corrupt(problems.join(";")))
    }
}

/// 编码前的可行性检查,把编码器内部的报错提前成更易懂的提示。
fn preflight(image: &Image, target: Format) -> Result<()> {
    if !target.is_implemented() {
        return Err(ConvertError::unsupported(format!(
            "{}的编解码功能尚未实现,请选择其它目标格式",
            target.name()
        )));
    }
    if target == Format::Ico
        && (image.width() > ICO_MAX_DIMENSION || image.height() > ICO_MAX_DIMENSION)
    {
        return Err(ConvertError::unsupported(format!(
            "ICO 图标最大支持 {}×{},当前图像为 {}×{},请先设置缩放",
            ICO_MAX_DIMENSION,
            ICO_MAX_DIMENSION,
            image.width(),
            image.height()
        )));
    }
    Ok(())
}

/// 汇总需要提醒用户的信息。
fn collect_notes(
    image: &Image,
    source: Format,
    target: Format,
    source_bytes: u64,
    output_bytes: u64,
) -> Vec<String> {
    let mut notes = Vec::new();

    if source == target {
        notes.push("源格式与目标格式相同,文件已被重新编码".to_string());
    }

    if !target.supports_alpha() && has_semi_transparent_pixel(image) {
        notes.push(format!(
            "{} 不支持透明度,半透明像素已按不透明处理",
            target.name()
        ));
    }

    if source_bytes > 0 && output_bytes > source_bytes {
        let percent = (output_bytes - source_bytes) as f64 / source_bytes as f64 * 100.0;
        if percent >= 1.0 {
            notes.push(format!("输出体积比原文件大约 {percent:.0}%"));
        }
    }

    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::build_default;
    use crate::core::options::{NamingRule, TransformOptions};
    use crate::util::fs::read_file as read;
    use crate::util::fs::temp_dir;

    /// 生成一张 6×4 的测试图,含一个半透明像素。
    fn sample() -> Image {
        let mut buffer = image::RgbaImage::new(6, 4);
        for y in 0..4 {
            for x in 0..6 {
                let alpha = if x == 0 && y == 0 { 128 } else { 255 };
                buffer.put_pixel(x, y, image::Rgba([(x * 40) as u8, (y * 60) as u8, 200, alpha]));
            }
        }
        Image::ImageRgba8(buffer)
    }

    #[test]
    fn convert_file_writes_output_and_reports_sizes() {
        let registry = build_default();
        let dir = temp_dir("basic");
        let input = dir.join("photo.bmp");
        let bmp = registry
            .encode(&sample(), Format::Bmp, &Default::default())
            .unwrap();
        std::fs::write(&input, &bmp).unwrap();

        let mut options = ConvertOptions::new(Format::Qoi, dir.join("out"));
        options.naming = NamingRule::KeepOriginal;

        let outcome = convert_file(&input, &registry, &options).unwrap();
        assert_eq!(outcome.source_format, Format::Bmp);
        assert_eq!(outcome.target_format, Format::Qoi);
        assert_eq!((outcome.width, outcome.height), (6, 4));
        assert_eq!(outcome.source_bytes, bmp.len() as u64);
        assert!(outcome.output.exists());
        assert!(outcome.output.to_string_lossy().ends_with("photo.qoi"));

        // 写出的文件能被重新读回,且像素一致。
        let restored = registry.decode(&read(&outcome.output).unwrap()).unwrap();
        assert_eq!(restored.to_rgba8().as_raw(), sample().to_rgba8().as_raw());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn existing_output_is_not_overwritten_by_default() {
        let registry = build_default();
        let dir = temp_dir("keep");
        let input = dir.join("a.bmp");
        std::fs::write(
            &input,
            registry
                .encode(&sample(), Format::Bmp, &Default::default())
                .unwrap(),
        )
        .unwrap();

        let options = ConvertOptions::new(Format::Qoi, dir.clone());
        let first = convert_file(&input, &registry, &options).unwrap();
        let second = convert_file(&input, &registry, &options).unwrap();
        assert_ne!(first.output, second.output);
        assert!(first.output.exists() && second.output.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overwrite_flag_reuses_the_same_path() {
        let registry = build_default();
        let dir = temp_dir("over");
        let input = dir.join("a.bmp");
        std::fs::write(
            &input,
            registry
                .encode(&sample(), Format::Bmp, &Default::default())
                .unwrap(),
        )
        .unwrap();

        let mut options = ConvertOptions::new(Format::Qoi, dir.clone());
        options.overwrite = true;
        let first = convert_file(&input, &registry, &options).unwrap();
        let second = convert_file(&input, &registry, &options).unwrap();
        assert_eq!(first.output, second.output);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn empty_output_dir_falls_back_to_input_directory() {
        let registry = build_default();
        let dir = temp_dir("fallback");
        let input = dir.join("b.bmp");
        std::fs::write(
            &input,
            registry
                .encode(&sample(), Format::Bmp, &Default::default())
                .unwrap(),
        )
        .unwrap();

        let mut options = ConvertOptions::new(Format::Farbfeld, PathBuf::new());
        options.overwrite = true;
        let outcome = convert_file(&input, &registry, &options).unwrap();
        assert_eq!(outcome.output.parent().unwrap(), dir);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn netpbm_output_extension_follows_color_mode() {
        let registry = build_default();
        let dir = temp_dir("ext");
        let input = dir.join("c.bmp");
        std::fs::write(
            &input,
            registry
                .encode(&sample(), Format::Bmp, &Default::default())
                .unwrap(),
        )
        .unwrap();

        let gray = Image::ImageLuma8(image::GrayImage::from_pixel(2, 2, image::Luma([10])));
        std::fs::write(
            dir.join("gray.pgm"),
            registry.encode(&gray, Format::Netpbm, &Default::default()).unwrap(),
        )
        .unwrap();

        let mut options = ConvertOptions::new(Format::Netpbm, dir.clone());
        options.overwrite = true;

        let from_gray = convert_file(&dir.join("gray.pgm"), &registry, &options).unwrap();
        assert!(from_gray.output.to_string_lossy().ends_with("gray.pgm"));

        let from_rgba = convert_file(&input, &registry, &options).unwrap();
        assert!(from_rgba.output.to_string_lossy().ends_with("c.pam"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_input_file_is_reported() {
        let registry = build_default();
        let dir = temp_dir("missing");
        let options = ConvertOptions::new(Format::Qoi, dir.clone());
        let err = convert_file(&dir.join("nope.bmp"), &registry, &options).unwrap_err();
        assert!(matches!(err, ConvertError::InvalidPath(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn empty_file_is_rejected() {
        let registry = build_default();
        let dir = temp_dir("empty");
        let input = dir.join("empty.bmp");
        std::fs::write(&input, b"").unwrap();
        let options = ConvertOptions::new(Format::Qoi, dir.clone());
        let err = convert_file(&input, &registry, &options).unwrap_err();
        assert!(matches!(err, ConvertError::Corrupt(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unsupported_target_is_rejected_before_encoding() {
        let registry = build_default();
        let options = ConvertOptions::new(Format::Gif, PathBuf::new());
        let err = convert_bytes(
            &registry
                .encode(&sample(), Format::Qoi, &Default::default())
                .unwrap(),
            &registry,
            &options,
        )
        .unwrap_err();
        match err {
            ConvertError::UnsupportedFeature(message) => assert!(message.contains("GIF")),
            other => panic!("期望 UnsupportedFeature,实际 {other:?}"),
        }
    }

    #[test]
    fn oversize_image_for_ico_is_rejected_with_hint() {
        let registry = build_default();
        let big = Image::ImageRgba8(image::RgbaImage::new(300, 10));
        let options = ConvertOptions::new(Format::Ico, PathBuf::new());
        let data = registry
            .encode(&big, Format::Farbfeld, &Default::default())
            .unwrap();
        let err = convert_bytes(&data, &registry, &options).unwrap_err();
        match err {
            ConvertError::UnsupportedFeature(message) => assert!(message.contains("256×256")),
            other => panic!("期望 UnsupportedFeature,实际 {other:?}"),
        }
    }

    #[test]
    fn decode_falls_back_when_extension_lies() {
        let registry = build_default();
        // 内容其实是 QOI,却起名 .bmp。
        let data = registry
            .encode(&sample(), Format::Qoi, &Default::default())
            .unwrap();
        let (image, format) = decode_input(&registry, Path::new("fake.bmp"), &data).unwrap();
        assert_eq!(format, Format::Qoi);
        assert_eq!((image.width(), image.height()), (6, 4));
    }

    #[test]
    fn decode_without_extension_uses_magic() {
        let registry = build_default();
        let data = registry
            .encode(&sample(), Format::Qoi, &Default::default())
            .unwrap();
        let (_, format) = decode_input(&registry, Path::new("noext"), &data).unwrap();
        assert_eq!(format, Format::Qoi);
    }

    #[test]
    fn garbage_input_reports_error() {
        let registry = build_default();
        let data = [0u8; 64];
        let err = decode_input(&registry, Path::new("junk.bmp"), &data).unwrap_err();
        assert!(matches!(err, ConvertError::Corrupt(_)));
    }

    #[test]
    fn same_format_conversion_is_noted() {
        let image = sample();
        let notes = collect_notes(&image, Format::Bmp, Format::Bmp, 100, 100);
        assert!(notes.iter().any(|note| note.contains("重新编码")));
    }

    #[test]
    fn size_change_percent_is_computed() {
        let outcome = ConversionOutcome {
            input: PathBuf::from("a"),
            output: PathBuf::from("b"),
            source_format: Format::Bmp,
            target_format: Format::Qoi,
            width: 1,
            height: 1,
            source_bytes: 200,
            output_bytes: 250,
            elapsed: Duration::from_millis(3),
            notes: Vec::new(),
        };
        assert!((outcome.size_change_percent() - 25.0).abs() < 1e-9);
        assert!(outcome.summary().contains("用时 3 ms"));
    }

    #[test]
    fn transform_options_reach_the_output() {
        let registry = build_default();
        let data = registry
            .encode(&sample(), Format::Qoi, &Default::default())
            .unwrap();

        let options = ConvertOptions {
            transform: TransformOptions {
                resize: Some((12, 8)),
                ..Default::default()
            },
            ..ConvertOptions::new(Format::Farbfeld, PathBuf::new())
        };

        let encoded = convert_bytes(&data, &registry, &options).unwrap();
        let restored = registry.decode(&encoded).unwrap();
        assert_eq!((restored.width(), restored.height()), (12, 8));
    }
}
