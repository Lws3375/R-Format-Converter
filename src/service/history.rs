//! 转换历史的读写与统计。
//!
//! 历史记录以 JSON 数组形式保存在应用数据目录下。每次转换结束后追加一条,超过
//! 上限时丢弃最早的记录,避免文件无限增长。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::convert::ConversionOutcome;
use crate::core::error::{ConvertError, Result};
use crate::core::format::Format;
use crate::util::fs::{app_data_dir, ensure_dir, human_size, read_file, write_file};

/// 历史记录文件名。
pub const HISTORY_FILE: &str = "history.json";

/// 最多保留的历史条数。
pub const MAX_HISTORY_ENTRIES: usize = 500;

/// 一条转换记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// 完成时间。
    pub timestamp: DateTime<Local>,
    /// 源文件路径。
    pub input: PathBuf,
    /// 输出文件路径。失败时为空。
    pub output: PathBuf,
    /// 源格式。识别失败时为 `None`。
    pub source_format: Option<Format>,
    /// 目标格式。
    pub target_format: Format,
    /// 输出图像宽度。
    pub width: u32,
    /// 输出图像高度。
    pub height: u32,
    /// 源文件大小。
    pub source_bytes: u64,
    /// 输出文件大小。
    pub output_bytes: u64,
    /// 耗时(毫秒)。
    pub elapsed_ms: u64,
    /// 是否成功。
    pub success: bool,
    /// 失败原因或成功提示。
    pub message: String,
}

impl HistoryEntry {
    /// 由一次成功的转换结果构造记录。
    pub fn from_outcome(outcome: &ConversionOutcome) -> Self {
        Self {
            timestamp: Local::now(),
            input: outcome.input.clone(),
            output: outcome.output.clone(),
            source_format: Some(outcome.source_format),
            target_format: outcome.target_format,
            width: outcome.width,
            height: outcome.height,
            source_bytes: outcome.source_bytes,
            output_bytes: outcome.output_bytes,
            elapsed_ms: outcome.elapsed.as_millis() as u64,
            success: true,
            message: if outcome.notes.is_empty() {
                "转换成功".to_string()
            } else {
                outcome.notes.join(";")
            },
        }
    }

    /// 由一次失败的转换构造记录。
    pub fn from_error(input: &Path, target: Format, error: &ConvertError, elapsed_ms: u64) -> Self {
        Self {
            timestamp: Local::now(),
            input: input.to_path_buf(),
            output: PathBuf::new(),
            source_format: None,
            target_format: target,
            width: 0,
            height: 0,
            source_bytes: 0,
            output_bytes: 0,
            elapsed_ms,
            success: false,
            message: error.user_message(),
        }
    }

    /// 由一条只有文字说明的失败信息构造记录。
    ///
    /// 后台任务在文件粒度上报失败时只带一段说明文字,没有 [`ConvertError`],
    /// 所以这里单独提供一个入口;源文件大小仍然尽量从磁盘读取,便于统计。
    pub fn from_failure(input: &Path, target: Format, message: &str) -> Self {
        Self {
            timestamp: Local::now(),
            input: input.to_path_buf(),
            output: PathBuf::new(),
            source_format: None,
            target_format: target,
            width: 0,
            height: 0,
            source_bytes: std::fs::metadata(input).map(|meta| meta.len()).unwrap_or(0),
            output_bytes: 0,
            elapsed_ms: 0,
            success: false,
            message: message.to_string(),
        }
    }

    /// 时间列的显示文本。
    pub fn time_text(&self) -> String {
        self.timestamp.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    /// 格式列的显示文本,例如 `BMP → QOI`。
    pub fn format_text(&self) -> String {
        match self.source_format {
            Some(source) => format!("{} → {}", source.id().to_uppercase(), self.target_format.id().to_uppercase()),
            None => format!("? → {}", self.target_format.id().to_uppercase()),
        }
    }

    /// 尺寸列的显示文本。
    pub fn size_text(&self) -> String {
        if self.width == 0 || self.height == 0 {
            "—".to_string()
        } else {
            format!("{}×{}", self.width, self.height)
        }
    }

    /// 体积变化列的显示文本。
    pub fn volume_text(&self) -> String {
        if !self.success {
            return "—".to_string();
        }
        format!(
            "{} → {}",
            human_size(self.source_bytes),
            human_size(self.output_bytes)
        )
    }

    /// 耗时列的显示文本。
    pub fn elapsed_text(&self) -> String {
        if self.elapsed_ms >= 1000 {
            format!("{:.2} s", self.elapsed_ms as f64 / 1000.0)
        } else {
            format!("{} ms", self.elapsed_ms)
        }
    }

    /// 状态列的显示文本。
    pub fn status_text(&self) -> &'static str {
        if self.success { "成功" } else { "失败" }
    }
}

