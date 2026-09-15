//! Greeter state machine: username/password entry, session picking, and
//! driving the greetd auth exchange, wired to the renderer for drawing and
//! to evdev for input.

use anyhow::Result;

use crate::config::Config;
use crate::greetd_client::{AuthStep, GreetdClient};
use crate::keymap::Key;
use crate::power::PowerAction;
use crate::renderer::{Gpu, Rect, Renderer, SolidQuad};
use crate::session;

#[derive(PartialEq, Eq)]
enum Focus {
    Username,
    Password,
    Session,
}

pub enum Outcome {
    /// The user session was handed off to greetd; the greeter should exit.
    Exit,
    /// Keep looping.
    Continue,
}

pub struct App {
    cfg: Config,
    username: String,
    password: String,
    focus: Focus,
    session_index: usize,
    message: Option<(String, bool)>,
    authenticating: bool,
    confirm_power: Option<PowerAction>,
}

impl App {
    pub fn new(cfg: Config) -> Self {
        let username = session::load_last_user(&cfg).unwrap_or_default();
        let session_index = cfg.selected_session_index();
        App {
            username,
            password: String::new(),
            focus: Focus::Password,
            session_index,
            message: None,
            authenticating: false,
            confirm_power: None,
            cfg,
        }
        .with_initial_focus()
    }

    fn with_initial_focus(mut self) -> Self {
        self.focus = if self.username.is_empty() {
            Focus::Username
        } else {
            Focus::Password
        };
        self
    }

    pub fn handle_key(&mut self, key: Key) -> Outcome {
        if let Some(action) = self.confirm_power {
            match key {
                Key::Enter => {
                    if let Err(err) = action.execute() {
                        log::error!("power action failed: {err}");
                        self.message = Some((format!("Power action failed: {err}"), true));
                    }
                    self.confirm_power = None;
                }
                Key::Escape => {
                    self.confirm_power = None;
                    self.message = None;
                }
                _ => {}
            }
            return Outcome::Continue;
        }
        if self.authenticating {
            // Ignore input while blocked on a greetd round-trip.
            return Outcome::Continue;
        }
        match key {
            Key::Char(c) => {
                if let Some(field) = self.field_mut() {
                    field.push(c);
                }
            }
            Key::Backspace | Key::Delete => {
                if let Some(field) = self.field_mut() {
                    field.pop();
                }
            }
            Key::Tab => {
                self.focus = match self.focus {
                    Focus::Username => Focus::Password,
                    Focus::Password => Focus::Session,
                    Focus::Session => Focus::Username,
                };
            }
            Key::Up | Key::Left => self.cycle_session(-1),
            Key::Down | Key::Right => self.cycle_session(1),
            Key::Escape => {
                self.password.clear();
                self.message = None;
            }
            Key::Enter => {
                if self.username.is_empty() {
                    self.focus = Focus::Username;
                    return Outcome::Continue;
                }
                return self.try_login();
            }
            Key::Shutdown => {
                self.confirm_power = Some(PowerAction::Shutdown);
                self.message = None;
            }
            Key::Reboot => {
                self.confirm_power = Some(PowerAction::Reboot);
                self.message = None;
            }
        }
        Outcome::Continue
    }

