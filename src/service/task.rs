//! 批量转换的后台调度。
//!
//! 界面提交一批文件后,这里会启动一个协调线程和若干工作线程:协调线程负责派发
//! 任务、汇总结果,工作线程从共享队列里取文件逐个转换,并通过通道把进度事件回传
//! 给界面。采用"共享队列 + 按需取任务"而不是"预先分块",可以避免某个线程分到
//! 几个超大文件时拖慢整批任务。
//!
//! 只用标准库的 [`std::thread`] 与 [`std::sync::mpsc`],不引入额外的线程池依赖。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::convert::{convert_file, ConversionOutcome};
use crate::core::options::ConvertOptions;
use crate::core::registry::Registry;

/// 批量任务在运行过程中向界面汇报的事件。
#[derive(Debug)]
pub enum TaskEvent {
    /// 任务开始,`total` 为待处理文件总数。
    Started { total: usize },
    /// 开始处理第 `index` 个文件。
    FileStarted { index: usize, path: PathBuf },
    /// 第 `index` 个文件转换成功。
    FileFinished {
        index: usize,
        outcome: Box<ConversionOutcome>,
    },
    /// 第 `index` 个文件转换失败。
    FileFailed {
        index: usize,
        path: PathBuf,
        message: String,
    },
    /// 全部文件处理完毕(或被取消)。
    Finished(TaskSummary),
}

/// 一次批量任务的汇总结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSummary {
    /// 文件总数。
    pub total: usize,
    /// 成功数。
    pub succeeded: usize,
    /// 失败数。
    pub failed: usize,
    /// 是否被用户取消。
    pub cancelled: bool,
    /// 整批任务的耗时。
    pub elapsed: Duration,
}

impl TaskSummary {
    /// 已处理完的文件数(成功 + 失败)。
    pub fn processed(&self) -> usize {
        self.succeeded + self.failed
    }

    /// 完成比例,范围 0.0~1.0。
    pub fn progress(&self) -> f32 {
        if self.total == 0 {
            return 1.0;
        }
        (self.processed() as f32 / self.total as f32).clamp(0.0, 1.0)
    }

    /// 结果描述,展示在状态栏。
    pub fn summary_text(&self) -> String {
        let mut text = format!(
            "成功 {} 个,失败 {} 个,用时 {:.2} 秒",
            self.succeeded,
            self.failed,
            self.elapsed.as_secs_f64()
        );
        if self.cancelled {
            text.push_str("(已取消)");
        }
        text
    }
}

/// 批量转换任务句柄。
///
/// 句柄只负责"提交"和"接收进度",不做任何阻塞等待,界面可以在每帧调用
/// [`TaskRunner::drain`] 把新事件取出来刷新界面。
pub struct TaskRunner {
    receiver: Receiver<TaskEvent>,
    cancel: Arc<AtomicBool>,
    coordinator: JoinHandle<()>,
    total: usize,
}

impl TaskRunner {
    /// 启动一次批量转换。
    ///
    /// `workers` 会被限制在 `1..=文件数` 之间;文件数为 0 时也会正常发送开始与结束
    /// 事件,界面无需特判。
    pub fn spawn(
        files: Vec<PathBuf>,
        registry: Arc<Registry>,
        options: ConvertOptions,
        workers: usize,
    ) -> Self {
        let total = files.len();
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));

        let coordinator = {
            let cancel = Arc::clone(&cancel);
            thread::Builder::new()
                .name("r-convert-coordinator".to_string())
                .spawn(move || {
                    run_batch(files, registry, options, workers, cancel, sender);
                })
                .expect("无法创建批量转换线程")
        };

        Self {
            receiver,
            cancel,
            coordinator,
            total,
        }
    }

    /// 待处理文件总数。
    pub fn total(&self) -> usize {
        self.total
    }

    /// 取出当前已到达的全部事件,不阻塞。
    pub fn drain(&self) -> Vec<TaskEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            events.push(event);
        }
        events
    }

    /// 取出一个事件,没有则返回 `None`。
    pub fn try_recv(&self) -> Option<TaskEvent> {
        self.receiver.try_recv().ok()
    }

    /// 请求取消。已经在转换中的文件会跑完,尚未开始的会被跳过。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// 是否已被请求取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 任务是否仍在运行。
    pub fn is_running(&self) -> bool {
        !self.coordinator.is_finished()
    }
}