/// 历史记录集合。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryStore {
    /// 记录列表,按时间先后排列,末尾为最新。
    pub entries: Vec<HistoryEntry>,
}

impl HistoryStore {
    /// 历史文件路径。
    pub fn path() -> PathBuf {
        app_data_dir().join(HISTORY_FILE)
    }

    /// 从默认位置读取,失败时返回空集合。
    pub fn load() -> Self {
        Self::load_from(&Self::path()).unwrap_or_default()
    }

    /// 从指定文件读取。
    pub fn load_from(path: &Path) -> Result<Self> {
        let bytes = read_file(path)?;
        let mut store: HistoryStore = serde_json::from_slice(&bytes)?;
        store.truncate_to_limit();
        Ok(store)
    }

    /// 写入默认位置。
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    /// 写入指定文件。
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            ensure_dir(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        write_file(path, text.as_bytes())
    }

    /// 追加一条记录,并保证不超过上限。
    pub fn push(&mut self, entry: HistoryEntry) {
        self.entries.push(entry);
        self.truncate_to_limit();
    }

    /// 清空全部记录。
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// 记录条数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否没有任何记录。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 成功的条数。
    pub fn succeeded(&self) -> usize {
        self.entries.iter().filter(|entry| entry.success).count()
    }

    /// 失败的条数。
    pub fn failed(&self) -> usize {
        self.entries.iter().filter(|entry| !entry.success).count()
    }

