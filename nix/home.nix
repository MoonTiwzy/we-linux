# Home Manager: services.we-linux.enable = true;
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.we-linux;
in
{
  options.services.we-linux = {
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
        service). With Hyprland without UWSM, turn this off and add
        `exec-once = we-linux daemon` to the Hyprland config.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    systemd.user.services.we-linux = lib.mkIf cfg.autostart {
      Unit = {
        Description = "WE Linux: the wallpaper daemon";
        PartOf = [ "graphical-session.target" ];
        After = [ "graphical-session.target" ];
      };
      Service = {
        ExecStart = "${lib.getExe cfg.package} daemon";
        Restart = "on-failure";
        RestartSec = 2;
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