impl Drop for TaskRunner {
    fn drop(&mut self) {
        // 句柄被丢弃说明界面已经不再需要结果,顺手取消剩余任务,避免白干活。
        self.cancel();
    }
}

/// 协调线程的主体逻辑。
fn run_batch(
    files: Vec<PathBuf>,
    registry: Arc<Registry>,
    options: ConvertOptions,
    workers: usize,
    cancel: Arc<AtomicBool>,
    sender: Sender<TaskEvent>,
) {
    let started = Instant::now();
    let total = files.len();
    let _ = sender.send(TaskEvent::Started { total });

    if total > 0 {
        let worker_count = workers.clamp(1, total);
        let queue = Arc::new(Mutex::new(
            files.into_iter().enumerate().collect::<VecDeque<_>>(),
        ));
        let succeeded = Arc::new(AtomicUsize::new(0));
        let failed = Arc::new(AtomicUsize::new(0));

        thread::scope(|scope| {
            for _ in 0..worker_count {
                let queue = Arc::clone(&queue);
                let registry = Arc::clone(&registry);
                let cancel = Arc::clone(&cancel);
                let succeeded = Arc::clone(&succeeded);
                let failed = Arc::clone(&failed);
                let sender = sender.clone();
                let options = &options;

                scope.spawn(move || {
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        // 取任务时立即释放锁,转换过程不持有锁。
                        let next = queue
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .pop_front();
                        let Some((index, path)) = next else {
                            break;
                        };

                        let _ = sender.send(TaskEvent::FileStarted {
                            index,
                            path: path.clone(),
                        });

                        match convert_file(&path, &registry, options) {
                            Ok(outcome) => {
                                succeeded.fetch_add(1, Ordering::Relaxed);
                                let _ = sender.send(TaskEvent::FileFinished {
                                    index,
                                    outcome: Box::new(outcome),
                                });
                            }
                            Err(error) => {
                                failed.fetch_add(1, Ordering::Relaxed);
                                let _ = sender.send(TaskEvent::FileFailed {
                                    index,
                                    path,
                                    message: error.user_message(),
                                });
                            }
                        }
                    }
                });
            }
        });

        let _ = sender.send(TaskEvent::Finished(TaskSummary {
            total,
            succeeded: succeeded.load(Ordering::Relaxed),
            failed: failed.load(Ordering::Relaxed),
            cancelled: cancel.load(Ordering::Relaxed),
            elapsed: started.elapsed(),
        }));
    } else {
        let _ = sender.send(TaskEvent::Finished(TaskSummary {
            total: 0,
            succeeded: 0,
            failed: 0,
            cancelled: false,
            elapsed: started.elapsed(),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::build_default;
    use crate::core::format::Format;
    use crate::core::image::Image;
    use crate::core::options::EncodeOptions;
    use crate::util::fs::temp_dir;

    /// 在临时目录里生成 `count` 个 BMP 文件,返回目录与文件列表。
    ///
    /// `side` 控制图片边长,取消相关的测试需要单张图片足够大,才能稳定地观察到
    /// "尚未处理完就取消"的状态。
    fn prepare_inputs(tag: &str, count: usize, side: u32) -> (PathBuf, Vec<PathBuf>) {
        let registry = build_default();
        let dir = temp_dir(tag);
        let mut files = Vec::new();
        for index in 0..count {
            let mut buffer = image::RgbaImage::new(side, side);
            buffer.put_pixel(0, 0, image::Rgba([index as u8, 10, 20, 255]));
            let image = Image::ImageRgba8(buffer);
            let bytes = registry
                .encode(&image, Format::Bmp, &EncodeOptions::default())
                .unwrap();
            let path = dir.join(format!("in{index}.bmp"));
            std::fs::write(&path, &bytes).unwrap();
            files.push(path);
        }
        (dir, files)
    }

    /// 等待任务结束并返回最终汇总。
    fn wait_for_summary(runner: &TaskRunner) -> TaskSummary {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            for event in runner.drain() {
                if let TaskEvent::Finished(summary) = event {
                    return summary;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("等待批量任务结束超时");
    }

    #[test]
    fn batch_converts_every_file() {
        let (dir, files) = prepare_inputs("task-ok", 5, 4);
        let registry = Arc::new(build_default());
        let options = ConvertOptions::new(Format::Qoi, dir.join("out"));

        let runner = TaskRunner::spawn(files, Arc::clone(&registry), options, 3);
        let summary = wait_for_summary(&runner);

        assert_eq!(summary.total, 5);
        assert_eq!(summary.succeeded, 5);
        assert_eq!(summary.failed, 0);
        assert!(!summary.cancelled);
        assert_eq!(summary.processed(), 5);
        assert!((summary.progress() - 1.0).abs() < f32::EPSILON);
        assert!(summary.summary_text().contains("成功 5 个"));

        // 输出目录里应当有 5 个结果文件。
        let produced = std::fs::read_dir(dir.join("out")).unwrap().count();
        assert_eq!(produced, 5);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failing_files_are_reported_without_stopping_the_batch() {
        let (dir, mut files) = prepare_inputs("task-mixed", 3, 4);
        let bad = dir.join("broken.bmp");
        std::fs::write(&bad, b"not an image at all").unwrap();
        files.push(bad);

        let registry = Arc::new(build_default());
        let options = ConvertOptions::new(Format::Qoi, dir.join("out"));

        let runner = TaskRunner::spawn(files, registry, options, 2);
        let summary = wait_for_summary(&runner);

        assert_eq!(summary.total, 4);
        assert_eq!(summary.succeeded, 3);
        assert_eq!(summary.failed, 1);
        assert!(!summary.cancelled);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn events_carry_indexes_and_outcomes() {
        let (dir, files) = prepare_inputs("task-events", 2, 4);
        let registry = Arc::new(build_default());
        let options = ConvertOptions::new(Format::Farbfeld, dir.join("out"));

        let runner = TaskRunner::spawn(files, registry, options, 1);

        let mut started_indexes = Vec::new();
        let mut finished_indexes = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            for event in runner.drain() {
                match event {
                    TaskEvent::FileStarted { index, .. } => started_indexes.push(index),
                    TaskEvent::FileFinished { index, .. } => finished_indexes.push(index),
                    TaskEvent::Finished(_) => {}
                    _ => {}
                }
            }
            if !runner.is_running() {
                for event in runner.drain() {
                    if let TaskEvent::FileFinished { index, .. } = event {
                        finished_indexes.push(index);
                    }
                }
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }

        started_indexes.sort_unstable();
        finished_indexes.sort_unstable();
        assert_eq!(started_indexes, vec![0, 1]);
        assert_eq!(finished_indexes, vec![0, 1]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn empty_batch_finishes_immediately() {
        let registry = Arc::new(build_default());
        let options = ConvertOptions::new(Format::Qoi, PathBuf::new());
        let runner = TaskRunner::spawn(Vec::new(), registry, options, 4);
        let summary = wait_for_summary(&runner);
        assert_eq!(summary.total, 0);
        assert_eq!(summary.progress(), 1.0);
        assert!(summary.summary_text().contains("成功 0 个"));
    }

    #[test]
    fn cancel_stops_pending_files() {
        // 图片刻意做大一些,保证"取消"发生在整批处理完之前。
        let (dir, files) = prepare_inputs("task-cancel", 40, 256);
        let registry = Arc::new(build_default());
        let options = ConvertOptions::new(Format::Qoi, dir.join("out"));

        let runner = TaskRunner::spawn(files, registry, options, 1);
        runner.cancel();
        assert!(runner.is_cancelled());

        let summary = wait_for_summary(&runner);
        assert!(summary.cancelled);
        assert!(summary.processed() < summary.total);
        assert!(summary.summary_text().contains("已取消"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn progress_reflects_processed_files() {
        let summary = TaskSummary {
            total: 4,
            succeeded: 1,
            failed: 1,
            cancelled: false,
            elapsed: Duration::from_millis(20),
        };
        assert_eq!(summary.processed(), 2);
        assert!((summary.progress() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn workers_are_clamped_to_file_count() {
        let (dir, files) = prepare_inputs("task-clamp", 1, 4);
        let registry = Arc::new(build_default());
        let options = ConvertOptions::new(Format::Qoi, dir.join("out"));
        // 请求 16 个线程,但只有 1 个文件,不应出错。
        let runner = TaskRunner::spawn(files, registry, options, 16);
        let summary = wait_for_summary(&runner);
        assert_eq!(summary.succeeded, 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
