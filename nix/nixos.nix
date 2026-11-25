{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.datarig;
  toml = pkgs.formats.toml { };
  configFile = "datarig/config.toml";

  configuredPackage = pkgs.symlinkJoin {
    name = "datarig-configured";
    paths = [ cfg.package ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      wrapProgram "$out/bin/datarig" --add-flags "--config /etc/${configFile}"
    '';
  };
in
{
  options.programs.datarig = {
    enable = lib.mkEnableOption "datarig";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.datarig or (pkgs.callPackage ./package.nix { });
      defaultText = lib.literalExpression "pkgs.datarig or (pkgs.callPackage ./package.nix { })";
      description = "The datarig package to install.";
    };

    settings = lib.mkOption {
      type = lib.types.nullOr (lib.types.attrsOf toml.type);
      default = null;
      description = ''
        System-wide settings for datarig, generated as a read-only TOML file.
        Leave unset to use datarig's writable per-user configuration.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [
      (if cfg.settings == null then cfg.package else configuredPackage)
    ];

    environment.etc.${configFile} = lib.mkIf (cfg.settings != null) {
      source = toml.generate "datarig-config.toml" ({ version = 2; } // cfg.settings);
    };
  };
}
