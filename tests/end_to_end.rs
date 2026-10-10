//! 端到端集成测试。
//!
//! 单元测试分别验证各个模块,这个文件反过来只通过公开接口把整条链路串起来:
//! 造出输入文件 → 走完整的转换管线落到磁盘 → 再把结果读回来比对像素。批次测试
//! 还会真正启动后台线程池,验证任务事件的顺序与汇总。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use r_format_converter::codecs::build_default;
use r_format_converter::convert::{apply_transforms, compress_file, convert_file};
use r_format_converter::core::color::ColorType;
use r_format_converter::core::format::Format;
use r_format_converter::core::image::{Image, color_type};
use r_format_converter::core::options::{
    CompressFormatStrategy, CompressOptions, CompressPreset, ConvertOptions, DownscaleLimit,
    EncodeOptions, ResizeMode, Rotation,
};
use r_format_converter::core::registry::Registry;
use r_format_converter::service::task::{TaskEvent, TaskRunner, TaskSummary};
use r_format_converter::util::fs;

/// 临时目录守卫:测试结束(包括断言失败)时自动清理。
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rfc-e2e-{tag}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("创建临时目录失败");
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// 建一个子目录并返回路径。
    fn subdir(&self, name: &str) -> PathBuf {
        let dir = self.join(name);
        std::fs::create_dir_all(&dir).expect("创建子目录失败");
        dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 造一张带水平、垂直与对角渐变的彩色图,便于发现通道或行列错位。
fn gradient_image(width: u32, height: u32) -> Image {
    let mut buffer = image::RgbImage::new(width, height);
    let span = (width + height).max(1);
    for y in 0..height {
        for x in 0..width {
            let red = (x * 255 / width.max(1)) as u8;
            let green = (y * 255 / height.max(1)) as u8;
            let blue = ((x + y) * 255 / span) as u8;
            buffer.put_pixel(x, y, image::Rgb([red, green, blue]));
        }
    }
    Image::ImageRgb8(buffer)
}

/// 把图像按指定格式写成文件,作为后续转换的输入。
fn write_image(registry: &Registry, image: &Image, format: Format, path: &Path) {
    let bytes = registry
        .encode(image, format, &EncodeOptions::default())
        .unwrap_or_else(|error| panic!("编码 {} 失败:{error}", format.name()));
    fs::write_file(path, &bytes)
        .unwrap_or_else(|error| panic!("写入 {} 失败:{error}", path.display()));
}

#[test]
fn every_writable_format_survives_a_round_trip() {
    let registry = build_default();
    let scratch = Scratch::new("round-trip");

    let source = gradient_image(37, 23);
    let input = scratch.join("source.bmp");
    write_image(&registry, &source, Format::Bmp, &input);
    let expected = source.to_rgba8();

    let targets = registry.writable_formats();
    assert!(!targets.is_empty(), "至少要有一个可写格式");

    for target in targets {
        let output_dir = scratch.subdir(target.id());
        let options = ConvertOptions::new(target, output_dir);

        let outcome = convert_file(&input, &registry, &options)
            .unwrap_or_else(|error| panic!("转换为 {} 失败:{error}", target.name()));

        assert_eq!(outcome.source_format, Format::Bmp);
        assert_eq!(outcome.target_format, target);
        assert_eq!((outcome.width, outcome.height), (37, 23));
        assert!(outcome.output_bytes > 0, "{} 输出为空", target.name());
        assert!(outcome.output.exists(), "{} 未落盘", target.name());
        assert_eq!(
            outcome.output_bytes,
            std::fs::metadata(&outcome.output).unwrap().len()
        );

        let bytes = std::fs::read(&outcome.output).expect("读取输出失败");
        let decoded = registry
            .decode_as(&bytes, target)
            .unwrap_or_else(|error| panic!("{} 回读失败:{error}", target.name()));

        assert_eq!(decoded.width(), 37, "{} 宽度不一致", target.name());
        assert_eq!(decoded.height(), 23, "{} 高度不一致", target.name());
        let actual = decoded.to_rgba8();
        if target.is_lossless() {
            assert_eq!(
                actual.as_raw(),
                expected.as_raw(),
                "{} 往返后像素发生变化",
                target.name()
            );
        } else {
            let total_error: u64 = actual
                .as_raw()
                .iter()
                .zip(expected.as_raw())
                .map(|(actual, expected)| actual.abs_diff(*expected) as u64)
                .sum();
            let mean_error = total_error as f64 / expected.as_raw().len() as f64;
            assert!(
                mean_error <= 20.0,
                "{} 往返平均通道误差过大:{mean_error}",
                target.name()
            );
        }
    }
}

#[test]
fn gif_round_trip_conversion_and_notes() {
    let registry = build_default();
    let scratch = Scratch::new("gif-test");

    // 渐变图，转为 GIF
    let source = gradient_image(40, 30);
    let input = scratch.join("source.bmp");
    write_image(&registry, &source, Format::Bmp, &input);

    let gif_dir = scratch.subdir("gif_out");
    let options = ConvertOptions::new(Format::Gif, gif_dir);
    let outcome = convert_file(&input, &registry, &options).expect("转为 GIF 应当成功");
    assert_eq!(outcome.target_format, Format::Gif);
    assert_eq!((outcome.width, outcome.height), (40, 30));
    assert!(outcome.output.exists());

    // 从输出的 GIF 转换为 PNG
    let png_dir = scratch.subdir("png_out");
    let options_png = ConvertOptions::new(Format::Png, png_dir);
    let outcome_png = convert_file(&outcome.output, &registry, &options_png).expect("从 GIF 转为 PNG 应当成功");
    assert_eq!(outcome_png.source_format, Format::Gif);
    assert_eq!(outcome_png.target_format, Format::Png);
    assert_eq!((outcome_png.width, outcome_png.height), (40, 30));
}

#[test]
fn webp_and_svg_round_trip_conversions() {
    let registry = build_default();
    let scratch = Scratch::new("webp-svg");

    let source = gradient_image(48, 36);
    let input = scratch.join("source.bmp");
    write_image(&registry, &source, Format::Bmp, &input);

    // BMP -> WebP -> PNG
    let webp_dir = scratch.subdir("webp_out");
    let options_webp = ConvertOptions::new(Format::Webp, webp_dir);
    let outcome_webp = convert_file(&input, &registry, &options_webp).expect("转为 WebP 应当成功");
    assert_eq!(outcome_webp.target_format, Format::Webp);
    assert_eq!((outcome_webp.width, outcome_webp.height), (48, 36));

    let png_from_webp_dir = scratch.subdir("png_from_webp");
    let outcome_png1 = convert_file(
        &outcome_webp.output,
        &registry,
        &ConvertOptions::new(Format::Png, png_from_webp_dir),
    )
    .expect("从 WebP 转为 PNG 应当成功");
    assert_eq!(outcome_png1.source_format, Format::Webp);

    // BMP -> SVG -> PNG
    let svg_dir = scratch.subdir("svg_out");
    let options_svg = ConvertOptions::new(Format::Svg, svg_dir);
    let outcome_svg = convert_file(&input, &registry, &options_svg).expect("转为 SVG 应当成功");
    assert_eq!(outcome_svg.target_format, Format::Svg);
    assert_eq!((outcome_svg.width, outcome_svg.height), (48, 36));

    let png_from_svg_dir = scratch.subdir("png_from_svg");
    let outcome_png2 = convert_file(
        &outcome_svg.output,
        &registry,
        &ConvertOptions::new(Format::Png, png_from_svg_dir),
    )
    .expect("从 SVG 转为 PNG 应当成功");
    assert_eq!(outcome_png2.source_format, Format::Svg);
}

#[test]
fn source_format_is_detected_from_content_not_extension() {
    let registry = build_default();
    let scratch = Scratch::new("sniff");

    // 故意把 QOI 內容写成 .bmp 后缀,管线应当按文件头认出真实格式。
    let source = gradient_image(24, 16);
    let mislabeled = scratch.join("actually-qoi.bmp");
    write_image(&registry, &source, Format::Qoi, &mislabeled);

    let options = ConvertOptions::new(Format::Farbfeld, scratch.subdir("out"));
    let outcome = convert_file(&mislabeled, &registry, &options).expect("按内容识别应当成功");

    assert_eq!(outcome.source_format, Format::Qoi);
    assert_eq!(outcome.target_format, Format::Farbfeld);

    let bytes = std::fs::read(&outcome.output).unwrap();
    let decoded = registry.decode_as(&bytes, Format::Farbfeld).unwrap();
    assert_eq!(
        decoded.to_rgba8().as_raw(),
        source.to_rgba8().as_raw(),
        "内容识别后往返仍应无损"
    );
}

#[test]
fn transforms_run_in_documented_order_before_encoding() {
    let registry = build_default();
    let scratch = Scratch::new("transform");

    let source = gradient_image(64, 32);
    let input = scratch.join("source.bmp");
    write_image(&registry, &source, Format::Bmp, &input);

    let mut options = ConvertOptions::new(Format::Qoi, scratch.subdir("out"));
    options.transform.rotate = Rotation::Deg90;
    options.transform.resize = Some((16, 8));
    options.transform.resize_mode = ResizeMode::Nearest;
    options.transform.grayscale = true;

    let outcome = convert_file(&input, &registry, &options).expect("带变换的转换应当成功");

    // 先旋转(64x32 → 32x64)再缩放成 16x8。
    assert_eq!((outcome.width, outcome.height), (16, 8));
    assert_eq!(outcome.target_format, Format::Qoi);
    assert!(outcome.size_change_percent().is_finite());

    // 直接调变换管线,确认灰度把颜色模式降成了单通道。
    let transformed = apply_transforms(source.clone(), &options.transform).expect("变换应当成功");
    assert_eq!((transformed.width(), transformed.height()), (16, 8));
    assert_eq!(
        color_type(&transformed),
        ColorType::Gray8,
        "变换结果应当是单通道灰度图"
    );

    // QOI 没有灰度色彩空间,落盘时按 RGB 保存,所以这里按像素判断灰度。
    let bytes = std::fs::read(&outcome.output).unwrap();
    let decoded = registry.decode_as(&bytes, Format::Qoi).unwrap();
    assert!(
        decoded
            .to_rgba8()
            .as_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2]),
        "输出像素的三通道应当相等"
    );
}

