//! 图像变换执行器。
//!
//! 变换顺序是固定的:旋转 → 镜像 → 缩放 → 灰度 → 颜色模式。固定顺序的好处是
//! 同样的参数无论处理多少次、在哪台机器上处理,得到的结果都完全一致,便于复现
//! 问题,也便于批量任务与单文件任务给出一致的输出。
//!
//! 像素运算全部委托给 `image` 库的 `DynamicImage` 方法,这些方法会保持位图原有的
//! 颜色模式,不会因为旋转或翻转就悄悄把灰度图变成真彩色。

use image::imageops::FilterType;

use crate::core::error::{ConvertError, Result};
use crate::core::image::{Image, color_type, to_color, to_gray};
use crate::core::options::{DownscaleLimit, ResizeMode, Rotation, TransformOptions};

/// 缩放后允许的最大像素数,防止误填超大尺寸把内存吃光。
const MAX_PIXELS: u64 = 1 << 30;

/// 按固定顺序把 [`TransformOptions`] 应用到图像上。
///
/// 未启用任何变换时直接返回原图像,不做无谓的拷贝。
pub fn apply_transforms(image: Image, options: &TransformOptions) -> Result<Image> {
    if options.is_identity() {
        return Ok(image);
    }

    let mut current = rotate(image, options.rotate);

    if options.flip_horizontal {
        current = current.fliph();
    }
    if options.flip_vertical {
        current = current.flipv();
    }

    current = resize(current, options.resize, options.resize_mode)?;

    if options.grayscale {
        current = to_gray(&current);
    }

    if let Some(color) = options.color
        && color != color_type(&current)
    {
        current = to_color(&current, color);
    }

    Ok(current)
}

/// 旋转。90° 与 270° 会交换宽高。
fn rotate(image: Image, rotation: Rotation) -> Image {
    match rotation {
        Rotation::None => image,
        Rotation::Deg90 => image.rotate90(),
        Rotation::Deg180 => image.rotate180(),
        Rotation::Deg270 => image.rotate270(),
    }
}

/// 缩放。
///
/// 宽或高为 0 视为"不缩放",避免生成空图像;尺寸与当前一致时跳过,省掉一次
/// 逐像素重采样。
fn resize(image: Image, target: Option<(u32, u32)>, mode: ResizeMode) -> Result<Image> {
    let Some((width, height)) = target else {
        return Ok(image);
    };
    if width == 0 || height == 0 {
        return Ok(image);
    }
    if width == image.width() && height == image.height() {
        return Ok(image);
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ConvertError::unsupported(format!(
            "缩放目标 {width}×{height} 过大,请缩小尺寸"
        )));
    }

    let filter = match mode {
        ResizeMode::Nearest => FilterType::Nearest,
        ResizeMode::Bilinear => FilterType::Triangle,
    };
    Ok(image.resize_exact(width, height, filter))
}

