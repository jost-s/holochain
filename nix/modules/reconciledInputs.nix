{ self, lib, inputs, ... }:

{
  options.reconciledInputs = lib.mkOption { type = lib.types.raw; };
  config.reconciledInputs = lib.genAttrs (builtins.attrNames inputs)
    (name:
      let
        input =
          if builtins.pathExists (inputs."${name}" + "/Cargo.toml")
          then inputs."${name}"
          else
            inputs."${name}";
        rev = input.rev or (self.rev or "unknown");
      in
      (input // { inherit rev; })
    );
}
