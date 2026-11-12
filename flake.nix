{
  description = "WE Linux: Wallpaper Engine's library, workshop and live wallpapers, native on Linux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    penguin-engine = {
      url = "github:MoonTiwzy/penguin-engine/low-footprint";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      penguin-engine,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: rec {
        we-linux = pkgs.callPackage ./nix/package.nix {
          penguin-engine = penguin-engine.packages.${pkgs.stdenv.hostPlatform.system}.default;
        };
        default = we-linux;
      });

      # programs.we-linux.enable = true;
      nixosModules.default = import ./nix/module.nix self;
      # services.we-linux.enable = true;
      homeManagerModules.default = import ./nix/home.nix self;

      # A NixOS VM: the window opens under a compositor, the tools run.
      checks.x86_64-linux.vm = nixpkgs.legacyPackages.x86_64-linux.testers.runNixOSTest (
        import ./nix/test.nix self
      );
    };
}
