//! 内存位图结构。
//!
//! [`Image`] 采用行优先、逐像素紧密排列的字节缓冲区,不保留任何行填充字节。
//! 各编解码器在读取文件时负责去掉格式自身的行对齐填充,写出时再按需要补回。

use crate::core::error::{ConvertError, Result};
use crate::core::pixel::{ColorType, Rgba};

/// 一张内存中的位图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// 宽度(像素)。
    pub width: u32,
    /// 高度(像素)。
    pub height: u32,
    /// 存储布局。
    pub color: ColorType,
    /// 像素数据,长度为 `width * height * color.channels()`。
    pub data: Vec<u8>,
}

impl Image {
    /// 由原始字节构造位图,并校验长度。
    pub fn new(width: u32, height: u32, color: ColorType, data: Vec<u8>) -> Result<Self> {
        let expected = pixel_count(width, height)? * color.channels();
        if data.len() != expected {
            return Err(ConvertError::corrupt(format!(
                "位图数据长度不符:期望 {expected} 字节,实际 {} 字节",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            color,
            data,
        })
    }

    /// 创建全零位图(黑色或全透明,取决于颜色类型)。
    pub fn new_zeroed(width: u32, height: u32, color: ColorType) -> Result<Self> {
        let len = pixel_count(width, height)? * color.channels();
        Self::new(width, height, color, vec![0u8; len])
    }

    /// 创建填充为指定颜色的位图。
    pub fn filled(width: u32, height: u32, color: ColorType, pixel: Rgba) -> Result<Self> {
        let mut image = Self::new_zeroed(width, height, color)?;
        for y in 0..height {
            for x in 0..width {
                image.set_pixel(x, y, pixel);
            }
        }
        Ok(image)
    }

    /// 由 RGBA 字节流构造位图。
    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::new(width, height, ColorType::Rgba8, data)
    }

    /// 每行字节数。
    pub fn row_bytes(&self) -> usize {
        self.width as usize * self.color.channels()
    }

    /// 像素总数。
    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// 数据总字节数。
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }

    /// 是否为灰度布局。
    pub fn is_gray(&self) -> bool {
        self.color.is_gray()
    }

    /// 指定像素在 `data` 中的起始下标。
    fn index(&self, x: u32, y: u32) -> usize {
        let channels = self.color.channels();
        (y as usize * self.width as usize + x as usize) * channels
    }

    /// 读取像素。越界时返回透明像素,便于缩放等算法处理边界。
    pub fn get_pixel(&self, x: u32, y: u32) -> Rgba {
        if x >= self.width || y >= self.height {
            return Rgba::TRANSPARENT;
        }
        let offset = self.index(x, y);
        let bytes = &self.data[offset..offset + self.color.channels()];
        match self.color {
            ColorType::Gray8 => Rgba::from_gray(bytes[0]),
            ColorType::GrayAlpha8 => Rgba {
                r: bytes[0],
                g: bytes[0],
                b: bytes[0],
                a: bytes[1],
            },
            ColorType::Rgb8 => Rgba::from_rgb(bytes[0], bytes[1], bytes[2]),
            ColorType::Rgba8 => Rgba::from_array([bytes[0], bytes[1], bytes[2], bytes[3]]),
        }
    }

