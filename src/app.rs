//! Greeter state machine: username/password entry, session picking, and
//! driving the greetd auth exchange, wired to the renderer for drawing and
//! to evdev for input.

use anyhow::Result;

use crate::config::Config;
use crate::greetd_client::{AuthStep, GreetdClient};
use crate::keymap::Key;
use crate::renderer::{Gpu, Rect, Renderer, SolidQuad};
use crate::session;

#[derive(PartialEq, Eq)]
enum Focus {
    Username,
    Password,
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
        if self.authenticating {
            // Ignore input while blocked on a greetd round-trip.
            return Outcome::Continue;
        }
        match key {
            Key::Char(c) => self.field_mut().push(c),
            Key::Backspace => {
                self.field_mut().pop();
            }
            Key::Delete => {
                self.field_mut().pop();
            }
            Key::Tab => {
                self.focus = match self.focus {
                    Focus::Username => Focus::Password,
                    Focus::Password => Focus::Username,
                };
            }
            Key::Up | Key::Left => {
                if !self.cfg.sessions.is_empty() {
                    self.session_index =
                        (self.session_index + self.cfg.sessions.len() - 1) % self.cfg.sessions.len();
                }
            }
            Key::Down | Key::Right => {
                if !self.cfg.sessions.is_empty() {
                    self.session_index = (self.session_index + 1) % self.cfg.sessions.len();
                }
            }
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
        }
        Outcome::Continue
    }

    fn field_mut(&mut self) -> &mut String {
        match self.focus {
            Focus::Username => &mut self.username,
            Focus::Password => &mut self.password,
        }
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
        let card_h = 320.0_f32.min(h - 40.0);
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

        if let Some(session) = self.cfg.sessions.get(self.session_index) {
            labels.push(renderer.make_label(
                &format!("Session: {}  (\u{2191}/\u{2193} to change)", session.name),
                font * 0.8,
                card_x + 32.0,
                password_y + field_h + 20.0,
                [170, 170, 180],
            ));
        }

        if self.authenticating {
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

        (panels, labels)
    }
}
