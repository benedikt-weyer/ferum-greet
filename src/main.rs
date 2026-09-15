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

fn run(cfg: Config) -> Result<()> {
    let mut drm = DrmBackend::open(&cfg.drm_device)?;
    log::info!("display mode: {}x{} @ {}Hz", drm.width(), drm.height(), drm.refresh_hz());

    let gpu = Gpu::new()?;
    let mut renderer = Renderer::new(&gpu, drm.width(), drm.height());

    let bg_image = background::load(&cfg.background, drm.width(), drm.height())?;
    let background_tex = renderer.upload_background(&gpu, drm.width(), drm.height(), bg_image.as_raw());

    let mut app = app::App::new(cfg);

    let (tx, rx) = mpsc::channel::<InputEvent>();
    input::spawn_keyboard_readers(tx);

    // Render once immediately so the screen isn't blank while waiting for
    // the first keypress, then redraw on every input event plus a slow
    // heartbeat so the cursor/clock (if any) stays fresh.
    loop {
        let (panels, labels) = app.draw(&gpu, &mut renderer);
        let frame = renderer.render_frame(&gpu, &background_tex, &panels, &labels)?;
        drm.present(&frame.data, frame.bytes_per_row)?;

        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(InputEvent::Key(key)) => {
                // Drain any additional buffered events so fast typing
                // doesn't cause one redraw per keystroke queue backlog.
                let mut pending = vec![key];
                while let Ok(InputEvent::Key(k)) = rx.try_recv() {
                    pending.push(k);
                }
                for key in pending {
                    if let Outcome::Exit = app.handle_key(key) {
                        return Ok(());
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("all keyboard input threads exited unexpectedly");
            }
        }
    }
}
