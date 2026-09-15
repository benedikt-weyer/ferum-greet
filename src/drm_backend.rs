//! Direct KMS/DRM display backend.
//!
//! This intentionally does not use a Wayland or X11 compositor: ferum-greet
//! opens `/dev/dri/cardN` nodes itself - one or several, spanning multiple
//! GPUs - and drives every connected connector it finds on each with its
//! own CRTC and mode, scanning out double-buffered "dumb buffers" per
//! output that we fill with pixels rendered off-screen by wgpu (see
//! `gpu.rs`). Presentation is a CPU blit into the dumb buffer followed by a
//! page flip - simple and works on every KMS driver (including virtio-gpu
//! in a VM), at the cost of an extra memcpy per frame, which is irrelevant
//! for a login screen that only redraws on input.

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

/// One connected display: its own CRTC, mode, and double-buffered scanout
/// buffers, driven independently of every other output - whether that's
/// another connector on the same card or one on a different GPU entirely.
struct Output {
    /// Index into `DrmBackend::cards` of the card driving this output.
    card: usize,
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

pub struct DrmBackend {
    cards: Vec<Card>,
    outputs: Vec<Output>,
}

impl DrmBackend {
    /// Opens `device` (or, if `device` is "auto", every `/dev/dri/card0..15`
    /// that has at least one connected connector - spanning multiple GPUs),
    /// then drives every connected connector on each card, each with its own
    /// CRTC and preferred mode, with double-buffered dumb buffers set up for
    /// scanout.
    pub fn open(device: &str) -> Result<Self> {
        let cards = if device == "auto" {
            Self::open_all_connected()?
        } else {
            vec![(device.to_string(), Self::open_path(Path::new(device))?)]
        };
        Self::from_cards(cards)
    }

