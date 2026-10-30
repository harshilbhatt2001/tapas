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

  # The Android app in android/: SDK for Gradle, no emulator (the app runs on the phone).
  languages.java = {
    enable = true;
    jdk.package = pkgs.jdk21;
  };
  android = {
    enable = true;
    platforms.version = [ "37" ];
    buildTools.version = [ "36.0.0" ];
    emulator.enable = false;
    systemImages.enable = false;
    ndk.enable = false;
    android-studio.enable = false;
    extras = [ ];
    googleAPIs.enable = false;
    googleTVAddOns.enable = false;
  };
  # One debug keystore for every checkout: the Android OAuth client is registered with its SHA-1,
  # and the phone only accepts updates signed by the same key.
  enterShell = ''
    export ANDROID_USER_HOME="$HOME/.android"
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
}
