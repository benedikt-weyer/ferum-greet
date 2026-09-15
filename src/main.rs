//! ferum-greet: a GPU-rendered login manager greeter for greetd.
//!
//! Renders directly to a KMS/DRM scanout buffer with wgpu (no Wayland/X11
//! compositor involved) and reads keyboard input straight from evdev.

mod app;
mod background;
mod config;
mod drm_backend;
mod greetd_client;
mod input;
mod keymap;
mod power;
mod renderer;
mod session;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;

use app::Outcome;
use config::Config;
use drm_backend::DrmBackend;
use input::InputEvent;
use renderer::{Gpu, Renderer};

fn main() -> std::process::ExitCode {
    let config_path = std::env::args()
        .nth(1)
        .filter(|a| a == "--config")
        .and_then(|_| std::env::args().nth(2))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(config::DEFAULT_CONFIG_PATH));
    let cfg = Config::load(&config_path);

    // greetd runs the greeter attached to its VT, like it would a TUI
    // greeter - our stderr ends up on that VT, not in the systemd journal.
    // Log to a file next to the other state we keep instead, so `journalctl
    // -u greetd` isn't the only way to see what happened.
    init_logging(&cfg.state_dir.join("ferum-greet.log"));

    match run(cfg) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            log::error!("fatal: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn init_logging(log_file: &std::path::Path) {
    let builder_with_env =
        || env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));

    if let Some(parent) = log_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::OpenOptions::new().create(true).append(true).open(log_file) {
        Ok(file) => builder_with_env()
            .target(env_logger::Target::Pipe(Box::new(file)))
            .init(),
        Err(err) => {
            builder_with_env().init();
            log::warn!(
                "could not open log file {} ({err}), logging to stderr instead",
                log_file.display()
            );
        }
    }
}

/// Everything needed to draw and present one connected display: its own
/// wgpu render target (outputs can have different resolutions) and
/// background texture, matched by index to a `DrmBackend` output.
struct OutputSurface {
    renderer: Renderer,
    background_tex: renderer::BackgroundTexture,
}

fn run(cfg: Config) -> Result<()> {
    let mut drm = DrmBackend::open(&cfg.drm_device)?;
    let gpu = Gpu::new()?;

    let mut surfaces = Vec::with_capacity(drm.output_count());
    for idx in 0..drm.output_count() {
        let (width, height) = (drm.width(idx), drm.height(idx));
        log::info!("display {idx}: {width}x{height} @ {}Hz", drm.refresh_hz(idx));

        let renderer = Renderer::new(&gpu, width, height);
        let bg_image = background::load(&cfg.background, width, height)?;
        let background_tex = renderer.upload_background(&gpu, width, height, bg_image.as_raw());
        surfaces.push(OutputSurface { renderer, background_tex });
    }

    let mut app = app::App::new(cfg);

    // Cursor position, tracked in output 0's pixel space and scaled to each
    // output's own resolution when drawing (see below) so a mirrored setup
    // shows the pointer at the same relative spot everywhere. `None` until
    // a pointer device actually reports motion, so a mouseless greeter
    // never draws or hit-tests a cursor at all.
    let (ref_w, ref_h) = (drm.width(0) as f32, drm.height(0) as f32);
    let mut cursor: Option<(f32, f32)> = None;

    let (tx, rx) = mpsc::channel::<InputEvent>();
    input::spawn_keyboard_readers(tx.clone());
    input::spawn_pointer_readers(tx);

    // Render once immediately so the screens aren't blank while waiting for
    // the first keypress, then redraw on every input event plus a slow
    // heartbeat so the cursor/clock (if any) stays fresh. The same App
    // state drives every output, so all monitors mirror the same UI.
    loop {
        for (idx, surface) in surfaces.iter_mut().enumerate() {
            let (out_w, out_h) = (surface.renderer.width() as f32, surface.renderer.height() as f32);
            let out_cursor = cursor.map(|(x, y)| (x / ref_w * out_w, y / ref_h * out_h));
            let (panels, labels) = app.draw(&gpu, &mut surface.renderer, out_cursor);
            let frame = surface
                .renderer
                .render_frame(&gpu, &surface.background_tex, &panels, &labels)?;
            drm.present(idx, &frame.data, frame.bytes_per_row)?;
        }

        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(event) => {
                // Drain any additional buffered events so fast typing/mouse
                // motion doesn't cause one redraw per queued event.
                let mut pending = vec![event];
                while let Ok(event) = rx.try_recv() {
                    pending.push(event);
                }
                for event in pending {
                    match event {
                        InputEvent::Key(key) => {
                            if let Outcome::Exit = app.handle_key(key) {
                                return Ok(());
                            }
                        }
                        InputEvent::MouseMove { dx, dy } => {
                            let (x, y) = cursor.get_or_insert((ref_w / 2.0, ref_h / 2.0));
                            *x = (*x + dx).clamp(0.0, ref_w);
                            *y = (*y + dy).clamp(0.0, ref_h);
                        }
                        InputEvent::MouseButton { pressed: true } => {
                            if let Some((x, y)) = cursor {
                                app.handle_click(x, y, ref_w, ref_h);
                            }
                        }
                        InputEvent::MouseButton { pressed: false } => {}
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("all input threads exited unexpectedly");
            }
        }
    }
}
