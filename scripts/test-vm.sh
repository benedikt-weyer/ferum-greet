#!/usr/bin/env bash
# Runs the NixOS VM test defined in nixos/vm-test.nix: boots a virtual
# machine with ferum-greet configured as the greetd greeter and checks that
# it comes up correctly (DRM mode-set, GPU adapter found, greetd stays up).
#
# Usage:
#   scripts/test-vm.sh            # run the automated test (like `nix flake check`)
#   scripts/test-vm.sh --interactive
#       Build and boot the same VM interactively (graphical window) so you
#       can watch/use the greeter yourself. Log in as `demo` / `demo`.

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [[ "${1:-}" == "--interactive" || "${1:-}" == "-i" ]]; then
    echo "Building interactive VM (login: demo / demo)..."
    nix run .#run-vm
else
    echo "Running automated VM test..."
    nix build .#checks.$(nix eval --raw --impure --expr 'builtins.currentSystem').vmTest -L
    echo "OK - see ./result/ for the test log and screenshots."
fi