    fn field_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            Focus::Username => Some(&mut self.username),
            Focus::Password => Some(&mut self.password),
            Focus::Session => None,
        }
    }

    fn cycle_session(&mut self, delta: i32) {
        let len = self.cfg.sessions.len();
        if len == 0 {
            return;
        }
        let len = len as i32;
        self.session_index = ((self.session_index as i32 + delta).rem_euclid(len)) as usize;
    }

    fn try_login(&mut self) -> Outcome {
        self.authenticating = true;
        self.message = None;
        let result = self.run_auth_flow();
        self.authenticating = false;
        match result {
            Ok(true) => Outcome::Exit,
            Ok(false) => {
                self.password.clear();
                Outcome::Continue
            }
            Err(err) => {
                log::error!("login flow failed: {err}");
                self.message = Some((format!("Login error: {err}"), true));
                self.password.clear();
                Outcome::Continue
            }
        }
    }

    /// Runs the full greetd exchange for one login attempt. Returns
    /// `Ok(true)` if a session was started (the greeter should exit),
    /// `Ok(false)` if authentication failed (message already set).
    fn run_auth_flow(&mut self) -> Result<bool> {
        let mut client = GreetdClient::connect()?;
        let mut step = client.create_session(&self.username)?;
        let mut password_sent = false;

        loop {
            match step {
                AuthStep::Prompt { message, secret } => {
                    log::debug!("greetd prompt: {message} (secret={secret})");
                    let answer = if secret && !password_sent {
                        password_sent = true;
                        self.password.clone()
                    } else {
                        String::new()
                    };
                    step = client.post_auth_response(Some(answer))?;
                }
                AuthStep::Info { message, is_error } => {
                    if is_error {
                        self.message = Some((message, true));
                    }
                    step = client.post_auth_response(None)?;
                }
                AuthStep::Success => break,
                AuthStep::Failure(description) => {
                    self.message = Some((
                        if description.is_empty() {
                            "Authentication failed".to_string()
                        } else {
                            description
                        },
                        true,
                    ));
                    return Ok(false);
                }
            }
        }

        let session = self
            .cfg
            .sessions
            .get(self.session_index)
            .cloned()
            .unwrap_or_else(|| crate::config::SessionConfig {
                name: "Default".to_string(),
                exec: "/bin/sh -l".to_string(),
                env: Vec::new(),
            });
        let cmd: Vec<String> = shell_words::split(&session.exec)
            .unwrap_or_else(|_| vec![session.exec.clone()]);

        match client.start_session(cmd, session.env.clone())? {
            AuthStep::Success => {
                session::save_last_user(&self.cfg, &self.username);
                Ok(true)
            }
            AuthStep::Failure(description) => {
                self.message = Some((description, true));
                let _ = client.cancel_session();
                Ok(false)
            }
            _ => {
                self.message = Some(("Unexpected response starting session".to_string(), true));
                let _ = client.cancel_session();
                Ok(false)
            }
        }
    }

    pub fn draw(&mut self, gpu: &Gpu, renderer: &mut Renderer) -> (Vec<SolidQuad>, Vec<crate::renderer::Label>) {
        let _ = gpu;
        let (w, h) = (renderer.width() as f32, renderer.height() as f32);

        let card_w = 420.0_f32.min(w - 40.0);
        let card_h = 388.0_f32.min(h - 40.0);
        let card_x = (w - card_w) / 2.0;
        let card_y = (h - card_h) / 2.0;

        let mut panels = vec![SolidQuad {
            rect: Rect { x: card_x, y: card_y, w: card_w, h: card_h },
            color: [0.08, 0.09, 0.11, 0.72],
            radius: 18.0,
        }];

        let field_w = card_w - 64.0;
        let field_h = 44.0;
        let username_y = card_y + 96.0;
        let password_y = username_y + field_h + 24.0;
        let session_y = password_y + field_h + 24.0;

        let accent = self.cfg.theme.accent_color;
        let accent_f = [accent[0] as f32 / 255.0, accent[1] as f32 / 255.0, accent[2] as f32 / 255.0, 1.0];

        let field_color = |focused: bool| {
            if focused {
                [accent_f[0] * 0.35, accent_f[1] * 0.35, accent_f[2] * 0.35, 0.9]
            } else {
                [1.0, 1.0, 1.0, 0.08]
            }
        };

        panels.push(SolidQuad {
            rect: Rect { x: card_x + 32.0, y: username_y, w: field_w, h: field_h },
            color: field_color(self.focus == Focus::Username),
            radius: 10.0,
        });
        panels.push(SolidQuad {
            rect: Rect { x: card_x + 32.0, y: password_y, w: field_w, h: field_h },
            color: field_color(self.focus == Focus::Password),
            radius: 10.0,
        });
        panels.push(SolidQuad {
            rect: Rect { x: card_x + 32.0, y: session_y, w: field_w, h: field_h },
            color: field_color(self.focus == Focus::Session),
            radius: 10.0,
        });

        let mut labels = Vec::new();
        let font = self.cfg.theme.font_size;

        labels.push(renderer.make_label("Sign in", font * 1.4, card_x + 32.0, card_y + 28.0, [235, 235, 240]));

        let username_display = if self.username.is_empty() { "Username" } else { &self.username };
        let username_color = if self.username.is_empty() { [140, 140, 150] } else { [230, 230, 235] };
        labels.push(renderer.make_label(
            username_display,
            font,
            card_x + 46.0,
            username_y + (field_h - font) / 2.0 - 2.0,
            username_color,
        ));

        let dots: String = "•".repeat(self.password.chars().count());
        let password_display = if self.password.is_empty() { "Password".to_string() } else { dots };
        let password_color = if self.password.is_empty() { [140, 140, 150] } else { [230, 230, 235] };
        labels.push(renderer.make_label(
            &password_display,
            font,
            card_x + 46.0,
            password_y + (field_h - font) / 2.0 - 2.0,
            password_color,
        ));

        let has_choice = self.cfg.sessions.len() > 1;
        let chevron_color = if has_choice { [190, 190, 200] } else { [90, 90, 100] };
        let session_text_y = session_y + (field_h - font) / 2.0 - 2.0;
        labels.push(renderer.make_label("\u{2039}", font, card_x + 46.0, session_text_y, chevron_color));
        let session_name = self
            .cfg
            .sessions
            .get(self.session_index)
            .map(|s| s.name.as_str())
            .unwrap_or("No sessions configured");
        labels.push(renderer.make_label(session_name, font, card_x + 72.0, session_text_y, [230, 230, 235]));
        labels.push(renderer.make_label(
            "\u{203a}",
            font,
            card_x + 32.0 + field_w - 28.0,
            session_text_y,
            chevron_color,
        ));

        if let Some(action) = self.confirm_power {
            labels.push(renderer.make_label(
                action.confirm_label(),
                font * 0.8,
                card_x + 32.0,
                card_y + card_h - 34.0,
                [230, 190, 110],
            ));
        } else if self.authenticating {
            labels.push(renderer.make_label(
                "Signing in\u{2026}",
                font * 0.85,
                card_x + 32.0,
                card_y + card_h - 34.0,
                accent,
            ));
        } else if let Some((message, is_error)) = &self.message {
            let color = if *is_error { [230, 110, 110] } else { [170, 200, 170] };
            labels.push(renderer.make_label(message, font * 0.8, card_x + 32.0, card_y + card_h - 34.0, color));
        }

        labels.push(renderer.make_label(
            "F1 Shut down   F2 Restart",
            font * 0.75,
            16.0,
            h - font * 0.75 - 16.0,
            [180, 180, 190],
        ));

        (panels, labels)
    }
}