    /// 累计处理过的输入字节数。
    pub fn total_source_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.source_bytes).sum()
    }

    /// 累计产生的输出字节数。
    pub fn total_output_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.output_bytes).sum()
    }

    /// 最近 `count` 条记录,最新的排在最前。
    pub fn recent(&self, count: usize) -> Vec<&HistoryEntry> {
        self.entries.iter().rev().take(count).collect()
    }

    /// 按关键字过滤记录(匹配输入路径、输出路径或消息)。
    ///
    /// 关键字为空时返回全部记录,最新的排在最前。
    pub fn search(&self, keyword: &str) -> Vec<&HistoryEntry> {
        let keyword = keyword.trim().to_lowercase();
        self.entries
            .iter()
            .rev()
            .filter(|entry| {
                if keyword.is_empty() {
                    return true;
                }
                let haystack = format!(
                    "{} {} {}",
                    entry.input.display(),
                    entry.output.display(),
                    entry.message
                )
                .to_lowercase();
                haystack.contains(&keyword)
            })
            .collect()
    }

    /// 只保留最近 `count` 条记录。
    pub fn retain_recent(&mut self, count: usize) {
        if self.entries.len() > count {
            let start = self.entries.len() - count;
            self.entries.drain(..start);
        }
    }

    /// 统计摘要,展示在历史页顶部。
    pub fn summary(&self) -> String {
        if self.entries.is_empty() {
            return "暂无转换记录".to_string();
        }
        format!(
            "共 {} 条记录,成功 {} 条,失败 {} 条,累计输出 {}",
            self.len(),
            self.succeeded(),
            self.failed(),
            human_size(self.total_output_bytes())
        )
    }

    /// 裁剪到上限。
    fn truncate_to_limit(&mut self) {
        self.retain_recent(MAX_HISTORY_ENTRIES);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::fs::temp_dir;
    use std::time::Duration;

    fn entry(name: &str, success: bool) -> HistoryEntry {
        HistoryEntry {
            timestamp: Local::now(),
            input: PathBuf::from(format!("C:/in/{name}.bmp")),
            output: PathBuf::from(format!("C:/out/{name}.qoi")),
            source_format: Some(Format::Bmp),
            target_format: Format::Qoi,
            width: 8,
            height: 4,
            source_bytes: 1024,
            output_bytes: 512,
            elapsed_ms: 12,
            success,
            message: if success {
                "转换成功".to_string()
            } else {
                "文件损坏".to_string()
            },
        }
    }

    #[test]
    fn store_round_trips_through_file() {
        let dir = temp_dir("history");
        let path = dir.join("history.json");

        let mut store = HistoryStore::default();
        store.push(entry("a", true));
        store.push(entry("b", false));
        store.save_to(&path).unwrap();

        let loaded = HistoryStore::load_from(&path).unwrap();
        assert_eq!(loaded, store);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn counters_and_totals_are_correct() {
        let mut store = HistoryStore::default();
        store.push(entry("a", true));
        store.push(entry("b", true));
        store.push(entry("c", false));
        assert_eq!(store.len(), 3);
        assert_eq!(store.succeeded(), 2);
        assert_eq!(store.failed(), 1);
        assert_eq!(store.total_source_bytes(), 3072);
        assert_eq!(store.total_output_bytes(), 1536);
        assert!(store.summary().contains("成功 2 条"));
    }

    #[test]
    fn empty_store_has_readable_summary() {
        let store = HistoryStore::default();
        assert!(store.is_empty());
        assert_eq!(store.summary(), "暂无转换记录");
    }

    #[test]
    fn recent_returns_newest_first() {
        let mut store = HistoryStore::default();
        for name in ["a", "b", "c"] {
            store.push(entry(name, true));
        }
        let recent = store.recent(2);
        assert_eq!(recent.len(), 2);
        assert!(recent[0].input.to_string_lossy().contains('c'));
        assert!(recent[1].input.to_string_lossy().contains('b'));
    }

    #[test]
    fn search_matches_paths_and_messages() {
        let mut store = HistoryStore::default();
        store.push(entry("alpha", true));
        store.push(entry("beta", false));

        assert_eq!(store.search("").len(), 2);
        assert_eq!(store.search("alpha").len(), 1);
        assert_eq!(store.search("文件损坏").len(), 1);
        assert_eq!(store.search("gamma").len(), 0);
    }

    #[test]
    fn retain_recent_drops_oldest() {
        let mut store = HistoryStore::default();
        for name in ["a", "b", "c", "d"] {
            store.push(entry(name, true));
        }
        store.retain_recent(2);
        assert_eq!(store.len(), 2);
        assert!(store.entries[0].input.to_string_lossy().contains('c'));
    }

    #[test]
    fn limit_is_enforced_on_push() {
        let mut store = HistoryStore::default();
        for index in 0..MAX_HISTORY_ENTRIES + 5 {
            store.push(entry(&format!("f{index}"), true));
        }
        assert_eq!(store.len(), MAX_HISTORY_ENTRIES);
    }

    #[test]
    fn error_entry_carries_message() {
        let error = ConvertError::corrupt("位图数据长度不符");
        let record = HistoryEntry::from_error(Path::new("C:/in/bad.bmp"), Format::Qoi, &error, 3);
        assert!(!record.success);
        assert!(record.message.contains("长度不符"));
        assert_eq!(record.format_text(), "? → QOI");
        assert_eq!(record.size_text(), "—");
        assert_eq!(record.volume_text(), "—");
        assert_eq!(record.status_text(), "失败");
    }

    #[test]
    fn outcome_entry_is_marked_successful() {
        let outcome = ConversionOutcome {
            input: PathBuf::from("C:/in/a.bmp"),
            output: PathBuf::from("C:/out/a.qoi"),
            source_format: Format::Bmp,
            target_format: Format::Qoi,
            width: 10,
            height: 20,
            source_bytes: 4000,
            output_bytes: 1000,
            elapsed: Duration::from_millis(1500),
            notes: vec!["输出体积比原文件大约 5%".to_string()],
        };
        let record = HistoryEntry::from_outcome(&outcome);
        assert!(record.success);
        assert_eq!(record.format_text(), "BMP → QOI");
        assert_eq!(record.size_text(), "10×20");
        assert_eq!(record.elapsed_text(), "1.50 s");
        assert!(record.volume_text().contains("→"));
        assert!(record.message.contains("大约"));
        assert_eq!(record.status_text(), "成功");
    }
}
