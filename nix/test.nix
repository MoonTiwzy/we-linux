# A NixOS VM with a headless compositor: the tools run and the window opens.
# (Rendering a wallpaper needs Wallpaper Engine's own assets, which are not
# ours to ship: that is checked by hand.)
self:
{ ... }:
{
  name = "we-linux";

  nodes.machine =
    { ... }:
    {
      imports = [ self.nixosModules.default ];
      programs.we-linux = {
        enable = true;
        autostart = false;
      };
      programs.sway.enable = true;
      hardware.graphics.enable = true;
      users.users.alice.isNormalUser = true;
      virtualisation.memorySize = 2048;
    };

  testScript = ''
    session = "XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-1"
    headless = "-E WLR_BACKENDS=headless -E WLR_RENDERER=pixman -E WLR_LIBINPUT_NO_DEVICES=1"

    machine.wait_for_unit("multi-user.target")
    machine.succeed("we-linux --version")
    machine.succeed("we-linux --help | grep -q daemon")
    # The engine comes along on the wrapper's PATH.
    engine = machine.succeed("grep -o '/nix/store/[^:\" ]*wepaper[^:\" ]*/bin' $(readlink -f $(command -v we-linux)) | head -1").strip()
    for tool in ["wepaper"]:
        machine.succeed(f"{engine}/{tool} --help >/dev/null")

    machine.succeed("loginctl enable-linger alice")
    machine.wait_for_file("/run/user/1000")
    # The user's own manager and bus first: a compositor started before them
    # sometimes never comes up, and the test then waited a quarter of an hour.
    machine.wait_for_unit("user@1000.service")
    machine.wait_for_file("/run/user/1000/bus")

    def compositor():
        machine.succeed(f"systemd-run --uid=alice -E XDG_RUNTIME_DIR=/run/user/1000 {headless} --unit=compositor sway")
        try:
            machine.wait_until_succeeds("test -S /run/user/1000/wayland-1", timeout=60)
            return True
        except Exception:
            # Why it did not come up, for whoever reads the log.
            print(machine.execute("journalctl -u compositor --no-pager | tail -40")[1])
            machine.execute("systemctl stop compositor; systemctl reset-failed compositor")
            return False

    assert any(compositor() for _ in range(3)), "sway did not start in three tries"

    machine.succeed("systemd-run --uid=alice -E XDG_RUNTIME_DIR=/run/user/1000 -E WAYLAND_DISPLAY=wayland-1 --unit=window we-linux")
    tree = f"su alice -c '{session} swaymsg -s /run/user/1000/sway-ipc.*.sock -t get_tree'"
    machine.wait_until_succeeds(f"{tree} | grep -q '\"app_id\": \"we-linux\"'", timeout=90)
    machine.succeed(f"su alice -c '{session} we-linux status' | grep -q 'wallpaper: none'")
  '';
}
