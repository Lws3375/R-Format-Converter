//! 各格式的编解码实现。
//!
//! PNG 使用纯 Rust DEFLATE 库处理压缩流,JPEG 使用 Rust 图像库处理复杂的有损编码;
//! 其余格式编解码逻辑由本项目实现。每个模块导出一个 `register` 函数,把编解码器
//! 加入注册表。

pub mod bmp;
pub mod farbfeld;
pub mod ico;
pub mod jpeg;
pub mod netpbm;
pub mod png;
pub mod qoi;
pub mod tga;

use crate::core::registry::Registry;

/// 装配全部已实现格式的编解码器。
///
/// 新增格式时只需在此处追加一行注册代码,界面会自动出现该格式的选项。
pub fn build_default() -> Registry {
    let mut registry = Registry::new();
    bmp::register(&mut registry);
    netpbm::register(&mut registry);
    png::register(&mut registry);
    qoi::register(&mut registry);
    farbfeld::register(&mut registry);
    ico::register(&mut registry);
    jpeg::register(&mut registry);
    // TGA 没有魔数,只能依据文件头取值做启发式判断,因此放在最后以免误判其他格式。
    tga::register(&mut registry);
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::format::Format;
    use crate::core::image::Image;
    use crate::core::options::EncodeOptions;
    use crate::core::pixel::{ColorType, Rgba};

    /// 构造一张内容确定的测试图,覆盖多种颜色与半透明像素。
    pub(crate) fn sample_image() -> Image {
        let mut image = Image::new_zeroed(4, 3, ColorType::Rgba8).unwrap();
        let palette = [
            Rgba::from_rgb(255, 0, 0),
            Rgba::from_rgb(0, 255, 0),
            Rgba::from_rgb(0, 0, 255),
            Rgba::from_rgb(255, 255, 255),
            Rgba::from_rgb(128, 64, 32),
            Rgba::new(10, 20, 30, 128),
        ];
        for y in 0..image.height {
            for x in 0..image.width {
                let index = (y * image.width + x) as usize % palette.len();
                image.set_pixel(x, y, palette[index]);
            }
        }
        image
    }

    #[test]
    fn default_registry_covers_all_implemented_formats() {
        let registry = build_default();
        for format in Format::all() {
            assert_eq!(
                registry.is_supported(*format),
                format.is_implemented(),
                "{} 的注册状态与实现状态不一致",
                format.id()
            );
        }
    }

    #[test]
    fn every_format_round_trips_pixel_exact() {
        let registry = build_default();
        let source = sample_image();
        for format in Format::all()
            .iter()
            .filter(|f| f.is_implemented() && f.is_lossless())
        {
            let encoded = registry
                .encode(&source, *format, &EncodeOptions::default())
                .unwrap_or_else(|err| panic!("{} 编码失败:{err}", format.id()));
            let decoded = registry
                .decode(&encoded)
                .unwrap_or_else(|err| panic!("{} 解码失败:{err}", format.id()));
            assert_eq!(
                (decoded.width, decoded.height),
                (source.width, source.height),
                "{} 尺寸不一致",
                format.id()
            );
            let original = source.to_rgba();
            let restored = decoded.to_rgba();
            assert_eq!(
                original.data,
                restored.data,
                "{} 往返后像素不一致",
                format.id()
            );
        }
    }
}
