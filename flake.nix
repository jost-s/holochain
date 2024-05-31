{
  description =
    "Holochain is an open-source framework to develop peer-to-peer applications with high levels of security, reliability, and performance.";

  inputs = {
    # nix packages pointing to the github repo
    nixpkgs.url = "nixpkgs/nixos-unstable";

    # lib to build nix packages from rust crates
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # rustup, rust and cargo
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    launcher = {
      url = "github:holochain/launcher/holochain-weekly";
      flake = false;
    };
  };

  # refer to flake-parts docs https://flake.parts/
  outputs = inputs @ { self, nixpkgs, flake-parts, ... }:
    # all possible parameters for a module: https://flake.parts/module-arguments.html#top-level-module-arguments
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "aarch64-darwin" "x86_64-linux" "x86_64-darwin" ];

      imports =
        # auto import all nix code from `./modules`, treat each one as a flake and merge them
        (
          map (m: "${./.}/nix/modules/${m}")
            (builtins.attrNames (builtins.readDir ./nix/modules))
        );

      perSystem = { pkgs, ... }: {
        legacyPackages = pkgs;
      };
    };
}
