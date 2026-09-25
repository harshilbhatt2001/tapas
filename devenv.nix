{ pkgs, config, ... }:

{
  imports = [ ./claude-code.nix ];

  languages.rust.enable = true;

  # nom: readable nix build output; nvd: closure diffs (see pkg-diff).
  packages = [ pkgs.git pkgs.nix-output-monitor pkgs.nvd ];

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
