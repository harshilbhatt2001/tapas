# Single definition of the tapas package, shared by flake.nix and devenv.nix.
{ lib, rustPlatform }:

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
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  meta = {
    inherit (cargoToml.package) description;
    license = lib.licenses.mit;
    mainProgram = "tapas";
  };
}
