//! The start floor (GLOSSARY.md): a bare winit window with nothing drawn.
//!
//! It opens a 1200×800 pt window, the size Genea's project window opens at,
//! and exits as soon as AppKit first reports it visible
//! (`WindowEvent::Occluded(false)`), which is the milestone Genea's start
//! budgets are margins over. Its last stdout line is one JSON object with
//! the milestones in ms since the kernel started the process:
//!
//! ```text
//! {"window_created_ms":95.1,"visible_ms":121.4}
//! ```
//!
//! winit is pinned to the version Slint links, so the floor goes through the
//! same window setup as Genea.

use genea_bench::sys;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

#[derive(Default)]
struct Floor {
    window: Option<Window>,
    created_ns: Option<u64>,
}

impl ApplicationHandler for Floor {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes =
            Window::default_attributes().with_title("Genea start floor").with_inner_size(LogicalSize::new(1200.0, 800.0));
        match event_loop.create_window(attributes) {
            Ok(window) => {
                self.created_ns = Some(sys::now_ns());
                self.window = Some(window);
            }
            Err(error) => {
                eprintln!("genea-floor: no window: {error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Occluded(false) => {
                let visible_ns = sys::now_ns();
                let start_ns = sys::usage(std::process::id()).map_or(0, |u| u.start_ns);
                let ms = |t: u64| (t.saturating_sub(start_ns) as f64 / 1e4).round() / 100.0;
                println!(
                    r#"{{"window_created_ms":{},"visible_ms":{}}}"#,
                    self.created_ns.map_or(f64::NAN, ms),
                    ms(visible_ns)
                );
                event_loop.exit();
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}

fn main() {
    let event_loop = match EventLoop::new() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("genea-floor: no event loop: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = event_loop.run_app(&mut Floor::default()) {
        eprintln!("genea-floor: {error}");
        std::process::exit(1);
    }
}
