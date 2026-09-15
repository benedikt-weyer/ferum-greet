#!/usr/bin/env bash
# Lists the commands available for working on ferum-greet.
set -euo pipefail

cat <<'EOF'
ferum-greet - available commands
=================================

Development
  nix develop                          Enter a shell with the Rust toolchain
                                        and all native deps (or: direnv allow)
  cargo build                          Build the greeter
  cargo check                          Type-check without a full build
  cargo run -- --config <path>         Run outside greetd for quick iteration
                                        (needs a real /dev/dri device and
                                        evdev access; $GREETD_SOCK unset means
                                        login attempts fail loudly - expected)

Building the Nix package / module
  nix build .#default                  Build the ferum-greet package
  nix flake check                      Evaluate + build everything (package,
                                        NixOS module, devShell, the VM test)

Testing
  scripts/test-vm.sh                   Run the automated VM test (headless,
                                        screenshot-based; same as the vmTest
                                        part of `nix flake check`)
  scripts/test-vm.sh --interactive     Boot the same VM in a real QEMU GUI
                                        window you can use (login: demo/demo)
  nix build .#checks.<system>.vmTest   Same as the first, spelled out
  nix run .#run-vm                     Same as --interactive, spelled out

Other
  scripts/help.sh                      This list
EOF
