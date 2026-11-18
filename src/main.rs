#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bookmarks;
mod chrome_ui;
mod cleaner;
mod events;
mod favicon;
mod history;
mod icons;
mod memory;
mod net;
mod page_cache;
mod prefs;
mod tabs;

use app::BrowserApp;
use winit::event_loop::{ControlFlow, EventLoop};

fn main() {
    // Parse Chrome's bookmark file while the runtime and window system initialise.
    let bookmarks = std::thread::spawn(bookmarks::BookmarkStore::load);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        // DNS lookups etc. use the blocking pool; cap it and reap idle threads quickly.
        .max_blocking_threads(8)
        .thread_keep_alive(std::time::Duration::from_secs(10))
        .build()
        .expect("failed to build tokio runtime");
    let _guard = rt.enter();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);

    let initial_url = std::env::args().nth(1);
    let proxy = event_loop.create_proxy();
    let app = BrowserApp::new(proxy, initial_url, bookmarks);
    event_loop.run_app(app).expect("event loop error");
}
