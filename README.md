# ferum-greet

A GPU-rendered login manager greeter for [greetd](https://git.sr.ht/~kennylevinsen/greetd),
written in Rust. It does **not** use Wayland or X11: it talks to
`/dev/dri/cardN` (KMS/DRM) directly for display setup and scanout, renders
the UI with [`wgpu`](https://github.com/gfx-rs/wgpu), and reads the keyboard
straight from `evdev`.

## How it renders without a compositor

There's no windowing system involved anywhere:

- **`drm`** drives every connected connector on the chosen card, each with
  its own CRTC, mode, and pair of double-buffered "dumb buffers" for
  scanout (legacy KMS + page flips) - plug in several monitors and the same
  UI is mirrored on all of them.
- **`wgpu`** renders the frame off-screen into a plain texture (a headless
  `wgpu::Instance`/`Device`, requested without any surface) using Vulkan or
  GLES, whichever is available - real GPU driver or a software one
  (llvmpipe/lavapipe). Each output gets its own render target sized to its
  own mode, since monitors can differ in resolution.
- Each frame is copied from that texture back to the CPU and memcpy'd into
  the DRM buffer, then page-flipped onto the screen. This costs one extra
  copy per frame per output, which is irrelevant for a login screen that
  only redraws on keypresses.
- **`glyphon`**/**`cosmic-text`** shape and rasterize all UI text into the
  same wgpu frame.
- **`image`**/**`resvg`**/**`tiny-skia`** decode the background (JPEG/PNG or
  SVG) and scale it to cover the screen.
- **`evdev`** is read directly (no libinput/seatd) for keyboard input; the
  greeter process needs to be in the `input` and `video` groups, which the
  NixOS module sets up for you.
- **`greetd_ipc`** drives the actual authentication handshake and session
  launch over `$GREETD_SOCK`.

greetd runs the greeter attached to its VT, the same way it would a TUI
greeter, so ferum-greet's stderr ends up on that VT rather than in the
systemd journal. It logs to `<state_dir>/ferum-greet.log` (by default
`/var/lib/ferum-greet/ferum-greet.log`) instead - check there, not
`journalctl -u greetd`, when something goes wrong.

## Configuration

ferum-greet reads `/etc/ferum-greet/config.toml` (override with
`--config <path>`). See [`src/config.rs`](src/config.rs) for the full
schema; the important bits:

```toml
remember_last_user = true
default_session = "Sway"

[background]
kind = "image"       # "default" | "color" | "image"
path = "/etc/ferum-greet/wallpaper.jpg"

[[sessions]]
name = "Sway"
exec = "sway"

[[sessions]]
name = "GNOME"
exec = "gnome-session"

[theme]
accent_color = [90, 150, 240]
font_size = 20.0
```

The default background (`kind = "default"`) is a nature photo baked into
the Nix package at build time from a hash-pinned URL (see `flake.nix`).

## NixOS module

```nix
{
  inputs.ferum-greet.url = "github:you/ferum-greet";

  outputs = { self, nixpkgs, ferum-greet, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        ferum-greet.nixosModules.default
        {
          services.ferum-greet = {
            enable = true;
            rememberLastUser = true;
            sessions = [
              { name = "Sway"; exec = "sway"; }
              { name = "GNOME"; exec = "gnome-session"; }
              { name = "Plasma"; exec = "startplasma-wayland"; }
            ];
            background.kind = "default";
          };
        }
      ];
    };
  };
}
```

This enables `services.greetd` with ferum-greet as the greeter, adds the
`greeter` user to the `video`/`input` groups it needs, turns on
`hardware.graphics` (required - there is no software-only text fallback),
and renders the config file for you. See `nixos/module.nix` for every
option (`drmDevice`, `stateDir`, `theme.*`, ...).

## Testing

`nixos/vm-test.nix` boots a NixOS VM (via `pkgs.testers.runNixOSTest`) with
ferum-greet as its greeter and checks that KMS mode-setting succeeds, a GPU
adapter (real or the software lavapipe one) is found, and greetd doesn't
crash-loop.

```console
$ ./scripts/test-vm.sh              # same as `nix flake check`
$ ./scripts/test-vm.sh --interactive  # watch it in a real QEMU window
```

or directly:

```console
$ nix flake check
$ nix run .#run-vm   # login: demo / demo
```

## Building / development

```console
$ nix develop     # or: direnv allow
$ cargo build
$ cargo run -- --config ./dev-config.toml   # run outside greetd for quick iteration
                                             # (needs a real /dev/dri device and evdev access)
```

Running outside of greetd won't have `$GREETD_SOCK` set, so login attempts
will fail with a clear error - useful for checking that rendering and input
work before wiring it into greetd.

## Known limitations

- Keyboard layout is a fixed US-QWERTY table (`src/keymap.rs`); no XKB.
- No mouse/touch support (the crate list mentions `evdev` for "keyboard,
  mouse, touch", but a greeter only needs a keyboard to type a username and
  password, so only that is implemented).
- Hotplugged keyboards after startup aren't picked up.