#[test]
fn oversized_ico_target_is_rejected_before_writing() {
    let registry = build_default();
    let scratch = Scratch::new("ico-limit");

    let input = scratch.join("large.bmp");
    write_image(&registry, &gradient_image(300, 300), Format::Bmp, &input);

    let output_dir = scratch.subdir("out");
    let options = ConvertOptions::new(Format::Ico, output_dir.clone());
    let error = convert_file(&input, &registry, &options).expect_err("超出 ICO 尺寸上限应当失败");

    assert!(
        error.user_message().contains("256"),
        "错误提示应当说明尺寸上限,实际为:{}",
        error.user_message()
    );
    assert!(
        std::fs::read_dir(&output_dir).unwrap().next().is_none(),
        "预检查失败时不应留下输出文件"
    );
}

#[test]
fn batch_runner_converts_every_file_across_worker_threads() {
    let registry = Arc::new(build_default());
    let scratch = Scratch::new("batch");
    let output_dir = scratch.subdir("out");

    let mut files = Vec::new();
    for index in 0..7u32 {
        let path = scratch.join(&format!("input-{index}.bmp"));
        write_image(
            &registry,
            &gradient_image(20 + index, 12 + index),
            Format::Bmp,
            &path,
        );
        files.push(path);
    }

    let options = ConvertOptions::new(Format::Farbfeld, output_dir);
    let runner = TaskRunner::spawn(files, Arc::clone(&registry), options, 3);

    let mut started = 0;
    // 多线程下完成顺序不确定,按事件里的下标归档,最后再按下标核对。
    let mut outputs: Vec<(usize, PathBuf)> = Vec::new();
    let mut summary: Option<TaskSummary> = None;
    let deadline = Instant::now() + Duration::from_secs(60);

    while summary.is_none() {
        for event in runner.drain() {
            match event {
                TaskEvent::Started { total } => started = total,
                TaskEvent::FileFinished { index, outcome } => {
                    assert!(index < 7, "索引越界:{index}");
                    outputs.push((index, outcome.output.clone()));
                }
                TaskEvent::FileFailed { index, message, .. } => {
                    panic!("第 {index} 个文件意外失败:{message}")
                }
                TaskEvent::Finished(value) => summary = Some(value),
                TaskEvent::FileStarted { .. } => {}
            }
        }

        if Instant::now() > deadline {
            runner.cancel();
            panic!("批量任务超时");
        }
        thread::sleep(Duration::from_millis(5));
    }

    let summary = summary.unwrap();
    assert_eq!(started, 7);
    assert_eq!(summary.total, 7);
    assert_eq!(summary.succeeded, 7);
    assert_eq!(summary.failed, 0);
    assert!(!summary.cancelled);
    assert_eq!(summary.processed(), 7);
    assert!((summary.progress() - 1.0).abs() < f32::EPSILON);
    assert!(!runner.is_running());

    assert_eq!(outputs.len(), 7);
    outputs.sort_by_key(|(index, _)| *index);
    for (position, (index, output)) in outputs.iter().enumerate() {
        assert_eq!(*index, position, "每个下标都应当恰好出现一次");
        assert!(output.exists(), "缺少输出文件 {}", output.display());

        let bytes = std::fs::read(output).expect("读取输出失败");
        let decoded = registry
            .decode_as(&bytes, Format::Farbfeld)
            .expect("回读输出失败");

        // 输入尺寸是 (20 + index, 12 + index),输出应当保持不受影响。
        let expected = (20 + *index as u32, 12 + *index as u32);
        assert_eq!(
            (decoded.width(), decoded.height()),
            expected,
            "第 {index} 个输出尺寸不符"
        );
        assert_eq!(
            decoded.to_rgba8().as_raw(),
            gradient_image(expected.0, expected.1).to_rgba8().as_raw(),
            "第 {index} 个输出像素与源图不符"
        );
    }
}

