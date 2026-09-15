//! Direct KMS/DRM display backend.
//!
//! This intentionally does not use a Wayland or X11 compositor: ferum-greet
//! opens a `/dev/dri/cardN` node itself, picks a connected connector/CRTC and
//! a mode, and scans out double-buffered "dumb buffers" that we fill with
//! pixels rendered off-screen by wgpu (see `gpu.rs`). Presentation is a
//! CPU blit into the dumb buffer followed by a page flip - simple and works
//! on every KMS driver (including virtio-gpu in a VM), at the cost of an
//! extra memcpy per frame, which is irrelevant for a login screen that only
//! redraws on input.

use std::fs::{File, OpenOptions};
use std::os::unix::io::{AsFd, BorrowedFd};
use std::path::Path;

use anyhow::{bail, Context, Result};
use drm::buffer::{Buffer as _, DrmFourcc};
use drm::control::{connector, crtc, dumbbuffer::DumbBuffer, framebuffer, Device as ControlDevice, Event, ModeTypeFlags, PageFlipFlags};
use drm::Device as BasicDevice;

pub struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl BasicDevice for Card {}
impl ControlDevice for Card {}

struct Buf {
    dumb: DumbBuffer,
    fb: framebuffer::Handle,
}

pub struct DrmBackend {
    card: Card,
    crtc: crtc::Handle,
    connector: connector::Handle,
    mode: drm::control::Mode,
    width: u32,
    height: u32,
    /// Byte stride of the buffers we hand back to callers to fill.
    pitch: u32,
    buffers: [Buf; 2],
    front: usize,
    first_present: bool,
}

impl DrmBackend {
    /// Opens `device` (or probes `/dev/dri/card0..15` if `device` is "auto"),
    /// picks the first connected connector and its preferred mode, and sets
    /// up double-buffered dumb buffers for scanout.
    pub fn open(device: &str) -> Result<Self> {
        let card = if device == "auto" {
            Self::open_first_connected()?
        } else {
            Self::open_path(Path::new(device))?
        };
        Self::from_card(card)
    }

    fn open_path(path: &Path) -> Result<Card> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("opening DRM device {}", path.display()))?;
        Ok(Card(file))
    }

    fn open_first_connected() -> Result<Card> {
        for idx in 0..16 {
            let path = format!("/dev/dri/card{idx}");
            let Ok(card) = Self::open_path(Path::new(&path)) else {
                continue;
            };
            let Ok(res) = card.resource_handles() else {
                continue;
            };
            let has_connected = res
                .connectors()
                .iter()
                .filter_map(|c| card.get_connector(*c, false).ok())
                .any(|c| c.state() == connector::State::Connected);
            if has_connected {
                log::info!("using DRM device {path}");
                return Ok(card);
            }
        }
        bail!("no DRM device with a connected connector found");
    }

    fn from_card(card: Card) -> Result<Self> {
        let res = card
            .resource_handles()
            .context("getting DRM resource handles")?;

        let connector_info = res
            .connectors()
            .iter()
            .filter_map(|c| card.get_connector(*c, true).ok())
            .find(|c| c.state() == connector::State::Connected)
            .context("no connected connector")?;

        let mode = *connector_info
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| connector_info.modes().first())
            .context("connector has no modes")?;

        let (width, height) = mode.size();
        let (width, height) = (width as u32, height as u32);

        let crtc_handle = Self::find_crtc(&card, &res, &connector_info)?;

        let fmt = DrmFourcc::Xrgb8888;
        let make_buf = || -> Result<Buf> {
            let dumb = card
                .create_dumb_buffer((width, height), fmt, 32)
                .context("creating dumb buffer")?;
            let fb = card
                .add_framebuffer(&dumb, 24, 32)
                .context("creating framebuffer")?;
            Ok(Buf { dumb, fb })
        };
        let buffers = [make_buf()?, make_buf()?];
        let pitch = buffers[0].dumb.pitch();

        card.set_crtc(
            crtc_handle,
            Some(buffers[0].fb),
            (0, 0),
            &[connector_info.handle()],
            Some(mode),
        )
        .context("setting CRTC mode")?;

        Ok(DrmBackend {
            card,
            crtc: crtc_handle,
            connector: connector_info.handle(),
            mode,
            width,
            height,
            pitch,
            buffers,
            front: 0,
            first_present: true,
        })
    }

    fn find_crtc(
        card: &Card,
        res: &drm::control::ResourceHandles,
        connector_info: &connector::Info,
    ) -> Result<crtc::Handle> {
        // Prefer the CRTC currently driving this connector's encoder, if any.
        if let Some(enc) = connector_info.current_encoder() {
            if let Ok(enc_info) = card.get_encoder(enc) {
                if let Some(crtc) = enc_info.crtc() {
                    return Ok(crtc);
                }
            }
        }
        // Otherwise, take any CRTC that's compatible with one of the
        // connector's encoders.
        for enc_handle in connector_info.encoders() {
            let Ok(enc_info) = card.get_encoder(*enc_handle) else {
                continue;
            };
            if let Some(crtc) = res.filter_crtcs(enc_info.possible_crtcs()).first() {
                return Ok(*crtc);
            }
        }
        bail!("no usable CRTC for connector")
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn refresh_hz(&self) -> f32 {
        let v = self.mode.vrefresh();
        if v == 0 {
            60.0
        } else {
            v as f32
        }
    }

    /// Copies `pixels` (tightly matching `stride() * height()` rows, format
    /// BGRX8888 matching `DrmFourcc::Xrgb8888`) into the back buffer and
    /// flips to it, blocking until the flip has completed.
    pub fn present(&mut self, pixels: &[u8], src_stride: u32) -> Result<()> {
        let back = 1 - self.front;
        {
            let mut map = self
                .card
                .map_dumb_buffer(&mut self.buffers[back].dumb)
                .context("mapping dumb buffer")?;
            let dst_stride = self.pitch as usize;
            let src_stride = src_stride as usize;
            let row_bytes = dst_stride.min(src_stride);
            for y in 0..self.height as usize {
                let src = &pixels[y * src_stride..y * src_stride + row_bytes];
                let dst = &mut map[y * dst_stride..y * dst_stride + row_bytes];
                dst.copy_from_slice(src);
            }
        }

        let fb = self.buffers[back].fb;
        if self.first_present {
            // The very first buffer was already set via set_crtc; nothing
            // has been page-flipped away from yet, so a flip has nothing to
            // wait on for the *other* buffer. Just set_crtc again onto the
            // newly-filled buffer.
            self.card
                .set_crtc(
                    self.crtc,
                    Some(fb),
                    (0, 0),
                    &[self.connector],
                    Some(self.mode),
                )
                .context("setting CRTC to new framebuffer")?;
            self.first_present = false;
        } else {
            self.card
                .page_flip(self.crtc, fb, PageFlipFlags::EVENT, None)
                .context("queueing page flip")?;
            self.wait_for_flip()?;
        }

        self.front = back;
        Ok(())
    }

    fn wait_for_flip(&self) -> Result<()> {
        loop {
            let events = self.card.receive_events().context("reading DRM events")?;
            for event in events {
                if let Event::PageFlip(_) = event {
                    return Ok(());
                }
            }
        }
    }
}

impl Drop for DrmBackend {
    fn drop(&mut self) {
        // The kernel removes framebuffers and dumb buffers owned by this
        // file descriptor when it is closed, so explicit teardown here is
        // only a courtesy for drivers that log warnings otherwise.
        for buf in &self.buffers {
            let _ = self.card.destroy_framebuffer(buf.fb);
        }
    }
}
