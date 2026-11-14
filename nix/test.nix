# A NixOS VM with a headless compositor: the tools run and the window opens.
# (Rendering a wallpaper needs Wallpaper Engine's own assets, which are not
# ours to ship: that is checked by hand.)
self:
{ pkgs, ... }:
{
  name = "we-linux";

  nodes.machine =
    { ... }:
    {
      environment.systemPackages = [
        pkgs.grim
        pkgs.imagemagick
      ];
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

    # A web wallpaper that plays sound, set for real: the renderer's web plugin
    # draws it, and the browser's process stays up (without GStreamer's plugins
    # in the package it ended as soon as the page played anything).
    machine.succeed("systemctl stop window")
    files = "/home/alice/.local/share/Steam/steamapps"
    machine.succeed(f"mkdir -p {files}/common/wallpaper_engine/assets /home/alice/web")
    machine.succeed("""cat > /home/alice/web/project.json <<'DOC'
    { "type": "web", "file": "index.html", "title": "A green page that hums" }
    DOC""")
    machine.succeed("""cat > /home/alice/web/index.html <<'DOC'
    <!doctype html>
    <html><body style="margin:0;background:#00ff00">
    <script>
      const sound = new AudioContext();
      const tone = sound.createOscillator();
      tone.connect(sound.destination);
      tone.start();
    </script>
    <audio autoplay loop src="data:audio/wav;base64,UklGRsQPAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YaAPAACAlKazuru0p5aBbVtNRURKVmh8kKOxuru2qpmFcV5PRkRIVGR4jaCvubu3rZyJdGFRR0RHUWF0iZytt7u5r6CNeGRUSERGT15xhZmqtru6saOQfGhWSkRFTVttgZantLu6s6aUf2tZTEVES1hpfpKksrq7tamXg29cTkVESVVmeo6hsLm7t6ubh3JfUEZESFJjdouerri8uK6ei3ZjUkhERlBfcoebq7e7ubChjnpmVUlERU5cb4OXqbW7urKkkn5pWEtERUxZa4CUprO6u7SnloFtW01FREpWaHyQo7G6u7aqmYVxXk9GREhUZHiNoK+5u7etnIl0YVFHREdRYXSJnK23u7mvoI14ZFRIREZPXnGFmaq2u7qxo5B8aFZKREVNW22Blqe0u7qzppSAa1lMRURLWGl+kqSyuru1qZeDb1xORURJVWZ6jqGwubu3q5uHcl9QRkRIUmN2i56uuLy4rp6LdmNSSERGUF9yh5urt7u5sKGOemZVSURFTlxvg5eptbu6sqSSfmlYS0RFTFlrgJSms7q7tKeWgW1bTUVESlZofJCjsbq7tqqZhXFeT0ZESFRkeI2gr7m7t62ciXRhUUdER1FhdImcrbe7ua+gjXhkVEhERk9ecYWZqra7urGjkHxoVkpERU1bbYGWp7S7urOmlH9rWUxFREtYaX6SpLK6u7Wpl4NvXE5FRElVZnqOobC5u7erm4dyX1BGREhSY3aLnq64vLiunot2Y1JIREZQX3KHm6u3u7mwoY56ZlVJREVOXG+Dl6m1u7qypJJ+aVhLREVMWWt/lKazuru0p5aBbVtNRURKVmh8kKOxuru2qpmFcV5PRkRIVGR4jaCvubu3rZyJdGFRR0RHUWF0iZytt7u5r6CNeGRUSERGT15xhZmqtru6saOQfGhWSkRFTVttgZantLu6s6aUf2tZTEVES1hpfpKksrq7tamXg29cTkVESVVmeo6hsLm7t6ubh3JfUEZESFJjdouerri8uK6ei3ZjUkhERlBfcoebq7e7ubChjnpmVUlERU5cb4OXqbW7urKkkn5pWEtERUxZa4CUprO6u7SnloFtW01FREpWaHyQo7G6u7aqmYVxXk9GREhUZHiNoK+5u7etnIl0YVFHREdRYXSJnK23u7mvoI14ZFRIREZPXnGFmaq2u7qxo5B8aFZKREVNW22Blqe0u7qzppR/a1lMRURLWGl+kqSyuru1qZeDb1xORURJVWZ6jqGwubu3q5uHcl9QRkRIUmN2i56uuLy4rp6LdmNSSERGUF9yh5urt7u5sKGOemZVSURFTlxvg5eptbu6sqSSfmlYS0RFTFlrgJSms7q7tKeWgW1bTUVESlZofJCjsbq7tqqZhXFeT0ZESFRkeI2gr7m7t62ciXRhUUdER1FhdImcrbe7ua+gjXhkVEhERk9ecYWZqra7urGjkHxoVkpERU1bbYGWp7S7urOmlH9rWUxFREtYaX6SpLK6u7Wpl4NvXE5FRElVZnqOobC5u7erm4dyX1BGREhSY3aLnq64vLiunot2Y1JIREZQX3KHm6u3u7mwoY56ZlVJREVOXG+Dl6m1u7qypJJ+aVhLREVMWWt/lKazuru0p5aBbVtNRURKVmh8kKOxuru2qpmFcV5PRkRIVGR4jaCvubu3rZyJdGFRR0RHUWF0iZytt7u5r6CNeGRUSERGT15xhZmqtru6saOQfGhWSkRFTVttgZantLu6s6aUgGtZTEVES1hpfpKksrq7tamXg29cTkVESVVmeo6hsLm7t6ubh3JfUEZESFJjdouerri8uK6ei3ZjUkhERlBfcoebq7e7ubChjnpmVUlERU5cb4OXqbW7urKkkn5pWEtERUxZa4CUprO6u7SnloFtW01FREpWaHyQo7G6u7aqmYVxXk9GREhUZHiNoK+5u7etnIl0YVFHREdRYXSJnK23u7mvoI14ZFRIREZPXnGFmaq2u7qxo5B8aFZKREVNW22Blqe0u7qzppR/a1lMRURLWGl+kqSyuru1qZeDb1xORURJVWZ6jqGwubu3q5uHcl9QRkRIUmN2i56uuLy4rp6LdmNSSERGUF9yh5urt7u5sKGOemZVSURFTlxvg5eptbu6sqSSfmlYS0RFTFlrgJSms7q7tKeWgW1bTUVESlZofJCjsbq7tqqZhXFeT0ZESFRkeI2gr7m7t62ciXRhUUdER1FhdImcrbe7ua+gjXhkVEhERk9ecYWZqra7urGjkHxoVkpERU1bbYGWp7S7urOmlH9rWUxFREtYaX6SpLK6u7Wpl4NvXE5FRElVZnqOobC5u7erm4dyX1BGREhSY3aLnq64vLiunot2Y1JIREZQX3KHm6u3u7mwoY56ZlVJREVOXG+Dl6m1u7qypJJ+aVhLREVMWWuAlKazuru0p5aBbVtNRURKVmh8kKOxuru2qpmFcV5PRkRIVGR4jaCvubu3rZyJdGFRR0RHUWF0iZytt7u5r6CNeGRUSERGT15xhZmqtru6saOQfGhWSkRFTVttgZantLu6s6aUf2tZTEVES1hpfpKksrq7tamXg29cTkVESVVmeo6hsLm7t6ubh3JfUEZESFJjdouerri8uK6ei3ZjUkhERlBfcoebq7e7ubChjnpmVUlERU5cb4OXqbW7urKkkn5pWEtERUxZa4CUprO6u7SnloFtW01FREpWaHyQo7G6u7aqmYVxXk9GREhUZHiNoK+5u7etnIl0YVFHREdRYXSJnK23u7mvoI14ZFRIREZPXnGFmaq2u7qxo5B8aFZKREVNW22Blqe0u7qzppR/a1lMRURLWGl+kqSyuru1qZeDb1xORURJVWZ6jqGwubu3q5uHcl9QRkRIUmN2i56uuLy4rp6LdmNSSERGUF9yh5urt7u5sKGOemZVSURFTlxvg5eptbu6sqSSfmlYS0RFTFlrgJSms7q7tKeWgW1bTUVESlZofJCjsbq7tqqZhXFeT0ZESFRkeI2gr7m7t62ciXRhUUdER1FhdImcrbe7ua+gjXhkVEhERk9ecYWZqra7urGjkHxoVkpERU1bbYGWp7S7urOmlH9rWUxFREtYaX6SpLK6u7Wpl4NvXE5FRElVZnqOobC5u7erm4dyX1BGREhSY3aLnq64vLiunot2Y1JIREZQX3KHm6u3u7mwoY56ZlVJREVOXG+Dl6m1u7qypJJ+aVhLREVMWWt/lKazuru0p5aBbVtNRURKVmh8kKOxuru2qpmFcV5PRkRIVGR4jaCvubu3rZyJdGFRR0RHUWF0iZytt7u5r6CNeGRUSERGT15xhZmqtru6saOQfGhWSkRFTVttgZantLu6s6aUf2tZTEVES1hpfpKksrq7tamXg29cTkVESVVmeo6hsLm7t6ubh3JfUEZESFJjdouerri8uK6ei3ZjUkhERlBfcoebq7e7ubChjnpmVUlERU5cb4OXqbW7urKkkn5pWEtERUxZa3+UprO6u7SnloFtW01FREpWaHyQo7G6u7aqmYVxXk9GREhUZHiNoK+5u7etnIl0YVFHREdRYXSJnK23u7mvoI14ZFRIREZPXnGFmaq2u7qxo5B8aFZKREVNW22Blqe0u7qzppSAa1lMRURLWGl+kqSyuru1qZeDb1xORURJVWZ6jqGwubu3q5uHcl9QRkRIUmN2i56uuLy4rp6LdmNSSERGUF9yh5urt7u5sKGOemZVSURFTlxvg5eptbu6sqSSfmlYS0RFTFlrgJSms7q7tKeWgW1bTUVESlZofJCjsbq7tqqZhXFeT0ZESFRkeI2gr7m7t62ciXRhUUdER1FhdImcrbe7ua+gjXhkVEhERk9ecYWZqra7urGjkHxoVkpERU1bbYGWp7S7urOmlIBrWUxFREtYaX6SpLK6u7Wpl4NvXE5FRElVZnqOobC5u7erm4dyX1BGREhSY3aLnq64vLiunot2Y1JIREZQX3KHm6u3u7mwoY56ZlVJREVOXG+Dl6m1u7qypJJ+aVhLREVMWWuAlKazuru0p5aBbVtNRURKVmh8kKOxuru2qpmFcV5PRkRIVGR4jaCvubu3rZyJdGFRR0RHUWF0iZytt7u5r6CNeGRUSERGT15xhZmqtru6saOQfGhWSkRFTVttgZantLu6s6aUf2tZTEVES1hpfpKksrq7tamXg29cTkVESVVmeo6hsLm7t6ubh3JfUEZESFJjdouerri8uK6ei3ZjUkhERlBfcoebq7e7ubChjnpmVUlERU5cb4OXqbW7urKkkn5pWEtERUxZa4CUprO6u7SnloFtW01FREpWaHyQo7G6u7aqmYVxXk9GREhUZHiNoK+5u7etnIl0YVFHREdRYXSJnK23u7mvoI14ZFRIREZPXnGFmaq2u7qxo5B8aFZKREVNW22Blqe0u7qzppR/a1lMRURLWGl+kqSyuru1qZeDb1xORURJVWZ6jqGwubu3q5uHcl9QRkRIUmN2i56uuLy4rp6LdmNSSERGUF9yh5urt7u5sKGOemZVSURFTlxvg5eptbu6sqSSfmlYS0RFTFlrgJSms7q7tKeWgW1bTUVESlZofJCjsbq7tqqZhXFeT0ZESFRkeI2gr7m7t62ciXRhUUdER1FhdImcrbe7ua+gjXhkVEhERk9ecYWZqra7urGjkHxoVkpERU1bbYGWp7S7urOmlIBrWUxFREtYaX6SpLK6u7Wpl4NvXE5FRElVZnqOobC5u7erm4dyX1BGREhSY3aLnq64vLiunot2Y1JIREZQX3KHm6u3u7mwoY56ZlVJREVOXG+Dl6m1u7qypJJ+aVhLREVMWWuAlKazuru0p5aBbVtNRURKVmh8kKOxuru2qpmFcV5PRkRIVGR4jaCvubu3rZyJdGFRR0RHUWF0iZytt7u5r6CNeGRUSERGT15xhZmqtru6saOQfGhWSkRFTVttgZantLu6s6aUf2tZTEVES1hpfpKksrq7tamXg29cTkVESVVmeo6hsLm7t6ubh3JfUEZESFJjdouerri8uK6ei3ZjUkhERlBfcoebq7e7ubChjnpmVUlERU5cb4OXqbW7urKkkn5pWEtERUxZa4CUprO6u7SnloFtW01FREpWaHyQo7G6u7aqmYVxXk9GREhUZHiNoK+5u7etnIl0YVFHREdRYXSJnK23u7mvoI14ZFRIREZPXnGFmaq2u7qxo5B8aFZKREVNW22Blqe0u7qzppR/a1lMRURLWGl+kqSyuru1qZeDb1xORURJVWZ6jqGwubu3q5uHcl9QRkRIUmN2i56uuLy4rp6LdmNSSERGUF9yh5urt7u5sKGOemZVSURFTlxvg5eptbu6sqSSfmlYS0RFTFlr"></audio>
    </body></html>
    DOC""")
    machine.succeed("chown -R alice /home/alice")
    machine.succeed(f"su alice -c '{session} we-linux apply /home/alice/web'")
    machine.wait_until_succeeds(f"su alice -c '{session} we-linux status' | grep -q 'state: shown'", timeout=120)
    # Long enough for a process that is going to end to have ended and come back.
    machine.sleep(20)
    print(machine.succeed(f"su alice -c '{session} we-linux logs' | tail -60"))
    machine.fail("grep -q 'web process ended' /run/user/1000/we-linux/engine.log")
    machine.succeed(f"su alice -c '{session} grim /tmp/desk.png'")
    colour = machine.succeed("magick /tmp/desk.png -format '%[hex:p{200,200}]' info:").strip()
    assert colour.upper().startswith("00FF00"), f"the page is not on the desktop: the pixel is {colour}"
  '';
}
