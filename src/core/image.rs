//! 内存位图。
//!
//! 位图直接采用 `image` 库的 [`DynamicImage`](image::DynamicImage),不再自建像素
//! 缓冲区。本模块只提供几个薄封装:位深度归一化、颜色模式读取与改写、透明度检测,
//! 供转换管线与界面预览使用。

use crate::core::color::ColorType;

/// 软件内部统一使用的位图类型。
pub type Image = image::DynamicImage;

/// 该位图是否已经是 8 位深度。
///
/// 软件的颜色模式、PNG 压缩等级等选项都按 8 位设计,因此解码出口会统一降位,
/// 之后的流程只需处理四种 8 位变体。
pub fn is_eight_bit(image: &Image) -> bool {
    matches!(
        image,
        Image::ImageLuma8(_)
            | Image::ImageLumaA8(_)
            | Image::ImageRgb8(_)
            | Image::ImageRgba8(_)
    )
}

/// 把任意位深度的图像统一到 8 位,通道布局保持不变。
///
/// `image` 库能解码 16 位与浮点图像,这里按原有通道布局降位:灰度仍是灰度,
/// 真彩色仍是真彩色,不会因为降位而凭空多出透明度通道。
pub fn normalize(image: Image) -> Image {
    if is_eight_bit(&image) {
        return image;
    }
    match &image {
        Image::ImageLuma16(_) => Image::ImageLuma8(image.to_luma8()),
        Image::ImageLumaA16(_) => Image::ImageLumaA8(image.to_luma_alpha8()),
        Image::ImageRgb16(_) | Image::ImageRgb32F(_) => Image::ImageRgb8(image.to_rgb8()),
        _ => Image::ImageRgba8(image.to_rgba8()),
    }
}

/// 读取位图当前的颜色模式。
pub fn color_type(image: &Image) -> ColorType {
    match image {
        Image::ImageLuma8(_) | Image::ImageLuma16(_) => ColorType::Gray8,
        Image::ImageLumaA8(_) | Image::ImageLumaA16(_) => ColorType::GrayAlpha8,
        Image::ImageRgb8(_) | Image::ImageRgb16(_) | Image::ImageRgb32F(_) => ColorType::Rgb8,
        // `DynamicImage` 标了 non_exhaustive,未知变体按四通道兜底,与 `normalize` 一致。
        _ => ColorType::Rgba8,
    }
}

/// 按目标颜色模式重建位图。
pub fn to_color(image: &Image, color: ColorType) -> Image {
    match color {
        ColorType::Gray8 => Image::ImageLuma8(image.to_luma8()),
        ColorType::GrayAlpha8 => Image::ImageLumaA8(image.to_luma_alpha8()),
        ColorType::Rgb8 => Image::ImageRgb8(image.to_rgb8()),
        ColorType::Rgba8 => Image::ImageRgba8(image.to_rgba8()),
    }
}

/// 转成灰度,原有透明度通道保持不变。
pub fn to_gray(image: &Image) -> Image {
    match color_type(image) {
        ColorType::GrayAlpha8 | ColorType::Rgba8 => Image::ImageLumaA8(image.to_luma_alpha8()),
        _ => Image::ImageLuma8(image.to_luma8()),
    }
}

/// 图像中是否存在"既非全透明也非全不透明"的像素。
///
/// 只有带透明度通道的位图才可能返回 `true`,这与旧实现保持一致。
pub fn has_semi_transparent_pixel(image: &Image) -> bool {
    match image {
        Image::ImageLumaA8(buffer) => has_partial_alpha(buffer.as_raw(), 2),
        Image::ImageRgba8(buffer) => has_partial_alpha(buffer.as_raw(), 4),
        // 非 8 位位图在解码出口已被归一化,这里只是兜住极端情况。
        Image::ImageLumaA16(_) | Image::ImageRgba16(_) | Image::ImageRgba32F(_) => {
            has_partial_alpha(image.to_rgba8().as_raw(), 4)
        }
        _ => false,
    }
}

/// 在按 `channels` 分组的像素流里查找部分透明的 alpha 值。
fn has_partial_alpha(raw: &[u8], channels: usize) -> bool {
    let alpha = channels - 1;
    raw.chunks_exact(channels)
        .any(|pixel| pixel[alpha] != 0 && pixel[alpha] != 255)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayAlphaImage, GrayImage, RgbImage, RgbaImage};

    // `image` 0.25 只给 8 位缓冲区留了类型别名,16 位要自己写全。
    type Gray16Image = image::ImageBuffer<image::Luma<u16>, Vec<u16>>;
    type GrayAlpha16Image = image::ImageBuffer<image::LumaA<u16>, Vec<u16>>;
    type Rgb16Image = image::ImageBuffer<image::Rgb<u16>, Vec<u16>>;

    #[test]
    fn eight_bit_images_are_left_alone() {
        let image = Image::ImageRgb8(RgbImage::new(2, 3));
        assert!(is_eight_bit(&image));
        assert_eq!(color_type(&normalize(image)), ColorType::Rgb8);
    }

    #[test]
    fn sixteen_bit_images_are_demoted_keeping_layout() {
        let gray = Image::ImageLuma16(Gray16Image::new(2, 2));
        assert!(!is_eight_bit(&gray));
        assert!(is_eight_bit(&normalize(gray)));

        let gray_alpha = Image::ImageLumaA16(GrayAlpha16Image::new(2, 2));
        assert_eq!(color_type(&normalize(gray_alpha)), ColorType::GrayAlpha8);

        let rgb = Image::ImageRgb16(Rgb16Image::new(2, 2));
        assert_eq!(color_type(&normalize(rgb)), ColorType::Rgb8);
    }

    #[test]
    fn color_type_reads_every_eight_bit_layout() {
        assert_eq!(
            color_type(&Image::ImageLuma8(GrayImage::new(1, 1))),
            ColorType::Gray8
        );
        assert_eq!(
            color_type(&Image::ImageLumaA8(GrayAlphaImage::new(1, 1))),
            ColorType::GrayAlpha8
        );
        assert_eq!(
            color_type(&Image::ImageRgb8(RgbImage::new(1, 1))),
            ColorType::Rgb8
        );
        assert_eq!(
            color_type(&Image::ImageRgba8(RgbaImage::new(1, 1))),
            ColorType::Rgba8
        );
    }

    #[test]
    fn gray_conversion_keeps_alpha_channel() {
        let rgba = Image::ImageRgba8(RgbaImage::new(2, 2));
        assert_eq!(color_type(&to_gray(&rgba)), ColorType::GrayAlpha8);

        let rgb = Image::ImageRgb8(RgbImage::new(2, 2));
        assert_eq!(color_type(&to_gray(&rgb)), ColorType::Gray8);
    }

    #[test]
    fn only_alpha_layouts_can_report_partial_transparency() {
        let opaque = Image::ImageRgba8(RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255])));
        assert!(!has_semi_transparent_pixel(&opaque));

        let clear = Image::ImageRgba8(RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 0])));
        assert!(!has_semi_transparent_pixel(&clear));

        let half = Image::ImageRgba8(RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 128])));
        assert!(has_semi_transparent_pixel(&half));

        let no_alpha = Image::ImageRgb8(RgbImage::new(2, 2));
        assert!(!has_semi_transparent_pixel(&no_alpha));
    }
}
