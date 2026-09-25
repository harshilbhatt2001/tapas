{ pkgs, ... }:

let
  cacheHome =
    let xdg = builtins.getEnv "XDG_CACHE_HOME";
    in if xdg != "" then xdg else "${builtins.getEnv "HOME"}/.cache";
in
{
  imports = [ ./claude-code.nix ];

  languages.rust.enable = true;

  # nom: readable nix build output; nvd: closure diffs (see pkg-diff).
  packages = [ pkgs.git pkgs.nix-output-monitor pkgs.nvd ];

  # The repo sits on an ntfs-3g mount without exec bits, so build scripts in ./target
  # cannot run. Set via env (not an enterShell export) so it also reaches the interactive
  # shell and anything launched from it.
  env.CARGO_TARGET_DIR = "${cacheHome}/tapas/target";

  # Same derivation as the flake: `devenv build outputs.tapas`.
  outputs.tapas = pkgs.callPackage ./nix/package.nix { };

  scripts.tapas.exec = ''cargo run --quiet -- "$@"'';

  # Build the flake package and show what changed in its closure since the last run.
  scripts.pkg-diff.exec = ''
    if [ -L result ]; then ln -sfn "$(readlink result)" result-prev; fi
    nom build .#default
    if [ -L result-prev ]; then nvd diff result-prev result; fi
  '';

  enterTest = ''
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
  '';
}
