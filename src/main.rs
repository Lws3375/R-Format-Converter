//! R格式转换器软件 V1.0 —— 可执行程序入口。
//!
//! 这里只做三件事:初始化日志、描述窗口参数、启动界面。所有业务逻辑都在库
//! (`r_format_converter`) 中实现,便于单元测试与将来复用。

// 发布版本不带控制台窗口;调试版本保留,方便查看日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use egui::ViewportBuilder;

use r_format_converter::app::{build_icon, parse_startup_args, App};
use r_format_converter::core::{window_title, APP_FULL_NAME};
use r_format_converter::util::log;

fn main() -> eframe::Result {
    // 调试版本输出 debug 级日志,发布版本输出 info 级日志并写入文件。
    log::init(cfg!(debug_assertions));

    // 命令行参数:待载入的文件、可选的目标分页,以及载入后是否立刻开始转换。
    let startup = parse_startup_args(std::env::args_os().skip(1));
    log::note_startup(startup.files.len());

    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title(window_title())
            .with_inner_size([1120.0, 780.0])
            .with_min_inner_size([900.0, 600.0])
            .with_icon(build_icon()),
        centered: true,
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        APP_FULL_NAME,
        options,
        Box::new(move |cc| {
            let mut app = App::new(cc);
            app.preload(startup.files);
            if let Some(view) = startup.view {
                app.show_view(view);
            }
            if startup.auto_start {
                app.auto_start();
            }
            Ok(Box::new(app))
        }),
    )
}

