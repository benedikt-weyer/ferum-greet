{
  description = "ferum-greet development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ rust-overlay.overlays.default ];
        pkgs = import nixpkgs { inherit system overlays; };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustToolchain
            pkg-config

            # Common graphics / windowing deps useful for a GPU-based GUI
            libGL
            libxkbcommon
            wayland
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
          ];

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
            pkgs.libGL
            pkgs.libxkbcommon
            pkgs.wayland
            pkgs.xorg.libX11
            pkgs.xorg.libXcursor
            pkgs.xorg.libXi
            pkgs.xorg.libXrandr
          ];

          shellHook = ''
            export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library"
          '';
        };
      });
}