    fn open_path(path: &Path) -> Result<Card> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("opening DRM device {}", path.display()))?;
        Ok(Card(file))
    }

    /// Probes every `/dev/dri/cardN` node and returns all of them that have
    /// at least one connected connector - this is what lets "auto" span
    /// several GPUs at once instead of picking just the first one found.
    fn open_all_connected() -> Result<Vec<(String, Card)>> {
        let mut found = Vec::new();
        let mut seen = Vec::new();
        for idx in 0..16 {
            let path = format!("/dev/dri/card{idx}");
            let Ok(card) = Self::open_path(Path::new(&path)) else {
                continue;
            };
            let Ok(res) = card.resource_handles() else {
                continue;
            };
            let connectors: Vec<_> = res
                .connectors()
                .iter()
                .filter_map(|c| card.get_connector(*c, true).ok())
                .collect();
            let has_connected = connectors
                .iter()
                .any(|c| c.state() == connector::State::Connected);
            if has_connected {
                log::info!("using DRM device {path}");
                found.push((path, card));
            } else {
                seen.push((
                    path,
                    connectors.iter().map(|c| format!("{:?}", c.state())).collect::<Vec<_>>(),
                ));
            }
        }
        if found.is_empty() {
            for (path, states) in &seen {
                log::warn!("{path}: no connected connector (saw {states:?})");
            }
            bail!("no DRM device with a connected connector found");
        }
        Ok(found)
    }

    fn from_cards(named_cards: Vec<(String, Card)>) -> Result<Self> {
        let mut cards = Vec::with_capacity(named_cards.len());
        let mut outputs = Vec::new();

        for (card_idx, (path, card)) in named_cards.into_iter().enumerate() {
            let res = card
                .resource_handles()
                .with_context(|| format!("getting DRM resource handles for {path}"))?;

            let mut connected: Vec<connector::Info> = res
                .connectors()
                .iter()
                .filter_map(|c| card.get_connector(*c, true).ok())
                .filter(|c| c.state() == connector::State::Connected)
                .collect();
            // Deterministic order so logs/behavior don't depend on kernel
            // enumeration order.
            connected.sort_by_key(|c| (format!("{:?}", c.interface()), c.interface_id()));

            let mut used_crtcs = Vec::new();
            for connector_info in &connected {
                let crtc_handle = match Self::find_crtc(&card, &res, connector_info, &used_crtcs) {
                    Ok(crtc) => crtc,
                    Err(err) => {
                        log::warn!(
                            "{path}: skipping connector {:?}{}: {err}",
                            connector_info.interface(),
                            connector_info.interface_id()
                        );
                        continue;
                    }
                };
                used_crtcs.push(crtc_handle);

                let mode = *connector_info
                    .modes()
                    .iter()
                    .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
                    .or_else(|| connector_info.modes().first())
                    .context("connector has no modes")?;
                let (width, height) = mode.size();
                let (width, height) = (width as u32, height as u32);

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

                log::info!(
                    "output {path}/{:?}{}: {width}x{height} @ {}Hz",
                    connector_info.interface(),
                    connector_info.interface_id(),
                    mode.vrefresh()
                );

                outputs.push(Output {
                    card: card_idx,
                    crtc: crtc_handle,
                    connector: connector_info.handle(),
                    mode,
                    width,
                    height,
                    pitch,
                    buffers,
                    front: 0,
                    first_present: true,
                });
            }

            cards.push(card);
        }

        if outputs.is_empty() {
            bail!("no connected connector could be driven (no usable CRTC)");
        }

        Ok(DrmBackend { cards, outputs })
    }

    fn find_crtc(
        card: &Card,
        res: &drm::control::ResourceHandles,
        connector_info: &connector::Info,
        used: &[crtc::Handle],
    ) -> Result<crtc::Handle> {
        // Prefer the CRTC currently driving this connector's encoder, if any
        // and if it isn't already claimed by an output we set up earlier.
        if let Some(enc) = connector_info.current_encoder()
            && let Ok(enc_info) = card.get_encoder(enc)
            && let Some(crtc) = enc_info.crtc()
            && !used.contains(&crtc)
        {
            return Ok(crtc);
        }
        // Otherwise, take any free CRTC that's compatible with one of the
        // connector's encoders.
        for enc_handle in connector_info.encoders() {
            let Ok(enc_info) = card.get_encoder(*enc_handle) else {
                continue;
            };
            if let Some(crtc) = res
                .filter_crtcs(enc_info.possible_crtcs())
                .into_iter()
                .find(|c| !used.contains(c))
            {
                return Ok(crtc);
            }
        }
        bail!("no free CRTC for connector")
    }

    /// Number of connected displays being driven, across every card.
    pub fn output_count(&self) -> usize {
        self.outputs.len()
    }

    pub fn width(&self, output: usize) -> u32 {
        self.outputs[output].width
    }

    pub fn height(&self, output: usize) -> u32 {
        self.outputs[output].height
    }

    pub fn refresh_hz(&self, output: usize) -> f32 {
        let v = self.outputs[output].mode.vrefresh();
        if v == 0 {
            60.0
        } else {
            v as f32
        }
    }

    /// Copies `pixels` (tightly matching `stride() * height()` rows, format
    /// BGRX8888 matching `DrmFourcc::Xrgb8888`) into `output`'s back buffer
    /// and flips to it, blocking until the flip has completed.
    pub fn present(&mut self, output: usize, pixels: &[u8], src_stride: u32) -> Result<()> {
        let card = &self.cards[self.outputs[output].card];
        let out = &mut self.outputs[output];
        let back = 1 - out.front;
        {
            let mut map = card
                .map_dumb_buffer(&mut out.buffers[back].dumb)
                .context("mapping dumb buffer")?;
            let dst_stride = out.pitch as usize;
            let src_stride = src_stride as usize;
            let row_bytes = dst_stride.min(src_stride);
            for y in 0..out.height as usize {
                let src = &pixels[y * src_stride..y * src_stride + row_bytes];
                let dst = &mut map[y * dst_stride..y * dst_stride + row_bytes];
                dst.copy_from_slice(src);
            }
        }

        let fb = out.buffers[back].fb;
        if out.first_present {
            // The very first buffer was already set via set_crtc; nothing
            // has been page-flipped away from yet, so a flip has nothing to
            // wait on for the *other* buffer. Just set_crtc again onto the
            // newly-filled buffer.
            card.set_crtc(out.crtc, Some(fb), (0, 0), &[out.connector], Some(out.mode))
                .context("setting CRTC to new framebuffer")?;
            out.first_present = false;
        } else {
            card.page_flip(out.crtc, fb, PageFlipFlags::EVENT, None)
                .context("queueing page flip")?;
            Self::wait_for_flip(card)?;
        }

        self.outputs[output].front = back;
        Ok(())
    }

    fn wait_for_flip(card: &Card) -> Result<()> {
        loop {
            let events = card.receive_events().context("reading DRM events")?;
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
        for out in &self.outputs {
            let card = &self.cards[out.card];
            for buf in &out.buffers {
                let _ = card.destroy_framebuffer(buf.fb);
            }
        }
    }
}
