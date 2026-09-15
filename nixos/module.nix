# NixOS module for ferum-greet: wires the greeter up as the `greetd` session,
# grants it the device permissions it needs (no seatd/logind integration -
# see the README), and renders /etc/ferum-greet/config.toml from options.
{ ferum-greet }:
{ config, lib, pkgs, ... }:
let
  cfg = config.services.ferum-greet;

  sessionSubmodule = lib.types.submodule {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        description = "Human-readable name shown in the greeter's session picker.";
        example = "GNOME";
      };
      exec = lib.mkOption {
        type = lib.types.str;
        description = "Command line greetd runs to start this session.";
        example = "gnome-session";
      };
      env = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [ "RUST_LOG=debug" ];
        description = ''
          Extra `"KEY=VALUE"` environment variables greetd sets for the
          session process - handy for a debug variant of an existing
          session (e.g. a second entry with the same `exec` and
          `RUST_LOG=debug`/`WAYLAND_DEBUG=1` here).
        '';
      };
    };
  };

  backgroundSubmodule = lib.types.submodule {
    options = {
      kind = lib.mkOption {
        type = lib.types.enum [ "default" "color" "image" ];
        default = "default";
        description = ''
          `"default"` uses the bundled nature wallpaper, `"color"` fills the
          screen with `color`, and `"image"` loads `path` (PNG, JPEG or SVG).
        '';
      };
      path = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "Image file to use when kind is \"image\".";
      };
      color = lib.mkOption {
        type = lib.types.nullOr (lib.types.submodule {
          options = {
            r = lib.mkOption { type = lib.types.ints.u8; };
            g = lib.mkOption { type = lib.types.ints.u8; };
            b = lib.mkOption { type = lib.types.ints.u8; };
          };
        });
        default = null;
        description = "RGB color to use when kind is \"color\".";
      };
    };
  };

  backgroundToml =
    { kind = cfg.background.kind; }
    // lib.optionalAttrs (cfg.background.kind == "image") { path = cfg.background.path; }
    // lib.optionalAttrs (cfg.background.kind == "color" && cfg.background.color != null) {
      inherit (cfg.background.color) r g b;
    };

  settingsFormat = pkgs.formats.toml { };

  configToml = settingsFormat.generate "ferum-greet-config.toml" ({
    drm_device = cfg.drmDevice;
    remember_last_user = cfg.rememberLastUser;
    state_dir = cfg.stateDir;
    sessions = map (s: { inherit (s) name exec env; }) cfg.sessions;
    background = backgroundToml;
    theme = {
      accent_color = cfg.theme.accentColor;
      font_family = cfg.theme.fontFamily;
      font_size = cfg.theme.fontSize;
    };
  } // lib.optionalAttrs (cfg.defaultSession != null) { default_session = cfg.defaultSession; });
in
{
  options.services.ferum-greet = {
    enable = lib.mkEnableOption "the ferum-greet GPU-rendered greetd greeter";

    package = lib.mkOption {
      type = lib.types.package;
      default = ferum-greet;
      description = "The ferum-greet package to run.";
    };

    drmDevice = lib.mkOption {
      type = lib.types.str;
      default = "auto";
      description = ''
        DRM device node to render to (e.g. `/dev/dri/card0`). `"auto"`
        picks the first device with a connected display.
      '';
    };

    rememberLastUser = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Pre-fill the username field with the last user who logged in successfully.";
    };

    stateDir = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/ferum-greet";
      description = "Where the last-logged-in username is persisted.";
    };

    background = lib.mkOption {
      type = backgroundSubmodule;
      default = { };
      description = "The greeter's background.";
    };

    sessions = lib.mkOption {
      type = lib.types.listOf sessionSubmodule;
      default = [{ name = "Default shell"; exec = "/bin/sh -l"; }];
      example = lib.literalExpression ''
        [
          { name = "GNOME"; exec = "gnome-session"; }
          { name = "Plasma"; exec = "startplasma-wayland"; }
          { name = "Sway"; exec = "sway"; }
        ]
      '';
      description = ''
        The desktop environments/window managers offered in the session
        picker. Cycle through them in the greeter with the arrow keys.
      '';
    };

    defaultSession = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Name of the `sessions` entry preselected at startup. Defaults to the first entry.";
    };

    theme = {
      accentColor = lib.mkOption {
        type = lib.types.listOf lib.types.ints.u8;
        default = [ 90 150 240 ];
        description = "RGB accent color used for focused fields and highlights.";
      };
      fontFamily = lib.mkOption {
        type = lib.types.str;
        default = "sans-serif";
        description = "Font family used for all greeter text.";
      };
      fontSize = lib.mkOption {
        type = lib.types.float;
        default = 20.0;
        description = "Base font size, in pixels.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    # ferum-greet needs a working Vulkan or GLES/EGL userland to render at
    # all (it has no software-only fallback); this also provides a
    # software Vulkan device (llvmpipe/lavapipe) when there is no real GPU.
    hardware.graphics.enable = lib.mkDefault true;

    services.greetd = {
      enable = true;
      settings.default_session = {
        command = "${cfg.package}/bin/ferum-greet --config ${configToml}";
        user = "greeter";
      };
    };

    # greetd's NixOS module creates the "greeter" user; it needs to be able
    # to open /dev/dri/* and /dev/input/* directly since ferum-greet talks
    # to KMS and evdev without a seat daemon.
    users.users.greeter.extraGroups = [ "video" "input" ];

    systemd.tmpfiles.rules = [
      "d ${cfg.stateDir} 0700 greeter greeter - -"
    ];
  };
}