/// 根据降采样限制对图像执行等比缩放。
///
/// 若计算得出的目标尺寸与原尺寸一致,则原样返回,避免重复重采样。
pub fn downscale_by_limit(image: Image, limit: DownscaleLimit) -> Result<Image> {
    let (target_w, target_h) = limit.calculate_target_size(image.width(), image.height());
    if target_w == image.width() && target_h == image.height() {
        return Ok(image);
    }
    resize(image, Some((target_w, target_h)), ResizeMode::Bilinear)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::color::ColorType;
    use image::Rgba;

    /// 4×2 的图像,左半红右半蓝,便于观察几何变换。
    fn sample() -> Image {
        let mut buffer = image::RgbaImage::new(4, 2);
        for y in 0..2 {
            for x in 0..4 {
                let pixel = if x < 2 {
                    Rgba([255, 0, 0, 255])
                } else {
                    Rgba([0, 0, 255, 255])
                };
                buffer.put_pixel(x, y, pixel);
            }
        }
        Image::ImageRgba8(buffer)
    }

    #[test]
    fn identity_keeps_pixels_untouched() {
        let image = sample();
        let result = apply_transforms(image.clone(), &TransformOptions::default()).unwrap();
        assert_eq!(result.width(), image.width());
        assert_eq!(result.height(), image.height());
        assert_eq!(result.to_rgba8().as_raw(), image.to_rgba8().as_raw());
    }

    #[test]
    fn rotation_swaps_dimensions() {
        let options = TransformOptions {
            rotate: Rotation::Deg90,
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap();
        assert_eq!((result.width(), result.height()), (2, 4));
    }

    #[test]
    fn flip_horizontal_moves_left_half_to_the_right() {
        let options = TransformOptions {
            flip_horizontal: true,
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap().to_rgba8();
        assert_eq!(*result.get_pixel(0, 0), Rgba([0, 0, 255, 255]));
        assert_eq!(*result.get_pixel(3, 0), Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn flip_vertical_keeps_horizontal_order() {
        let options = TransformOptions {
            flip_vertical: true,
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap().to_rgba8();
        assert_eq!(*result.get_pixel(0, 0), Rgba([255, 0, 0, 255]));
        assert_eq!(*result.get_pixel(3, 0), Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn resize_changes_dimensions() {
        let options = TransformOptions {
            resize: Some((8, 4)),
            resize_mode: ResizeMode::Nearest,
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap().to_rgba8();
        assert_eq!((result.width(), result.height()), (8, 4));
        // 最近邻放大后仍只有两种颜色。
        assert_eq!(*result.get_pixel(0, 0), Rgba([255, 0, 0, 255]));
        assert_eq!(*result.get_pixel(7, 0), Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn zero_size_resize_is_ignored() {
        let options = TransformOptions {
            resize: Some((0, 100)),
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap();
        assert_eq!((result.width(), result.height()), (4, 2));
    }

    #[test]
    fn oversize_resize_is_rejected() {
        let options = TransformOptions {
            resize: Some((100_000, 100_000)),
            ..Default::default()
        };
        let err = apply_transforms(sample(), &options).unwrap_err();
        assert!(matches!(err, ConvertError::UnsupportedFeature(_)));
    }

    #[test]
    fn grayscale_and_color_mode_are_applied_last() {
        let options = TransformOptions {
            grayscale: true,
            color: Some(ColorType::Gray8),
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap();
        assert_eq!(color_type(&result), ColorType::Gray8);
        // 纯红转灰度后三通道相等。
        let pixel = result.to_rgb8().get_pixel(0, 0).0;
        assert_eq!(pixel[0], pixel[1]);
        assert_eq!(pixel[1], pixel[2]);
    }

    #[test]
    fn color_mode_only_request_is_not_identity() {
        let options = TransformOptions {
            color: Some(ColorType::Rgb8),
            ..Default::default()
        };
        assert!(!options.is_identity());
        let result = apply_transforms(sample(), &options).unwrap();
        assert_eq!(color_type(&result), ColorType::Rgb8);
    }

    #[test]
    fn combined_transforms_are_reproducible() {
        let options = TransformOptions {
            rotate: Rotation::Deg270,
            flip_vertical: true,
            resize: Some((3, 3)),
            resize_mode: ResizeMode::Bilinear,
            grayscale: true,
            color: Some(ColorType::Gray8),
            ..Default::default()
        };
        let first = apply_transforms(sample(), &options).unwrap();
        let second = apply_transforms(sample(), &options).unwrap();
        assert_eq!(first.to_rgba8().as_raw(), second.to_rgba8().as_raw());
        assert_eq!((first.width(), first.height()), (3, 3));
    }

    #[test]
    fn downscale_by_limit_respects_limits() {
        let img = sample(); // 4x2
        let scaled = downscale_by_limit(img, DownscaleLimit::Scale50).unwrap();
        assert_eq!((scaled.width(), scaled.height()), (2, 1));

        let img = sample();
        let unchanged = downscale_by_limit(img, DownscaleLimit::Original).unwrap();
        assert_eq!((unchanged.width(), unchanged.height()), (4, 2));
    }
}
