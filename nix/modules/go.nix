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
                makeWrapper ${pkgs.go}/bin/go $out/bin/go \
                  ${builtins.concatStringsSep " " (
                    builtins.map (var: "--set ${var} \"\$${var}\"") 
                    [
                      "NIX_BINTOOLS_WRAPPER_TARGET_HOST_x86_64_apple_darwin"
                      "NIX_LDFLAGS"
                      "NIX_CFLAGS_COMPILE_FOR_BUILD"
                      "NIX_CFLAGS_COMPILE"
                    ]
                  )}
              '';
            };
      };
    };
}
