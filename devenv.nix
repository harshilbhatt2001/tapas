{ pkgs, config, ... }:

{
  imports = [ ./claude-code.nix ];

  languages.rust.enable = true;

  # nom: readable nix build output; nvd: closure diffs (see pkg-diff).
  # hyperfine, cargo-bloat: CLI timing and binary size (see the bench scripts).
  packages = [
    pkgs.git
    pkgs.nix-output-monitor
    pkgs.nvd
    pkgs.hyperfine
    pkgs.cargo-bloat
  ];

  # Same derivation as the flake: `devenv build outputs.tapas`.
  outputs.tapas = pkgs.callPackage ./nix/package.nix { };

  # Runs the working tree from any directory.
  scripts.tapas.exec = ''
    cargo run --quiet --manifest-path ${config.devenv.root}/Cargo.toml -- "$@"
  '';

  # Build the flake package and show what changed in its closure since the last run.
  scripts.pkg-diff.exec = ''
    if [ -L result ]; then ln -sfn "$(readlink result)" result-prev; fi
    nom build .#default
    if [ -L result-prev ]; then nvd diff result-prev result; fi
  '';

  # Criterion benches. Extra args go to criterion, e.g. `bench -- --save-baseline main`.
  scripts.bench.exec = ''
    cargo bench --manifest-path ${config.devenv.root}/Cargo.toml "$@"
  '';

  # Cold-start and export timings of the release binary in a throwaway TAPAS_HOME.
  scripts.bench-cli.exec = ''
    set -euo pipefail
    cargo build --release --quiet --manifest-path ${config.devenv.root}/Cargo.toml
    bin="''${CARGO_TARGET_DIR:-${config.devenv.root}/target}/release/tapas"
    home=$(mktemp -d)
    trap 'rm -rf "$home"' EXIT
    # The starter week, so the exports have something to write.
    mkdir -p "$home/data"
    cp ${config.devenv.root}/benches/fixtures/starter-store.json "$home/data/store.json"
    TAPAS_HOME=$home hyperfine --warmup 3 -N "$@" \
      "$bin plans" \
      "$bin export ics -o /dev/null" \
      "$bin export csv --weeks 52 -o /dev/null"
  '';

  # Pre-commit hooks; `devenv test` (and so CI) runs them on every file too.
  # Lint levels, pedantic included, live in Cargo.toml `[lints]`.
  git-hooks.hooks = {
    rustfmt.enable = true;
    detect-private-keys.enable = true;
    # Google OAuth client secrets, refresh tokens and access tokens on top of the defaults.
    ripsecrets = {
      enable = true;
      settings.additionalPatterns = [
        "GOCSPX-[A-Za-z0-9_-]{20,}"
        "1//[A-Za-z0-9_-]{30,}"
        "ya29\\.[A-Za-z0-9_-]{20,}"
      ];
    };
    clippy = {
      enable = true;
      settings = {
        denyWarnings = true;
        offline = false;
        extraArgs = "--all-targets";
      };
    };
  };

  enterTest = ''
    cargo test
  '';
}
