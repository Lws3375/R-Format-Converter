//! 图像变换执行器。
//!
//! 变换顺序是固定的:旋转 → 镜像 → 缩放 → 灰度 → 颜色模式。固定顺序的好处是
//! 同样的参数无论处理多少次、在哪台机器上处理,得到的结果都完全一致,便于复现
//! 问题,也便于批量任务与单文件任务给出一致的输出。

use crate::core::error::Result;
use crate::core::image::Image;
use crate::core::options::{ResizeMode, Rotation, TransformOptions};

/// 按固定顺序把 [`TransformOptions`] 应用到图像上。
///
/// 未启用任何变换时直接返回原图像,不做无谓的拷贝。
pub fn apply_transforms(image: Image, options: &TransformOptions) -> Result<Image> {
    if options.is_identity() {
        return Ok(image);
    }

    let mut current = image;

    current = match options.rotate {
        Rotation::None => current,
        Rotation::Deg90 => current.rotate_90(),
        Rotation::Deg180 => current.rotate_180(),
        Rotation::Deg270 => current.rotate_270(),
    };

    if options.flip_horizontal {
        current = current.flip_horizontal();
    }
    if options.flip_vertical {
        current = current.flip_vertical();
    }

    if let Some((width, height)) = options.resize {
        // 宽或高为 0 视为"不缩放",避免生成空图像;
        // 尺寸与当前一致时跳过,省掉一次逐像素重采样。
        if width > 0 && height > 0 && (width != current.width || height != current.height) {
            current = match options.resize_mode {
                ResizeMode::Nearest => current.resize_nearest(width, height)?,
                ResizeMode::Bilinear => current.resize_bilinear(width, height)?,
            };
        }
    }

    if options.grayscale {
        current = current.to_gray();
    }

    if let Some(color) = options.color
        && color != current.color
    {
        current = current.convert_to(color);
    }

    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pixel::{ColorType, Rgba};

    /// 4×2 的图像,左半红右半蓝,便于观察几何变换。
    fn sample() -> Image {
        let mut image = Image::new_zeroed(4, 2, ColorType::Rgba8).unwrap();
        for y in 0..2 {
            for x in 0..4 {
                let pixel = if x < 2 {
                    Rgba::new(255, 0, 0, 255)
                } else {
                    Rgba::new(0, 0, 255, 255)
                };
                image.set_pixel(x, y, pixel);
            }
        }
        image
    }

    #[test]
    fn identity_keeps_pixels_untouched() {
        let image = sample();
        let result = apply_transforms(image.clone(), &TransformOptions::default()).unwrap();
        assert_eq!(result.width, image.width);
        assert_eq!(result.height, image.height);
        assert_eq!(result.data, image.data);
    }

    #[test]
    fn rotation_swaps_dimensions() {
        let options = TransformOptions {
            rotate: Rotation::Deg90,
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap();
        assert_eq!((result.width, result.height), (2, 4));
    }

    #[test]
    fn flip_horizontal_moves_left_half_to_the_right() {
        let options = TransformOptions {
            flip_horizontal: true,
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap();
        assert_eq!(result.get_pixel(0, 0), Rgba::new(0, 0, 255, 255));
        assert_eq!(result.get_pixel(3, 0), Rgba::new(255, 0, 0, 255));
    }

    #[test]
    fn resize_changes_dimensions() {
        let options = TransformOptions {
            resize: Some((8, 4)),
            resize_mode: ResizeMode::Nearest,
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap();
        assert_eq!((result.width, result.height), (8, 4));
        // 最近邻放大后仍只有两种颜色。
        assert_eq!(result.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(result.get_pixel(7, 0), Rgba::new(0, 0, 255, 255));
    }

    #[test]
    fn zero_size_resize_is_ignored() {
        let options = TransformOptions {
            resize: Some((0, 100)),
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap();
        assert_eq!((result.width, result.height), (4, 2));
    }

    #[test]
    fn grayscale_and_color_mode_are_applied_last() {
        let options = TransformOptions {
            grayscale: true,
            color: Some(ColorType::Gray8),
            ..Default::default()
        };
        let result = apply_transforms(sample(), &options).unwrap();
        assert_eq!(result.color, ColorType::Gray8);
        // 纯红转灰度后三通道相等。
        let pixel = result.get_pixel(0, 0);
        assert_eq!(pixel.r, pixel.g);
        assert_eq!(pixel.g, pixel.b);
    }

    #[test]
    fn color_mode_only_request_is_not_identity() {
        let options = TransformOptions {
            color: Some(ColorType::Rgb8),
            ..Default::default()
        };
        assert!(!options.is_identity());
        let result = apply_transforms(sample(), &options).unwrap();
        assert_eq!(result.color, ColorType::Rgb8);
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
        assert_eq!(first.data, second.data);
        assert_eq!((first.width, first.height), (3, 3));
    }
}
