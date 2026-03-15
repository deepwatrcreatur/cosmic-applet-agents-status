{
  description = "COSMIC applet for monitoring coding agent status";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "cosmic-applet-agents-status";
          version = "0.1.0";
          src = ./.;

          cargoHash = "sha256-p1y9yGTTJAMaKaiskWXV1Dy0NK/VmMMI7uTIUVO8lEQ=";

          nativeBuildInputs = with pkgs; [
            pkg-config
            makeWrapper
          ];

          buildInputs = with pkgs; [
            fontconfig
            libxkbcommon
            wayland
            openssl
          ];

          postInstall = ''
            mkdir -p $out/share/applications
            cp data/com.deepwatrcreatur.CosmicAppletAgentsStatus.desktop $out/share/applications/

            wrapProgram $out/bin/cosmic-applet-agents-status \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [
                pkgs.wayland
                pkgs.libxkbcommon
                pkgs.fontconfig
              ]}
          '';
        };

        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/cosmic-applet-agents-status";
        };
      }
    );
}
