{
  lib,
  stdenv,
  rustPlatform,
  makeWrapper,
  pkg-config,
  fontconfig,
  wayland,
  libxkbcommon,
  ffmpeg,
  zenity,
  xdg-utils,
  wepaper,
}:

rustPlatform.buildRustPackage {
  pname = "we-linux";
  version = (lib.importTOML ../Cargo.toml).package.version;

  src = lib.cleanSourceWith {
    src = ../.;
    filter = path: type: !(type == "directory" && builtins.elem (baseNameOf path) [ "target" ".git" ]);
  };

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    makeWrapper
  ];
  buildInputs = [
    fontconfig
    wayland
    libxkbcommon
  ];

  postInstall = ''
    install -Dm644 assets/we-linux.desktop $out/share/applications/we-linux.desktop
    install -Dm644 assets/we-linux.svg $out/share/icons/hicolor/scalable/apps/we-linux.svg
  '';

  # The window opens Wayland and fontconfig at run time; Steam's
  # libsteam_api.so (found in an installed game) wants libstdc++. The renderer,
  # ffmpeg (stills of videos) and the file dialog come along on the PATH.
  postFixup = ''
    wrapProgram $out/bin/we-linux \
      --prefix LD_LIBRARY_PATH : ${
        lib.makeLibraryPath [
          wayland
          libxkbcommon
          fontconfig
          stdenv.cc.cc.lib
        ]
      } \
      --prefix PATH : ${
        lib.makeBinPath [
          wepaper
          ffmpeg
          zenity
          xdg-utils
        ]
      }
  '';

  meta = {
    description = "Wallpaper Engine's library, workshop and live wallpapers, native on Linux";
    homepage = "https://github.com/MoonTiwzy/we-linux";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "we-linux";
  };
}