    /// 写入像素。越界时静默忽略。
    pub fn set_pixel(&mut self, x: u32, y: u32, pixel: Rgba) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = self.index(x, y);
        let channels = self.color.channels();
        let slot = &mut self.data[offset..offset + channels];
        match self.color {
            ColorType::Gray8 => slot[0] = pixel.luma(),
            ColorType::GrayAlpha8 => {
                slot[0] = pixel.luma();
                slot[1] = pixel.a;
            }
            ColorType::Rgb8 => {
                slot[0] = pixel.r;
                slot[1] = pixel.g;
                slot[2] = pixel.b;
            }
            ColorType::Rgba8 => {
                slot[0] = pixel.r;
                slot[1] = pixel.g;
                slot[2] = pixel.b;
                slot[3] = pixel.a;
            }
        }
    }

    /// 转换为 RGBA 布局(若已是 RGBA 则克隆一份)。
    pub fn to_rgba(&self) -> Image {
        if self.color == ColorType::Rgba8 {
            return self.clone();
        }
        let mut out = Image {
            width: self.width,
            height: self.height,
            color: ColorType::Rgba8,
            data: vec![0u8; self.pixel_count() * 4],
        };
        for y in 0..self.height {
            for x in 0..self.width {
                let pixel = self.get_pixel(x, y);
                out.set_pixel(x, y, pixel);
            }
        }
        out
    }

    /// 转换到指定的颜色类型。
    pub fn convert_to(&self, color: ColorType) -> Image {
        if self.color == color {
            return self.clone();
        }
        let mut out = Image {
            width: self.width,
            height: self.height,
            color,
            data: vec![0u8; self.pixel_count() * color.channels()],
        };
        for y in 0..self.height {
            for x in 0..self.width {
                let pixel = self.get_pixel(x, y);
                out.set_pixel(x, y, pixel);
            }
        }
        out
    }

    /// 逐像素转为灰度。
    pub fn to_gray(&self) -> Image {
        let target = if self.color.has_alpha() {
            ColorType::GrayAlpha8
        } else {
            ColorType::Gray8
        };
        let mut out = Image {
            width: self.width,
            height: self.height,
            color: target,
            data: vec![0u8; self.pixel_count() * target.channels()],
        };
        for y in 0..self.height {
            for x in 0..self.width {
                out.set_pixel(x, y, self.get_pixel(x, y).to_gray());
            }
        }
        out
    }

    /// 水平镜像。
    pub fn flip_horizontal(&self) -> Image {
        let mut out = self.clone();
        for y in 0..self.height {
            for x in 0..self.width / 2 {
                let mirror = self.width - 1 - x;
                let left = self.get_pixel(x, y);
                let right = self.get_pixel(mirror, y);
                out.set_pixel(x, y, right);
                out.set_pixel(mirror, y, left);
            }
        }
        out
    }

    /// 垂直镜像。
    pub fn flip_vertical(&self) -> Image {
        let mut out = self.clone();
        for y in 0..self.height / 2 {
            let mirror = self.height - 1 - y;
            for x in 0..self.width {
                let top = self.get_pixel(x, y);
                let bottom = self.get_pixel(x, mirror);
                out.set_pixel(x, y, bottom);
                out.set_pixel(x, mirror, top);
            }
        }
        out
    }

    /// 顺时针旋转 90 度。
    pub fn rotate_90(&self) -> Image {
        let mut out = Self::new_zeroed(self.height, self.width, self.color)
            .expect("旋转后尺寸与原图一致");
        for ny in 0..out.height {
            for nx in 0..out.width {
                out.set_pixel(nx, ny, self.get_pixel(ny, self.height - 1 - nx));
            }
        }
        out
    }

    /// 顺时针旋转 180 度。
    pub fn rotate_180(&self) -> Image {
        let mut out = Self::new_zeroed(self.width, self.height, self.color)
            .expect("旋转后尺寸与原图一致");
        for y in 0..self.height {
            for x in 0..self.width {
                out.set_pixel(
                    x,
                    y,
                    self.get_pixel(self.width - 1 - x, self.height - 1 - y),
                );
            }
        }
        out
    }

    /// 顺时针旋转 270 度。
    pub fn rotate_270(&self) -> Image {
        let mut out = Self::new_zeroed(self.height, self.width, self.color)
            .expect("旋转后尺寸与原图一致");
        for ny in 0..out.height {
            for nx in 0..out.width {
                out.set_pixel(nx, ny, self.get_pixel(self.width - 1 - ny, nx));
            }
        }
        out
    }

    /// 最近邻缩放,速度最快,适合像素风格图像。
    pub fn resize_nearest(&self, width: u32, height: u32) -> Result<Image> {
        if width == 0 || height == 0 {
            return Err(ConvertError::invalid_path("缩放后的宽高必须大于 0"));
        }
        let mut out = Self::new_zeroed(width, height, self.color)?;
        for y in 0..height {
            let sy = (y as u64 * self.height as u64 / height as u64) as u32;
            for x in 0..width {
                let sx = (x as u64 * self.width as u64 / width as u64) as u32;
                out.set_pixel(x, y, self.get_pixel(sx.min(self.width - 1), sy.min(self.height - 1)));
            }
        }
        Ok(out)
    }

    /// 双线性缩放。采用预乘 Alpha 插值,避免透明区域边缘出现暗边。
    pub fn resize_bilinear(&self, width: u32, height: u32) -> Result<Image> {
        if width == 0 || height == 0 {
            return Err(ConvertError::invalid_path("缩放后的宽高必须大于 0"));
        }
        let mut out = Self::new_zeroed(width, height, self.color)?;

        // 采样点映射到原图的连续坐标,使用半像素偏移使结果居中。
        let scale_x = self.width as f64 / width as f64;
        let scale_y = self.height as f64 / height as f64;

        for y in 0..height {
            let fy = (y as f64 + 0.5) * scale_y - 0.5;
            let y0 = fy.floor().max(0.0) as u32;
            let y1 = (y0 + 1).min(self.height - 1);
            let wy = (fy - fy.floor()).clamp(0.0, 1.0);

            for x in 0..width {
                let fx = (x as f64 + 0.5) * scale_x - 0.5;
                let x0 = fx.floor().max(0.0) as u32;
                let x1 = (x0 + 1).min(self.width - 1);
                let wx = (fx - fx.floor()).clamp(0.0, 1.0);

                let p00 = self.get_pixel(x0.min(self.width - 1), y0);
                let p10 = self.get_pixel(x1, y0);
                let p01 = self.get_pixel(x0.min(self.width - 1), y1);
                let p11 = self.get_pixel(x1, y1);

                let top = interpolate(p00, p10, wx);
                let bottom = interpolate(p01, p11, wx);
                out.set_pixel(x, y, interpolate(top, bottom, wy));
            }
        }

        Ok(out)
    }
}

