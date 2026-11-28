#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bookmarks;
mod chat;
mod chrome_ui;
mod cite;
mod cleaner;
mod events;
mod favicon;
mod history;
mod icons;
mod memory;
mod net;
mod page_cache;
mod pdfjs;
mod prefs;
mod screenshot;
mod single;
mod tabs;
mod translate;
mod update;

use app::{BrowserApp, LaunchOptions};
use winit::event_loop::{ControlFlow, EventLoop};

fn main() {
    let launch = LaunchOptions::from_args(std::env::args().skip(1));
    // A file/URL launched while the browser is already running opens as a tab there,
    // unless `--new-window` was requested (e.g. detaching a tab into its own window).
    if !launch.new_window && launch.initial_url.as_deref().is_some_and(single::forward) {
        return;
    }
    // First run: ask before touching other browsers (welcome page), so skip the
    // silent auto-import this once and let the user's choice drive it.
    let ask_first_run = !prefs::UiPrefs::load().onboarded && !bookmarks::BookmarkStore::has_personal_bookmarks();
    // Parse Chrome's bookmark file while the runtime and window system initialise.
    let bookmarks = std::thread::spawn(move || {
        if ask_first_run {
            bookmarks::BookmarkStore::load_without_auto_import()
        } else {
            bookmarks::BookmarkStore::load()
        }
    });
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

    let proxy = event_loop.create_proxy();
    let app = BrowserApp::new(proxy, launch, bookmarks, ask_first_run);
    event_loop.run_app(app).expect("event loop error");
}
