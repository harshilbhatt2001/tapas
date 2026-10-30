{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "tapas-server";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../android/openapi.json
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "--package" "tapas-server" ];
  cargoTestFlags = [ "--workspace" ];

  meta.mainProgram = "tapas-server";
}
