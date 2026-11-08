{
  description = "G7 Link - Android and Linux PC working as one";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      packageBuilder = system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          qtEnv = pkgs.qt6.env "qt6-env" [
            pkgs.qt6.qtbase
            pkgs.qt6.qtdeclarative
            pkgs.qt6.qtsvg
            pkgs.qt6.qtwayland
            pkgs.qt6.qtshadertools
            pkgs.qt6.qttools
          ];
        in pkgs.rustPlatform.buildRustPackage {
          pname = "g7link";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = with pkgs; [
            pkg-config
            cmake
            ninja
            qt6.wrapQtAppsHook
          ];

          dontUseCmakeConfigure = true;
          dontUseNinjaBuild = true;
          dontUseNinjaInstall = true;

          buildInputs = with pkgs; [
            qtEnv
            pkgs.qt6.qtbase
            pkgs.qt6.qtdeclarative
            pkgs.qt6.qtsvg
            pkgs.qt6.qtwayland
            pkgs.libGL
            pkgs.vulkan-loader
            pkgs.libx11
            pkgs.libxkbcommon
            pkgs.ffmpeg
          ];

          LRELEASE = "${qtEnv}/bin/lrelease";
          QT_PLUGIN_PATH = "${qtEnv}/lib/qt-6/plugins";
          QML2_IMPORT_PATH = "${qtEnv}/lib/qt-6/qml";

          preBuild = ''
            mkdir -p $TMPDIR/qt-wrap
            cat > $TMPDIR/qt-wrap/qt.conf <<EOF
[Paths]
Prefix = ${qtEnv}
Headers = include
Libraries = lib
Plugins = lib/qt-6/plugins
QmlImports = lib/qt-6/qml
EOF
            ln -s ${qtEnv}/bin/qmake $TMPDIR/qt-wrap/qmake
            export QMAKE="$TMPDIR/qt-wrap/qmake"
            export PATH="$TMPDIR/qt-wrap:$PATH"
          '';

          cargoBuildFlags = [
            "-p" "g7link-desktop"
            "-p" "g7link-cli"
          ];

          doCheck = false;

          preFixup = ''
            patchelf --add-rpath "${pkgs.lib.makeLibraryPath (with pkgs; [
              libGL
              vulkan-loader
              libx11
              libxkbcommon
              ffmpeg
            ])}" $out/bin/g7link-desktop
          '';

          postInstall = ''
            # Translations
            mkdir -p $out/share/g7link
            for qm in target/*/*/g7link_*.qm target/*/g7link_*.qm; do
              if [ -f "$qm" ]; then
                cp -n "$qm" $out/share/g7link/ || true
              fi
            done

            # Desktop integration
            install -Dm644 desktop/packaging/linux/g7link.desktop $out/share/applications/g7link.desktop
            install -Dm644 desktop/packaging/linux/org.g7link.desktop.metainfo.xml $out/share/metainfo/org.g7link.desktop.metainfo.xml

            # Icons
            if [ -f assets/brand/icon.svg ]; then
              install -Dm644 assets/brand/icon.svg $out/share/icons/hicolor/scalable/apps/g7link.svg
              install -Dm644 assets/brand/icon.svg $out/share/pixmaps/g7link.svg
            fi
            if [ -f assets/brand/icon-512.png ]; then
              install -Dm644 assets/brand/icon-512.png $out/share/icons/hicolor/512x512/apps/g7link.png
              install -Dm644 assets/brand/icon-512.png $out/share/pixmaps/g7link.png
            fi
          '';

          meta = with pkgs.lib; {
            description = "Your Android phone and your PC, working as one";
            homepage = "https://github.com/Astraque-Softwares-Ltd/g7link";
            license = licenses.gpl3Plus;
            maintainers = with maintainers; [ ];
            platforms = platforms.linux;
            mainProgram = "g7link-desktop";
          };
        };
    in
    (flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        qtEnv = pkgs.qt6.env "qt6-env" [
          pkgs.qt6.qtbase
          pkgs.qt6.qtdeclarative
          pkgs.qt6.qtsvg
          pkgs.qt6.qtwayland
          pkgs.qt6.qtshadertools
          pkgs.qt6.qttools
        ];
      in {
        packages.default = packageBuilder system;
        packages.g7link = packageBuilder system;

        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/g7link-desktop";
          meta.description = "G7 Link desktop application";
        };

        apps.cli = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/g7link";
          meta.description = "G7 Link command-line client";
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            pkg-config
            cmake
            ninja
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            patchelf
          ];

          buildInputs = with pkgs; [
            qtEnv
            qt6.qtbase
            qt6.qtdeclarative
            qt6.qtsvg
            qt6.qtwayland
            libGL
            vulkan-loader
            libx11
            libxkbcommon
            ffmpeg
          ];

          shellHook = ''
            export QT_WRAP_DIR=$(mktemp -d)
            cat > "$QT_WRAP_DIR/qt.conf" <<EOF
[Paths]
Prefix = ${qtEnv}
Headers = include
Libraries = lib
Plugins = lib/qt-6/plugins
QmlImports = lib/qt-6/qml
EOF
            ln -s "${qtEnv}/bin/qmake" "$QT_WRAP_DIR/qmake"
            export QMAKE="$QT_WRAP_DIR/qmake"
            export PATH="$QT_WRAP_DIR:$PATH"
          '';

          LRELEASE = "${qtEnv}/bin/lrelease";
          QT_PLUGIN_PATH = "${qtEnv}/lib/qt-6/plugins";
          QML2_IMPORT_PATH = "${qtEnv}/lib/qt-6/qml";
          LD_LIBRARY_PATH = "${pkgs.lib.makeLibraryPath (with pkgs; [
            qtEnv
            libGL
            vulkan-loader
            libx11
            libxkbcommon
            ffmpeg
          ])}:$LD_LIBRARY_PATH";
          LIBRARY_PATH = "${pkgs.lib.makeLibraryPath (with pkgs; [
            qtEnv
            libGL
            vulkan-loader
            libx11
            libxkbcommon
            ffmpeg
          ])}:$LIBRARY_PATH";
        };
      })
    ) // {
      overlays.default = final: prev: {
        g7link = packageBuilder final.system;
      };
    };
}
