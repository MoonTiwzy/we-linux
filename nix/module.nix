# NixOS: programs.we-linux.enable = true;
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.we-linux;
in
{
  options.programs.we-linux = {
    enable = lib.mkEnableOption "WE Linux, Wallpaper Engine's library and live wallpapers";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      description = "The we-linux package.";
    };
    autostart = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Run `we-linux daemon` with the graphical session (a systemd user
        service). A compositor that does not reach graphical-session.target
        (Hyprland without UWSM) starts it itself: `exec-once = we-linux daemon`.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    systemd.user.services.we-linux = lib.mkIf cfg.autostart {
      description = "WE Linux: the wallpaper daemon";
      partOf = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
      wantedBy = [ "graphical-session.target" ];
      serviceConfig = {
        ExecStart = "${lib.getExe cfg.package} daemon";
        Restart = "on-failure";
        RestartSec = 2;
      };
    };
  };
}
