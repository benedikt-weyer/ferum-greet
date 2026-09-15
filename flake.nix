{
  description = "ferum-greet: a GPU-rendered (DRM/KMS + wgpu) login manager greeter for greetd";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    let
      # A small, permanently-addressable nature photo (Wikimedia Commons),
      # fetched at build time and baked in as the default wallpaper. Pinned
      # by hash like any other Nix fixed-output derivation, so the build
      # stays reproducible even though the source is a live URL.
      defaultWallpaper = pkgs: pkgs.fetchurl {
        url = "https://upload.wikimedia.org/wikipedia/commons/thumb/1/14/Landscape_Arnisee-region.JPG/1920px-Landscape_Arnisee-region.JPG";
        hash = "sha256-amPBIdpS824hKPAafc3Lc+4Mx6kNghKhLNNo1Br2OWg=";
      };

      mkFerumGreet = pkgs:
        let
          wallpaper = defaultWallpaper pkgs;
          runtimeLibs = with pkgs; [
            vulkan-loader
            libgbm
            libGL
          ];
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "ferum-greet";
          version = "0.1.0";
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              baseNameOf path != "target";
          };

          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.makeWrapper ];

          # Consumed by `option_env!("FERUM_GREET_DEFAULT_WALLPAPER")` in
          # src/config.rs at compile time.
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
        };

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

        ferum-greet = mkFerumGreet pkgs;
      in
      {
        packages.default = ferum-greet;
        packages.ferum-greet = ferum-greet;

        apps.default = {
          type = "app";
          program = "${ferum-greet}/bin/ferum-greet";
        };

        # `nix run .#run-vm` boots an interactive graphical VM of a plain
        # NixOS system with ferum-greet as its greetd greeter, for manual
        # testing without going through the full `nix flake check` test.
        apps.run-vm =
          let
            vm = (nixpkgs.lib.nixosSystem {
              inherit system;
              modules = [
                self.nixosModules.default
                {
                  services.ferum-greet.enable = true;
                  users.users.demo = {
                    isNormalUser = true;
                    initialPassword = "demo";
                  };
                  virtualisation.vmVariant.virtualisation.memorySize = 2048;
                  system.stateVersion = "24.11";
                }
              ];
            }).config.system.build.vm;
          in
          {
            type = "app";
            program = "${vm}/bin/run-*-vm";
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
