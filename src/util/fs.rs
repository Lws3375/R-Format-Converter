//! 文件与路径工具。
//!


use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::error::{ConvertError, Result};
use crate::core::options::NamingRule;

/// 应用数据目录名(配置与历史记录存放处)。
pub const APP_FOLDER: &str = "RFormatConverter";

/// 返回应用数据目录。
///
/// Windows 下使用 `%APPDATA%\RFormatConverter`,其它平台回退到用户主目录下的
/// 隐藏目录;两者都不可用时退回当前工作目录。
pub fn app_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(dir) = non_empty_env("APPDATA") {
            return PathBuf::from(dir).join(APP_FOLDER);
        }
    }

    if let Some(home) = non_empty_env("HOME") {
        return PathBuf::from(home).join(format!(".{APP_FOLDER}"));
    }

    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// 读取环境变量,空字符串视为未设置。
fn non_empty_env(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

/// 确保目录存在,不存在则逐级创建。
pub fn ensure_dir(dir: &Path) -> Result<()> {
    if dir.as_os_str().is_empty() {
        return Err(ConvertError::InvalidPath("目录路径为空".to_string()));
    }
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// 读取整个文件。
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    if !path.is_file() {
        return Err(ConvertError::InvalidPath(format!(
            "文件不存在:{}",
            path.display()
        )));
    }
    Ok(std::fs::read(path)?)
}

/// 写入整个文件,自动创建上级目录。
pub fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        ensure_dir(parent)?;
    }
    std::fs::write(path, data)?;
    Ok(())
}

/// 取小写扩展名(不含点),没有扩展名时返回空串。
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// 取文件名(不含扩展名)。
pub fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_string())
        .unwrap_or_else(|| "未命名".to_string())
}

/// 取带扩展名的文件名。
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// 把字节数格式化为便于阅读的字符串。
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.2} {}", value, UNITS[unit])
    }
}

/// 根据命名规则与输出目录推导目标文件路径。
///
/// `overwrite` 为 `false` 时,若目标已存在则自动追加序号,避免误覆盖用户文件。
pub fn build_output_path(
    input: &Path,
    output_dir: &Path,
    target_ext: &str,
    naming: NamingRule,
    overwrite: bool,
) -> PathBuf {
    let stem = file_stem(input);
    let file_name = match naming {
        NamingRule::KeepOriginal => format!("{stem}.{target_ext}"),
        NamingRule::AppendSuffix => format!("{stem}_converted.{target_ext}"),
        NamingRule::Timestamp => {
            let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            format!("{stem}_{stamp}.{target_ext}")
        }
    };

    let candidate = output_dir.join(file_name);
    if overwrite {
        candidate
    } else {
        unique_path(&candidate)
    }
}

/// 若路径已存在,则在扩展名前追加 `(1)`、`(2)` 等序号直到不冲突。
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = file_stem(path);
    let ext = extension_of(path);

    for index in 1..=9999 {
        let name = if ext.is_empty() {
            format!("{stem}({index})")
        } else {
            format!("{stem}({index}).{ext}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }

    path.to_path_buf()
}

/// 创建本次测试专用的临时目录。
///
/// 目录名里带上进程号与纳秒时间戳,避免并行执行的测试互相干扰。
#[cfg(test)]
pub(crate) fn temp_dir(tag: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("r-conv-{tag}-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

/// 在系统文件管理器中定位并选中文件。
pub fn reveal_in_explorer(path: &Path) -> Result<()> {
    let target = path.to_path_buf();
    if !target.exists() {
        return Err(ConvertError::InvalidPath(format!(
            "文件不存在:{}",
            target.display()
        )));
    }

    #[cfg(windows)]
    {
        // explorer 在成功时也可能返回非 0 退出码,因此忽略退出状态。
        Command::new("explorer")
            .arg(format!("/select,{}", target.display()))
            .spawn()
            .map_err(|err| ConvertError::InvalidPath(format!("无法启动资源管理器:{err}")))?;
    }

    #[cfg(not(windows))]
    {
        let dir = target.parent().unwrap_or_else(|| Path::new("."));
        open_directory(dir)?;
    }

    Ok(())
}

/// 用系统默认方式打开目录。
pub fn open_directory(dir: &Path) -> Result<()> {
    #[cfg(windows)]
    let mut command = Command::new("explorer");

    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");

    #[cfg(all(not(windows), not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");

    command
        .arg(dir)
        .spawn()
        .map_err(|err| ConvertError::InvalidPath(format!("无法打开目录:{err}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_scales_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.00 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.00 MB");
    }

    #[test]
    fn output_path_follows_naming_rule() {
        let input = Path::new("C:/images/photo.jpg");
        let dir = Path::new("C:/out");

        let keep = build_output_path(input, dir, "bmp", NamingRule::KeepOriginal, true);
        assert_eq!(keep, Path::new("C:/out/photo.bmp"));

        let suffix = build_output_path(input, dir, "bmp", NamingRule::AppendSuffix, true);
        assert_eq!(suffix, Path::new("C:/out/photo_converted.bmp"));

        let stamped = build_output_path(input, dir, "bmp", NamingRule::Timestamp, true);
        assert!(file_stem(&stamped).starts_with("photo_"));
    }

    #[test]
    fn extension_is_lowercased() {
        assert_eq!(extension_of(Path::new("a/b/PIC.JPG")), "jpg");
        assert_eq!(extension_of(Path::new("a/b/noext")), "");
    }
}
