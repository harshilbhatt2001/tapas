{
  pkgs,
  config,
  ...
}: let
  dev = "${config.devenv.root}/.dev";
in {
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

  # A local server for `devenv up`, with its state in .dev/. TAPAS_ALLOWED_EMAIL and
  # TAPAS_GOOGLE_WEB_CLIENT_ID go in devenv.local.nix; the web client secret in
  # .dev/google-web-client-secret.
  env = {
    TAPAS_DATABASE = "${dev}/tapas.db";
    TAPAS_GOOGLE_WEB_CLIENT_SECRET_FILE = "${dev}/google-web-client-secret";
    TAPAS_TOKEN_KEY_FILE = "${dev}/token-key";
    RUST_LOG = "info";
  };
  processes.server = {
    exec = ''
      mkdir -p ${dev}
      [ -e "$TAPAS_TOKEN_KEY_FILE" ] || head -c 32 /dev/urandom > "$TAPAS_TOKEN_KEY_FILE"
      if [ ! -e "$TAPAS_GOOGLE_WEB_CLIENT_SECRET_FILE" ]; then
        echo "missing $TAPAS_GOOGLE_WEB_CLIENT_SECRET_FILE: write the web client secret there" >&2
        exit 1
      fi
      exec cargo run -p tapas-server -- serve
    '';
    watch = {
      paths = [./crates ./Cargo.toml];
      extensions = ["rs" "sql" "toml"];
    };
    ready.http.get = {
      port = 8080;
      path = "/healthz";
    };
  };

  # Installs the debug app pointed at the local server through a USB reverse tunnel.
  scripts.phone.exec = ''
    adb reverse tcp:8080 tcp:8080
    cd ${config.devenv.root}/android
    ./gradlew installDebug -Ptapas.url=http://localhost:8080 "$@"
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
