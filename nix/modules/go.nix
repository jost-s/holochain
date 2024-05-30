{ self
, lib
, inputs
, ...
} @ flake: {
  perSystem =
    { config
    , self'
    , inputs'
    , system
    , pkgs
    , ...
    }: {
      packages = {
        goWrapper =
          # there is interference only in this specific case, we assemble a go derivationt that not propagate anything but still has everything available required for our specific use-case
          #
          # the wrapper inherits preconfigured environment variables from the
          # derivation that depends on the propagating go
          if pkgs.stdenv.isDarwin && pkgs.system == "x86_64-darwin" then
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
              }
          else pkgs.go;
      };
    };
}
