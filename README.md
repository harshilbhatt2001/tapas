# tapas

A personal closed-loop roadmap tracker. You build goals with timelines and checkpoints; tapas
measures where you actually are from Garmin, Strava, Google and GitHub, compares it with where
you should be, and tells you how to correct course. Every goal runs its own control loop, and a
master loop watches whether the whole system is heading where it should.

A hosted Rust server holds all the logic and runs autonomously; a thin Android app is the UI.

This is a ground-up redesign. The previous terminal training-week planner lives on the
[`archive/tui`](https://github.com/harshilbhatt2001/tapas/tree/archive/tui) branch and is not
carried over.

## Development

The toolchain comes from [devenv](https://devenv.sh); there is no global cargo.

```sh
devenv shell   # rust toolchain, clippy, rustfmt, rust-analyzer
devenv test    # git hooks: rustfmt, clippy pedantic -D warnings, secret scanning
cargo test --workspace
```

| Path | What |
|---|---|
| `crates/core` | the domain: goals, measurements, workouts. No I/O |
| `crates/sources` | one adapter per integration (Google Health, Google Calendar, OAuth) |
| `crates/server` | axum JSON API, sign-in, the sync loop, SQLite store (`schema.sql`) |
| `android/` | Kotlin/Compose app; its API client is generated from `android/openapi.json` |
| `nix/` | the server package, its NixOS module and the Hetzner host |

After changing the API, regenerate the app's contract (a test fails while it is stale):

```sh
cargo run -p tapas-server -- openapi > android/openapi.json
```

## Android app

The Android SDK and JDK come from devenv too. Point the app at the server in
`android/local.properties` (not committed):

```properties
tapas.url=https://<domain>
tapas.googleWebClientId=<web client id>.apps.googleusercontent.com
```

```sh
cd android
./gradlew assembleDebug    # app/build/outputs/apk/debug/app-debug.apk
./gradlew installDebug     # onto a phone with USB debugging, or: adb install -r <apk>
./gradlew signingReport    # SHA-1 for the Android OAuth client
```

The app is signed with `~/.android/debug.keystore`; back it up, since a new key means a new
SHA-1 in Google Cloud and reinstalling the app.

## Running locally

Sign-in still goes through Google, so the Web and Android OAuth clients from the
[one-time setup](#one-time-setup) must exist; for local work the consent screen can stay in
Testing with yourself as a test user.

**Server.** Put your account and the web client id in `devenv.local.nix` (not committed):

```nix
{
  env.TAPAS_ALLOWED_EMAIL = "you@gmail.com";
  env.TAPAS_GOOGLE_WEB_CLIENT_ID = "<web client id>.apps.googleusercontent.com";
}
```

and the web client secret in `.dev/` (not committed):

```sh
mkdir -p .dev && printf %s "$WEB_CLIENT_SECRET" > .dev/google-web-client-secret
devenv up    # http://127.0.0.1:8080, restarts on every change under crates/
```

The first start creates `.dev/token-key` and `.dev/tapas.db`. The same variables are set in
`devenv shell`, so `cargo run -p tapas-server -- serve` works there too.

**App on a phone over USB** (with the server running):

```sh
phone    # adb reverse tcp:8080 + ./gradlew installDebug against http://localhost:8080
```

`adb reverse` makes the phone's `localhost:8080` the laptop's, so there is no Wi-Fi or IP to
manage; run `phone` again after reconnecting the cable. Debug builds allow plain HTTP to
`localhost` and `10.0.2.2` (the emulator's host) only; release builds are HTTPS-only. A
`-Ptapas.url=…` overrides `local.properties`, so a plain `./gradlew installDebug` goes back to
the hosted server.

## Hosting

The server runs on a Hetzner Cloud VPS with NixOS, declared in this repo's flake
(`nixosConfigurations.tapas`). The host rebuilds itself from `main` every night at 04:00 with the
newest nixpkgs of the pinned release.

### One-time setup

1. **Google Cloud** (one project):
   - Enable the Google Health API and the Google Calendar API.
   - OAuth consent screen: add the Health and Calendar scopes from
     `crates/sources/src/google/oauth.rs`, then **publish to In production**. In Testing, refresh
     tokens expire after 7 days and the server stops syncing.
   - Create a *Web application* OAuth client (no redirect URIs needed). Its id goes into
     `nix/host/default.nix` and the app; its secret goes onto the server (below).
   - Create an *Android* OAuth client for the app's package name and signing SHA-1
     (`cd android && ./gradlew signingReport`).
2. **Values**: set `domain` and `googleWebClientId` in `nix/host/default.nix`. Point the
   domain's A record at the VPS.
3. **Backup**: create a Hetzner Object Storage bucket `tapas-backup` in `fsn1` (or change
   `backupReplica`) and an access key.
4. **Secrets**, kept outside the repo and copied once at install:

   ```sh
   mkdir -p secrets/etc/tapas
   printf %s "$WEB_CLIENT_SECRET" > secrets/etc/tapas/google-web-client-secret
   head -c 32 /dev/urandom > secrets/etc/tapas/token-key
   printf 'LITESTREAM_ACCESS_KEY_ID=%s\nLITESTREAM_SECRET_ACCESS_KEY=%s\n' "$KEY" "$SECRET" \
     > secrets/etc/tapas/litestream.env
   chmod -R go= secrets
   ```

   Keep a copy of `token-key` somewhere safe: without it the stored Google grants cannot be
   decrypted and must be connected again.
5. **Install**: create a CX22 (x86, any Ubuntu image) with your SSH key, then

   ```sh
   nix run github:nix-community/nixos-anywhere -- \
     --flake .#tapas --extra-files ./secrets root@<server-ip>
   ```

### Deploy a change

Push to `main` and wait for the nightly rebuild, or deploy now:

```sh
nixos-rebuild switch --flake .#tapas --target-host root@<domain> --build-host root@<domain>
```
