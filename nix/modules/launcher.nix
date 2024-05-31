# Definitions can be imported from a separate file like this one

{ self, inputs, lib, ... }@flake: {
  perSystem = { config, self', inputs', system, pkgs, ... }:
    let
      rustToolchain = config.rustHelper.mkRust {
        track = "stable";
        version = "1.78.0";
      };
      craneLib = inputs.crane.lib.${system}.overrideToolchain rustToolchain;

      apple_sdk =
        if system == "x86_64-darwin"
        then pkgs.darwin.apple_sdk_10_12
        else pkgs.darwin.apple_sdk_11_0;

      commonArgs = {

        pname = "hc-launch";
        src = inputs.launcher;

        cargoExtraArgs = "--bin hc-launch";

        buildInputs = [
          pkgs.glib
          pkgs.perl
        ]
        ++ (lib.optionals pkgs.stdenv.isLinux
          [
            pkgs.webkitgtk.dev
          ])
        ++ lib.optionals pkgs.stdenv.isDarwin
          [
            apple_sdk.frameworks.AppKit
            apple_sdk.frameworks.WebKit
          ]
        ;

        nativeBuildInputs = [ ]
          ++ (lib.optionals pkgs.stdenv.isLinux
          [
            pkgs.go
            pkgs.pkg-config
          ])
          ++ (lib.optionals pkgs.stdenv.isDarwin [
          (if pkgs.system == "x86_64-darwin" then
            pkgs.darwin.apple_sdk_11_0.stdenv.mkDerivation
              {
                name = "go";
                nativeBuildInputs = with pkgs; [
                  makeBinaryWrapper
                  go
                ];
                dontBuild = true;
                dontUnpack = true;
                installPhase = ''
                  makeWrapper ${pkgs.go}/bin/go $out/bin/go
                '';
              } else pkgs.go)
        ])
        ;

        doCheck = false;
      };

      # derivation building all dependencies
      deps = craneLib.buildDepsOnly (commonArgs // { });

      # derivation with the main crates
      package = craneLib.buildPackage (commonArgs // {
        cargoArtifacts = deps;

        stdenv =
          if pkgs.stdenv.isDarwin then
            pkgs.overrideSDK pkgs.stdenv "11.0"
          else
            pkgs.stdenv;
      });

    in
    {
      packages = {
        hc-launch = package;
      };
    };
}
