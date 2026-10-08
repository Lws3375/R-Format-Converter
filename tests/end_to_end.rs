//! 端到端集成测试。
//!
//! 单元测试分别验证各个模块,这个文件反过来只通过公开接口把整条链路串起来:
//! 用自研编码器造出输入文件 → 走完整的转换管线落到磁盘 → 再把结果读回来比对像素。
//! 批次测试还会真正启动后台线程池,验证任务事件的顺序与汇总。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use r_format_converter::codecs::build_default;
use r_format_converter::convert::{apply_transforms, convert_file};
use r_format_converter::core::format::Format;
use r_format_converter::core::image::Image;
use r_format_converter::core::options::{ConvertOptions, EncodeOptions, ResizeMode, Rotation};
use r_format_converter::core::pixel::{ColorType, Rgba};
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
        let path = std::env::temp_dir().join(format!(
            "rfc-e2e-{tag}-{}-{unique}",
            std::process::id()
        ));
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
    let mut image = Image::new_zeroed(width, height, ColorType::Rgb8).expect("构造图像失败");
    let span = (width + height).max(1);
    for y in 0..height {
        for x in 0..width {
            let red = (x * 255 / width.max(1)) as u8;
            let green = (y * 255 / height.max(1)) as u8;
            let blue = ((x + y) * 255 / span) as u8;
            image.set_pixel(x, y, Rgba::from_rgb(red, green, blue));
        }
    }
    image
}

/// 用自研编码器把图像写成文件,作为后续转换的输入。
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
    let expected = source.to_rgba();

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

        assert_eq!(decoded.width, 37, "{} 宽度不一致", target.name());
        assert_eq!(decoded.height, 23, "{} 高度不一致", target.name());
        assert_eq!(
            decoded.to_rgba().data,
            expected.data,
            "{} 往返后像素发生变化",
            target.name()
        );
    }
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
        decoded.to_rgba().data,
        source.to_rgba().data,
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
    assert_eq!((transformed.width, transformed.height), (16, 8));
    assert!(transformed.is_gray(), "变换结果应当是单通道灰度图");

    // QOI 没有灰度色彩空间,落盘时按 RGB 保存,所以这里按像素判断灰度。
    let bytes = std::fs::read(&outcome.output).unwrap();
    let decoded = registry.decode_as(&bytes, Format::Qoi).unwrap();
    assert!(
        decoded.to_rgba().data.as_chunks::<4>().0.iter().all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2]),
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
            (decoded.width, decoded.height),
            expected,
            "第 {index} 个输出尺寸不符"
        );
        assert_eq!(
            decoded.to_rgba().data,
            gradient_image(expected.0, expected.1).to_rgba().data,
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
