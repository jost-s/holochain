# Definitions can be imported from a separate file like this one

{ self, inputs, lib, ... }@flake: {
  perSystem = { config, self', inputs', system, pkgs, ... }:
    let
      rustToolchain = config.rustHelper.mkRust {
        track = "stable";
        version = "1.77.2";
      };
      craneLib = inputs.crane.lib.${system}.overrideToolchain rustToolchain;

      commonArgs = {

        pname = "hc-launch";
        src = inputs.launcher;

        CARGO_PROFILE = "release";

        cargoExtraArgs = "--bin hc-launch";

        buildInputs = (with pkgs; [
          # openssl

          # this is required for glib-networking
          glib
        ])
        ++ (lib.optionals pkgs.stdenv.isLinux
          (with pkgs; [
            libsoup
            # webkitgtk.dev
            # gdk-pixbuf
            # gtk3
          ]))
        ++ lib.optionals pkgs.stdenv.isDarwin
          [
            self'.legacyPackages.apple_sdk'.frameworks.AppKit
            self'.legacyPackages.apple_sdk'.frameworks.WebKit
          ]
        ;

        nativeBuildInputs = (with pkgs; [
          perl
          pkg-config

          # currently needed to build tx5
          self'.packages.goWrapper
        ])
        ++ (lib.optionals pkgs.stdenv.isLinux
          (with pkgs; [
            wrapGAppsHook
          ]))
        ++ (lib.optionals pkgs.stdenv.isDarwin [
          pkgs.xcbuild
          pkgs.libiconv
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

        # nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
        #   pkgs.makeBinaryWrapper
        # ];
      });

    in
    {
      packages = {
        hc-launch = package;
      };
    };
}
