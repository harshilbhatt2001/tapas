{
  pkgs,
  ...
}: {
  imports = [./claude-code.nix];

  languages.rust.enable = true;

  packages = [
    pkgs.git
    pkgs.gh
  ];

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
}
