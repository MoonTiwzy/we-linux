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
    engine = machine.succeed("grep -o '/nix/store/[^:\" ]*penguin-engine[^:\" ]*/bin' $(readlink -f $(command -v we-linux)) | head -1").strip()
    for tool in ["penguind", "penguinctl", "scene-viewer"]:
        machine.succeed(f"{engine}/{tool} --help >/dev/null")

    machine.succeed("loginctl enable-linger alice")
    machine.wait_for_file("/run/user/1000")
    machine.succeed(f"systemd-run --uid=alice -E XDG_RUNTIME_DIR=/run/user/1000 {headless} --unit=compositor sway")
    machine.wait_for_file("/run/user/1000/wayland-1")

    machine.succeed("systemd-run --uid=alice -E XDG_RUNTIME_DIR=/run/user/1000 -E WAYLAND_DISPLAY=wayland-1 --unit=window we-linux")
    tree = f"su alice -c '{session} swaymsg -s /run/user/1000/sway-ipc.*.sock -t get_tree'"
    machine.wait_until_succeeds(f"{tree} | grep -q '\"app_id\": \"we-linux\"'", timeout=90)
    machine.succeed(f"su alice -c '{session} we-linux status' | grep -q 'wallpaper: none'")
  '';
}
