//! Thin, blocking client for the greetd IPC protocol
//! (<https://man.sr.ht/~kennylevinsen/greetd/>).
//!
//! greetd passes the socket path to the greeter in `$GREETD_SOCK`; every
//! request/response is a length-prefixed JSON message over that
//! `AF_UNIX` stream.

use std::env;
use std::os::unix::net::UnixStream;

use anyhow::{Context, Result};
use greetd_ipc::codec::SyncCodec;
use greetd_ipc::{AuthMessageType, ErrorType, Request, Response};

pub struct GreetdClient {
    stream: UnixStream,
}

/// What the greeter should do next after sending a request.
pub enum AuthStep {
    /// greetd wants an answer to a prompt; `secret` says whether input
    /// should be masked.
    Prompt { message: String, secret: bool },
    /// An informational or error message to show, with no response
    /// expected - the caller should re-post an empty response to continue.
    Info { message: String, is_error: bool },
    /// Authentication succeeded; the session can now be started.
    Success,
    /// Authentication failed; the session was already cancelled by greetd.
    Failure(String),
}

impl GreetdClient {
    pub fn connect() -> Result<Self> {
        let sock_path =
            env::var("GREETD_SOCK").context("GREETD_SOCK is not set - not running under greetd")?;
        let stream = UnixStream::connect(&sock_path)
            .with_context(|| format!("connecting to greetd socket at {sock_path}"))?;
        Ok(GreetdClient { stream })
    }

    fn roundtrip(&mut self, req: Request) -> Result<AuthStep> {
        req.write_to(&mut self.stream)
            .context("writing request to greetd")?;
        let resp = Response::read_from(&mut self.stream).context("reading response from greetd")?;
        Ok(match resp {
            Response::Success => AuthStep::Success,
            Response::Error {
                error_type: ErrorType::AuthError,
                description,
            } => AuthStep::Failure(description),
            Response::Error {
                error_type: ErrorType::Error,
                description,
            } => AuthStep::Info {
                message: description,
                is_error: true,
            },
            Response::AuthMessage {
                auth_message_type,
                auth_message,
            } => match auth_message_type {
                AuthMessageType::Visible => AuthStep::Prompt {
                    message: auth_message,
                    secret: false,
                },
                AuthMessageType::Secret => AuthStep::Prompt {
                    message: auth_message,
                    secret: true,
                },
                AuthMessageType::Info => AuthStep::Info {
                    message: auth_message,
                    is_error: false,
                },
                AuthMessageType::Error => AuthStep::Info {
                    message: auth_message,
                    is_error: true,
                },
            },
        })
    }

    pub fn create_session(&mut self, username: &str) -> Result<AuthStep> {
        self.roundtrip(Request::CreateSession {
            username: username.to_string(),
        })
    }

    pub fn post_auth_response(&mut self, response: Option<String>) -> Result<AuthStep> {
        self.roundtrip(Request::PostAuthMessageResponse { response })
    }

    pub fn start_session(&mut self, cmd: Vec<String>, env: Vec<String>) -> Result<AuthStep> {
        self.roundtrip(Request::StartSession { cmd, env })
    }

    pub fn cancel_session(&mut self) -> Result<()> {
        let _ = self.roundtrip(Request::CancelSession)?;
        Ok(())
    }
}
