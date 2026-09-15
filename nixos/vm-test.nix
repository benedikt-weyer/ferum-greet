# A NixOS VM test that boots a machine with ferum-greet configured as the
# greetd greeter and checks that it actually comes up: DRM/KMS mode-setting
# succeeds, a GPU (real or the software lavapipe/llvmpipe one) is found, and
# the greetd session stays running rather than crash-looping.
{ pkgs, ferum-greet-module }:

pkgs.testers.runNixOSTest {
  name = "ferum-greet";

  nodes.machine = { config, pkgs, ... }: {
    imports = [ ferum-greet-module ];

    services.ferum-greet = {
      enable = true;
      rememberLastUser = true;
      background.kind = "color";
      background.color = { r = 20; g = 24; b = 32; };
      sessions = [
        { name = "Test shell"; exec = "/run/current-system/sw/bin/bash -l"; }
      ];
    };

    users.users.alice = {
      isNormalUser = true;
      password = "correct-horse-battery-staple";
    };

    # The default test machine already runs with a virtio-gpu display
    # (`virtualisation.graphics`, on by default), which is enough for a
    # `/dev/dri/cardN` KMS dumb-buffer device to show up even without 3D
    # acceleration.
    virtualisation.graphics = true;
    virtualisation.memorySize = 2048;
  };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    with subtest("a DRM device is present"):
        machine.wait_until_succeeds("test -e /dev/dri/card0")

    with subtest("greetd is running ferum-greet"):
        machine.wait_for_unit("greetd.service")
        machine.wait_until_succeeds("pgrep -u greeter -f ferum-greet")

    with subtest("ferum-greet found a display mode and a GPU adapter"):
        # greetd runs the greeter attached to its VT (like it would a TUI
        # greeter), so its stderr never reaches the journal - it logs to
        # its own file instead (see src/main.rs).
        log_file = "/var/lib/ferum-greet/ferum-greet.log"
        machine.wait_until_succeeds(f"test -e {log_file}")
        machine.wait_until_succeeds(f"grep -q 'display 0:' {log_file}")
        machine.wait_until_succeeds(f"grep -q 'using GPU adapter' {log_file}")

    with subtest("greetd did not crash-loop"):
        machine.sleep(5)
        machine.succeed("systemctl is-active greetd.service")
        machine.succeed("pgrep -u greeter -f ferum-greet")

    machine.screenshot("greeter")
  '';
}
