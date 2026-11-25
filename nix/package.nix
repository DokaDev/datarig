{
  lib,
  fetchFromGitHub,
  makeWrapper,
  rustPlatform,
  versionCheckHook,
}:

rustPlatform.buildRustPackage (finalAttrs: {
  pname = "datarig";
  version = "0.42.0";

  src = fetchFromGitHub {
    owner = "DokaDev";
    repo = "datarig";
    tag = "v${finalAttrs.version}";
    hash = "sha256-ZN3NlsNR6kHNh4Ryuzmt1N/Y4qBDL7MQ1Gf7xttXhfk=";
  };

  cargoHash = "sha256-KRFSS/fVwBgS3w73s049pY0Zn9WmugosPUZBBKkKQ84=";

  nativeBuildInputs = [
    makeWrapper
    rustPlatform.bindgenHook
  ];

  cargoBuildFlags = [
    "--package"
    "datarig-tui"
  ];

  # Exercise the CLI and TUI without external databases or a terminal session.
  cargoTestFlags = [
    "--package"
    "datarig-tui"
    "--lib"
    "--test"
    "cli_doc"
  ];

  postFixup = ''
    wrapProgram "$out/bin/datarig" --set DATARIG_NO_UPDATE_CHECK 1
  '';

  nativeInstallCheckInputs = [ versionCheckHook ];
  doInstallCheck = true;

  meta = {
    description = "Terminal database client for PostgreSQL, MySQL, and MariaDB";
    homepage = "https://github.com/DokaDev/datarig";
    changelog = "https://github.com/DokaDev/datarig/releases/tag/${finalAttrs.src.tag}";
    license = with lib.licenses; [
      mit
      asl20
    ];
    mainProgram = "datarig";
    platforms = lib.platforms.linux;
  };
})