#[test]
fn cancellation_leaves_a_consistent_summary() {
    let registry = Arc::new(build_default());
    let scratch = Scratch::new("cancel");

    let mut files = Vec::new();
    for index in 0..40u32 {
        let path = scratch.join(&format!("input-{index}.bmp"));
        write_image(&registry, &gradient_image(48, 48), Format::Bmp, &path);
        files.push(path);
    }

    let options = ConvertOptions::new(Format::Tga, scratch.subdir("out"));
    let runner = TaskRunner::spawn(files, Arc::clone(&registry), options, 1);
    runner.cancel();

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut summary: Option<TaskSummary> = None;
    while summary.is_none() {
        for event in runner.drain() {
            if let TaskEvent::Finished(value) = event {
                summary = Some(value);
            }
        }
        if Instant::now() > deadline {
            panic!("取消后任务未能及时结束");
        }
        thread::sleep(Duration::from_millis(5));
    }

    let summary = summary.unwrap();
    assert_eq!(summary.total, 40);
    assert!(summary.cancelled, "汇总应当标记为已取消");
    assert_eq!(summary.succeeded + summary.failed, summary.processed());
    assert!(summary.processed() <= 40);
}

#[test]
fn image_compression_presets_and_parallel_pipeline() {
    let registry = Arc::new(build_default());
    let scratch = Scratch::new("compress_e2e");

    // 1. 生成一张高分辨率平滑渐变图 (1000x800) 并保存为未压缩 BMP
    let large_img = gradient_image(1000, 800);
    let input_bmp = scratch.join("highres.bmp");
    write_image(&registry, &large_img, Format::Bmp, &input_bmp);
    let bmp_bytes = std::fs::metadata(&input_bmp).unwrap().len();

    // 2. 智能均衡压缩测试 (保持原格式，最长边降采样至 2560px，由于 1000<2560 保持 1000x800)
    let balanced_opts = CompressOptions {
        output_dir: scratch.subdir("balanced_out"),
        ..CompressOptions::from_preset(CompressPreset::Balanced)
    };
    let outcome_balanced = compress_file(&input_bmp, &registry, &balanced_opts)
        .expect("智能均衡压缩应当成功");
    assert_eq!(outcome_balanced.target_format, Format::Bmp);
    assert_eq!((outcome_balanced.width, outcome_balanced.height), (1000, 800));

    // 3. 转高效 WebP 极致压缩测试
    let webp_opts = CompressOptions {
        format_strategy: CompressFormatStrategy::Webp,
        downscale: DownscaleLimit::Scale50, // 减半至 500x400
        output_dir: scratch.subdir("webp_out"),
        ..CompressOptions::from_preset(CompressPreset::MaxSaving)
    };
    let outcome_webp = compress_file(&input_bmp, &registry, &webp_opts)
        .expect("转为 WebP 压缩应当成功");
    assert_eq!(outcome_webp.target_format, Format::Webp);
    assert_eq!((outcome_webp.width, outcome_webp.height), (500, 400));
    assert!(
        outcome_webp.output_bytes < bmp_bytes / 10,
        "WebP 降采样压缩后体积应当远小于原始未压缩 BMP (原: {}, 新: {})",
        bmp_bytes,
        outcome_webp.output_bytes
    );

    // 4. 并发批量压缩测试 (TaskRunner::spawn_compress)
    let mut batch_files = Vec::new();
    for i in 0..5 {
        let p = scratch.join(&format!("img_{i}.bmp"));
        write_image(&registry, &gradient_image(64, 48), Format::Bmp, &p);
        batch_files.push(p);
    }

    let compress_batch_opts = CompressOptions {
        format_strategy: CompressFormatStrategy::KeepOriginal,
        output_dir: scratch.subdir("batch_compress_out"),
        ..CompressOptions::from_preset(CompressPreset::FastLossless)
    };

    let runner = TaskRunner::spawn_compress(
        batch_files,
        Arc::clone(&registry),
        compress_batch_opts,
        3,
    );

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut final_summary: Option<TaskSummary> = None;
    while final_summary.is_none() {
        for event in runner.drain() {
            if let TaskEvent::Finished(s) = event {
                final_summary = Some(s);
            }
        }
        if Instant::now() > deadline {
            panic!("并发压缩超时");
        }
        thread::sleep(Duration::from_millis(5));
    }

    let summary = final_summary.unwrap();
    assert_eq!(summary.total, 5);
    assert_eq!(summary.succeeded, 5);
    assert_eq!(summary.failed, 0);
}