/// 计算像素总数,并防止宽高乘积溢出。
fn pixel_count(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(ConvertError::corrupt("图像宽高不能为 0"));
    }
    (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| ConvertError::corrupt("图像尺寸过大"))
}

/// 按预乘 Alpha 的方式在两个像素间插值。
fn interpolate(a: Rgba, b: Rgba, t: f64) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 {
        let value = x as f64 * (1.0 - t) + y as f64 * t;
        value.round().clamp(0.0, 255.0) as u8
    };

    // 先按透明度加权,再还原为直通 Alpha,消除透明边缘的黑色渗色。
    let weight_a = a.a as f64;
    let weight_b = b.a as f64;
    let total = weight_a * (1.0 - t) + weight_b * t;
    if total < 0.5 {
        return Rgba::TRANSPARENT;
    }

    let channel = |x: u8, y: u8| -> u8 {
        let value = (x as f64 * weight_a * (1.0 - t) + y as f64 * weight_b * t) / total;
        value.round().clamp(0.0, 255.0) as u8
    };

    Rgba {
        r: channel(a.r, b.r),
        g: channel(a.g, b.g),
        b: channel(a.b, b.b),
        a: mix(a.a, b.a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 2x2 真彩图,四个像素颜色各不相同。
    fn sample() -> Image {
        let data = vec![
            255, 0, 0, // (0,0) 红
            0, 255, 0, // (1,0) 绿
            0, 0, 255, // (0,1) 蓝
            255, 255, 255, // (1,1) 白
        ];
        Image::new(2, 2, ColorType::Rgb8, data).unwrap()
    }

    #[test]
    fn length_mismatch_is_rejected() {
        let err = Image::new(2, 2, ColorType::Rgb8, vec![0u8; 11]).unwrap_err();
        assert!(matches!(err, ConvertError::Corrupt(_)));
    }

    #[test]
    fn pixel_roundtrip() {
        let image = sample();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(255, 0, 0));
        assert_eq!(image.get_pixel(1, 1), Rgba::from_rgb(255, 255, 255));

        let mut copy = image.clone();
        copy.set_pixel(0, 0, Rgba::from_rgb(1, 2, 3));
        assert_eq!(copy.get_pixel(0, 0), Rgba::from_rgb(1, 2, 3));
        // 原图不受影响
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(255, 0, 0));
    }

    #[test]
    fn gray_layout_keeps_alpha() {
        let image = Image::new(
            2,
            1,
            ColorType::GrayAlpha8,
            vec![10, 255, 200, 128],
        )
        .unwrap();
        assert_eq!(image.get_pixel(0, 0), Rgba::new(10, 10, 10, 255));
        assert_eq!(image.get_pixel(1, 0), Rgba::new(200, 200, 200, 128));
    }

    #[test]
    fn flip_horizontal_mirrors_columns() {
        let image = sample().flip_horizontal();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(0, 255, 0));
        assert_eq!(image.get_pixel(1, 0), Rgba::from_rgb(255, 0, 0));
    }

    #[test]
    fn flip_vertical_mirrors_rows() {
        let image = sample().flip_vertical();
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(0, 0, 255));
        assert_eq!(image.get_pixel(0, 1), Rgba::from_rgb(255, 0, 0));
    }

    #[test]
    fn rotate_90_swaps_dimensions() {
        let image = sample().rotate_90();
        assert_eq!((image.width, image.height), (2, 2));
        // 原 (0,0) 红 顺时针转到 (1,0)
        assert_eq!(image.get_pixel(1, 0), Rgba::from_rgb(255, 0, 0));
        // 原 (1,0) 绿 转到 (1,1)
        assert_eq!(image.get_pixel(1, 1), Rgba::from_rgb(0, 255, 0));
    }

    #[test]
    fn rotate_four_times_is_identity() {
        let image = sample();
        let rotated = image.rotate_90().rotate_90().rotate_90().rotate_90();
        assert_eq!(rotated, image);
    }

    #[test]
    fn rotate_180_and_270_are_consistent() {
        let image = sample();
        assert_eq!(image.rotate_180(), image.rotate_90().rotate_90());
        assert_eq!(image.rotate_270(), image.rotate_90().rotate_180());
    }

    #[test]
    fn nearest_resize_keeps_colors() {
        let image = sample().resize_nearest(4, 4).unwrap();
        assert_eq!((image.width, image.height), (4, 4));
        assert_eq!(image.get_pixel(0, 0), Rgba::from_rgb(255, 0, 0));
        assert_eq!(image.get_pixel(3, 3), Rgba::from_rgb(255, 255, 255));
    }

    #[test]
    fn bilinear_resize_preserves_solid_color() {
        let solid = Image::filled(3, 3, ColorType::Rgba8, Rgba::from_rgb(40, 80, 120)).unwrap();
        let scaled = solid.resize_bilinear(7, 5).unwrap();
        assert_eq!((scaled.width, scaled.height), (7, 5));
        assert_eq!(scaled.get_pixel(3, 2), Rgba::from_rgb(40, 80, 120));
    }

    #[test]
    fn convert_to_gray_drops_channels() {
        let image = sample().to_gray();
        assert_eq!(image.color, ColorType::Gray8);
        assert_eq!(image.data.len(), 4);
        // 红色在 Rec.601 下约 76
        assert_eq!(image.data[0], Rgba::from_rgb(255, 0, 0).luma());
    }
}
