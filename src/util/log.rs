//! 日志初始化。
//!
//! 日志同时写往标准错误流与应用数据目录下的 `log/app.log`。默认级别为 `info`,
//! 也就是每次启动和每次转换各留一行;传入 `verbose` 或在环境变量 `RUST_LOG` 中
//! 显式指定级别时输出更详细的日志。发布版没有控制台,排查问题只能依赖这个日志文件。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

/// 应用数据目录下的日志子目录与文件名。
const LOG_DIR: &str = "log";
const LOG_FILE: &str = "app.log";

/// 单个日志文件的体积上限,超过后轮转为 `app.log.1`。
const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// 日志文件的完整路径,例如 `%APPDATA%\RFormatConverter\log\app.log`。
///
/// 应用数据目录不可用时返回 `None`,此时日志只写标准错误流。
pub fn log_file_path() -> Option<PathBuf> {
    let dir = crate::util::fs::app_data_dir();
    if dir.as_os_str().is_empty() {
        return None;
    }
    Some(dir.join(LOG_DIR).join(LOG_FILE))
}

/// 打开日志文件;超过体积上限时先轮转出一份 `app.log.1`。
fn open_log_file() -> Option<std::fs::File> {
    let path = log_file_path()?;
    crate::util::fs::ensure_dir(path.parent()?).ok()?;

    let too_big = std::fs::metadata(&path)
        .map(|meta| meta.len() > MAX_LOG_BYTES)
        .unwrap_or(false);
    if too_big {
        let rotated = path.with_extension("log.1");
        let _ = std::fs::remove_file(&rotated);
        let _ = std::fs::rename(&path, &rotated);
    }

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
}

/// 同时写标准错误流与日志文件的输出目标。
///
/// 发布版没有控制台,写标准错误流必然失败,这里刻意忽略,只要文件写成功即可。
struct Tee {
    file: std::sync::Mutex<std::fs::File>,
}

impl Write for Tee {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buffer);
        self.file_handle().write_all(buffer)?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        self.file_handle().flush()
    }
}

impl Tee {
    /// 取出文件句柄;锁被毒化时继续写日志,不因为一次 panic 就丢掉日志能力。
    fn file_handle(&self) -> std::sync::MutexGuard<'_, std::fs::File> {
        match self.file.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// 记录一次启动信息,便于在日志里区分不同的运行次数。
pub fn note_startup(file_count: usize) {
    log::info!(
        "{} {} 启动,命令行传入文件 {} 个",
        crate::core::APP_FULL_NAME,
        crate::core::APP_VERSION,
        file_count
    );
}

/// 初始化全局日志器。
///
/// 重复调用是安全的,已初始化时会直接返回。
pub fn init(verbose: bool) {
    let default_level = if verbose { "debug" } else { "info" };

    let mut builder = env_logger::Builder::new();
    builder
        .filter_level(parse_level(default_level))
        .format(|buffer, record| {
            writeln!(
                buffer,
                "[{}] [{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.args()
            )
        });

    // 日志同时写入文件:发布版没有控制台,出问题时只能靠文件回溯。
    if let Some(file) = open_log_file() {
        builder.target(env_logger::Target::Pipe(Box::new(Tee {
            file: std::sync::Mutex::new(file),
        })));
    }

    // 未显式设置 RUST_LOG 时使用上面的默认级别,否则尊重用户配置。
    if std::env::var("RUST_LOG").is_ok() {
        builder.parse_default_env();
    }

    let _ = builder.try_init();
}

/// 把字符串级别转换为 `log::LevelFilter`。
fn parse_level(text: &str) -> log::LevelFilter {
    match text.to_ascii_lowercase().as_str() {
        "off" => log::LevelFilter::Off,
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "info" => log::LevelFilter::Info,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Warn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_parsing_falls_back_to_warn() {
        assert_eq!(parse_level("debug"), log::LevelFilter::Debug);
        assert_eq!(parse_level("INFO"), log::LevelFilter::Info);
        assert_eq!(parse_level("未知"), log::LevelFilter::Warn);
    }

    #[test]
    fn log_file_path_lives_under_the_app_data_directory() {
        let path = log_file_path().expect("应用数据目录应当可用");
        let tail: Vec<String> = path
            .components()
            .rev()
            .take(2)
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(tail, vec!["app.log".to_string(), "log".to_string()]);
    }

    #[test]
    fn tee_writes_through_to_the_file() {
        let dir = crate::util::fs::temp_dir("log-tee");
        let path = dir.join("app.log");
        let file = std::fs::File::create(&path).unwrap();

        let mut tee = Tee {
            file: std::sync::Mutex::new(file),
        };
        tee.write_all(b"first\n").unwrap();
        tee.write_all(b"second\n").unwrap();
        tee.flush().unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\nsecond\n");

        drop(tee);
        std::fs::remove_dir_all(&dir).ok();
    }
}
