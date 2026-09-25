# tapas

A terminal training-week planner. Lay out a week of sessions (runs, rides, strength, life
blocks), see per-day load, energy and fuelling guidance, export the plan as iCalendar or
Google Calendar CSV, push it to Google Calendar and compare it with workouts and weight from
Google Health. It is a [ratatui](https://ratatui.rs) port of a single-page web artifact
(`reference/artifact-training-week-planner.html`).

## Install

With Nix (flakes enabled, nothing else needed):

```sh
nix profile install github:harshilbhatt2001/tapas   # install
nix run github:harshilbhatt2001/tapas               # run without installing
nix profile install .                      # from a local checkout
```

With Cargo (Rust 1.98+):

```sh
cargo install --locked --path .
```

### Shell completion

The Nix package installs completions for bash, fish and zsh. With a Cargo install, register
them yourself; `--plan` completes your saved plan names.

```sh
echo 'COMPLETE=fish tapas | source' > ~/.config/fish/completions/tapas.fish   # fish
echo 'source <(COMPLETE=bash tapas)' >> ~/.bashrc                            # bash
echo 'source <(COMPLETE=zsh tapas)' >> ~/.zshrc                              # zsh
```

## Development

The toolchain comes from [devenv](https://devenv.sh); there is no global cargo.

```sh
devenv shell                  # rust toolchain, clippy, rustfmt, rust-analyzer, nom, nvd
devenv test                   # git hooks (rustfmt, clippy pedantic -D warnings), cargo test
tapas                         # inside the shell: cargo run -- "$@"
nom build .#default           # build the Nix package with readable output
devenv build outputs.tapas    # same derivation via devenv
pkg-diff                      # build and nvd diff the closure against the previous result
```

The package is defined once in `nix/package.nix` and used by both `flake.nix` and
`devenv.nix`. Flakes only see git-tracked files, so `git add` new files before `nix build`.

## Google setup

Calendar push, Health import and Drive store sync need your own OAuth client.

1. In the [Google Cloud Console](https://console.cloud.google.com/) create a project.
2. Enable the **Google Calendar API**, the **Google Health API** and the **Google Drive API**.
3. Configure the OAuth consent screen (External, Testing), add the scopes listed below
   (including `drive.appdata`) under Data access, and add your Google account as a
   **test user**. The Health scopes are Restricted, so only test users can consent until the
   app is verified.
4. Create an OAuth client ID of type **Desktop app** and download its JSON.
5. Hand it to tapas and log in:

```sh
tapas google setup client_secret.json   # copies it into the config dir
tapas google login                      # browser consent, caches tokens
```

Scopes requested: `calendar.app.created` (tapas only touches the calendar it creates),
`drive.appdata` (a hidden app folder in your Drive, used to sync the store between machines),
`googlehealth.activity_and_fitness.readonly`,
`googlehealth.health_metrics_and_measurements.readonly`. Calendar and Drive share one consent.

Plans live in the platform data dir (`~/.local/share/tapas` on Linux), the OAuth client and
tokens in the config dir (`~/.config/tapas`). Set `TAPAS_HOME` to use
`$TAPAS_HOME/{data,config}` instead.

## Sync

tapas syncs the store between your machines through a hidden app folder in your own Google
Drive (`drive.appdata`: tapas sees only its own files there, and they do not show up in
Drive). There is no server of ours.

To turn it on, do the [Google setup](#google-setup) on each machine, with the Google Drive
API enabled and `drive.appdata` on the consent screen, and grant Drive access at login:

```sh
tapas google login --only calendar
tapas sync                              # first machine uploads, the others fetch it
```

What syncs: plans, the session library, the profile and the export settings (including the
Google calendar id, so every machine pushes to the same calendar). What stays on each
machine: which plan is open (`device.json`), the OAuth client and tokens, and the sync
bookkeeping in `<data dir>/sync/` (`state.json`, `base.json`, `conflict-*.json`).

When both machines changed the store, tapas merges per plan, session and type: a change on
one side wins, and if both sides changed the same thing the later edit wins. Before a merge or
`--keep` replaces the local store, the old one is saved to `<data dir>/sync/conflict-<utc>.json`.

```sh
tapas sync                              # push, pull or merge once; lists edits that collided
tapas sync status                       # local changes, last sync, Drive's copy; writes nothing
tapas sync --keep local                 # overwrite Drive with this machine's store
tapas sync --keep remote                # overwrite this machine's store with Drive's
```

The TUI syncs by itself: at start, 3 s after your last edit, and on quit (waiting at most
5 s). `s` on screen 5 syncs now. The header shows the state: off, syncing, synced HH:MM,
merged N (edits that collided), offline, login needed or error. Offline, keep working:
edits are saved locally and tapas retries every minute. Sync never opens a browser; when
the login no longer works it says "login needed" and you run `tapas google login --only
calendar` in a terminal. Without an OAuth client or login, sync is simply off. A store of
another schema version, local or on Drive, is refused and never overwritten; run the same
tapas on every machine.

### Sharing plans through a synced folder

Without Google, set `TAPAS_STORE` to a file inside a folder that Syncthing, Dropbox or a Drive desktop client
keeps in sync, on every machine:

```sh
export TAPAS_STORE=~/Sync/tapas/store.json
```

Only the store (plans, library, profile, export settings) moves there. It wins over
`TAPAS_HOME`. The OAuth client, tokens and `device.json` (which plan this machine has open)
stay in the local dirs, so no credentials land in the synced folder. tapas replaces the file
in one rename, so the sync tool never picks up a half-written store.

Caveat: tapas does not notice the conflict copies a sync tool makes when two machines edit
before syncing (for example `store.sync-conflict-*.json` or "conflicted copy"). Edit on one
machine at a time, and merge or delete such copies by hand.

## CLI

```
tapas                                   # TUI
tapas export ics|csv [--plan N] [--start YYYY-MM-DD] [--weeks N] [--life] [-o FILE]
tapas plans                             # list
tapas google setup <client_secret.json> # copy OAuth client into config dir
tapas google login                      # browser consent, caches tokens
tapas google push [--plan N] [--start D] [--weeks N]
tapas health weight [--apply]           # latest weight, optionally into profile
tapas health week [--start D]           # planned vs done for a week
tapas sync [--keep local|remote]        # sync the store with Google Drive
tapas sync status                       # what a sync would find, without writing
```

## License

MIT, see [LICENSE](LICENSE).
