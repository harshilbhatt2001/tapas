# Single definition of the tapas package, shared by flake.nix and devenv.nix.
{
  lib,
  stdenv,
  rustPlatform,
  installShellFiles,
}:

let
  cargoToml = lib.importTOML ../Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  inherit (cargoToml.package) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      (lib.fileset.maybeMissing ../tests)
      (lib.fileset.maybeMissing ../benches)
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  # Dynamic completions: each script calls back into the binary, so plan names stay current.
  nativeBuildInputs = [ installShellFiles ];
  postInstall = lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
    installShellCompletion --cmd tapas \
      --bash <(COMPLETE=bash $out/bin/tapas) \
      --fish <(COMPLETE=fish $out/bin/tapas) \
      --zsh <(COMPLETE=zsh $out/bin/tapas)
  '';

  meta = {
    inherit (cargoToml.package) description;
    license = lib.licenses.mit;
    mainProgram = "tapas";
  };
}
