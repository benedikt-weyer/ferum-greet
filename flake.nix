{
  description = "ferum-greet: a GPU-rendered (DRM/KMS + wgpu) login manager greeter for greetd";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, crane, flake-utils }:
    let
      # A small, permanently-addressable nature photo (Wikimedia Commons),
      # fetched at build time and baked in as the default wallpaper. Pinned
      # by hash like any other Nix fixed-output derivation, so the build
      # stays reproducible even though the source is a live URL.
      defaultWallpaper = pkgs: pkgs.fetchurl {
        url = "https://upload.wikimedia.org/wikipedia/commons/thumb/1/14/Landscape_Arnisee-region.JPG/1920px-Landscape_Arnisee-region.JPG";
        hash = "sha256-amPBIdpS824hKPAafc3Lc+4Mx6kNghKhLNNo1Br2OWg=";
      };

      # Built with crane rather than `rustPlatform.buildRustPackage` so that
      # dependency compilation (`cargoArtifacts`, everything in Cargo.lock)
      # is its own derivation, cached separately from ferum-greet's own
      # source - editing src/*.rs doesn't invalidate or rebuild the ~60
      # crates it depends on.
      mkFerumGreet = { pkgs, craneLib }:
        let
          wallpaper = defaultWallpaper pkgs;
          runtimeLibs = with pkgs; [
            vulkan-loader
            libgbm
            libGL
          ];

          commonArgs = {
            src = craneLib.cleanCargoSource ./.;
            strictDeps = true;
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        in
        craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "ferum-greet";
          version = "0.1.0";

          nativeBuildInputs = [ pkgs.makeWrapper ];

          # Consumed by `option_env!("FERUM_GREET_DEFAULT_WALLPAPER")` in
          # src/config.rs at compile time. Deliberately not part of
          # `commonArgs`/`cargoArtifacts`: it has nothing to do with
          # dependency compilation, and would invalidate that cache every
          # time the wallpaper URL/hash changes for no reason.
          FERUM_GREET_DEFAULT_WALLPAPER = "${wallpaper}";

          postFixup = ''
            wrapProgram $out/bin/ferum-greet \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath runtimeLibs}
          '';

          meta = with pkgs.lib; {
            description = "GPU-rendered (DRM/KMS + wgpu) login manager greeter for greetd";
            homepage = "https://github.com/ferum-greet/ferum-greet";
            license = licenses.agpl3Only;
            platforms = platforms.linux;
            mainProgram = "ferum-greet";
          };
        });

      ferumGreetModule = { config, lib, pkgs, ... }@args:
        import ./nixos/module.nix { ferum-greet = self.packages.${pkgs.system}.default; } args;
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ rust-overlay.overlays.default ];
        pkgs = import nixpkgs { inherit system overlays; };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain (_: rustToolchain);

        ferum-greet = mkFerumGreet { inherit pkgs craneLib; };
      in
      {
        packages.default = ferum-greet;
        packages.ferum-greet = ferum-greet;

        apps.default = {
          type = "app";
          program = "${ferum-greet}/bin/ferum-greet";
        };

        apps.help = {
          type = "app";
          program = "${pkgs.writeShellScript "ferum-greet-help" (builtins.readFile ./scripts/help.sh)}";
        };

        # `nix run .#run-vm` boots an interactive graphical VM of a plain
        # NixOS system with ferum-greet as its greetd greeter, for manual
        # testing without going through the full `nix flake check` test.
        apps.run-vm =
          let
            hostName = "ferum-greet";
            vm = (nixpkgs.lib.nixosSystem {
              inherit system;
              modules = [
                self.nixosModules.default
                {
                  networking.hostName = hostName;
                  services.ferum-greet.enable = true;
                  users.users.demo = {
                    isNormalUser = true;
                    initialPassword = "demo";
                  };
                  virtualisation.vmVariant.virtualisation = {
                    memorySize = 2048;
                    # Force a real GTK window with the VM's display, rather
                    # than relying on QEMU's default display heuristic.
                    graphics = true;
                    qemu.options = [ "-display" "gtk,show-cursor=on" ];
                    # All of QEMU's host<->guest directory sharing (the
                    # /nix/store mount and the default xchg/shared dirs)
                    # goes through a `virtiofsd` helper that needs
                    # privileges some sandboxed dev environments don't
                    # grant it (it fails with "Operation not permitted"
                    # opening the shared directory's root node). Baking the
                    # store into the VM's own disk image and dropping the
                    # (here unused) default shares sidesteps virtiofsd
                    # entirely, at the cost of a bigger disk image.
                    useNixStoreImage = true;
                    sharedDirectories = pkgs.lib.mkForce { };
                  };
                  system.stateVersion = "24.11";
                }
              ];
            }).config.system.build.vm;
          in
          {
            type = "app";
            program = "${vm}/bin/run-${hostName}-vm";
          };

        checks.vmTest = import ./nixos/vm-test.nix {
          inherit pkgs;
          ferum-greet-module = self.nixosModules.default;
        };
        checks.default = self.checks.${system}.vmTest;

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustToolchain
            pkg-config

            # DRM/KMS + GPU deps used directly by ferum-greet.
            libdrm
            vulkan-loader
            vulkan-headers
            vulkan-validation-layers
            mesa
            libGL
            libxkbcommon

            # Convenient for interactively poking at the VM test.
            qemu
          ];

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
            pkgs.libGL
            pkgs.vulkan-loader
            pkgs.libxkbcommon
          ];

          shellHook = ''
            export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library"
          '';
        };
      }) // {
      nixosModules.default = ferumGreetModule;
    };
}
