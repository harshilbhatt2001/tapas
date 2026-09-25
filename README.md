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
devenv test                   # cargo fmt --check, clippy -D warnings, cargo test
tapas                         # inside the shell: cargo run -- "$@"
nom build .#default           # build the Nix package with readable output
devenv build outputs.tapas    # same derivation via devenv
pkg-diff                      # build and nvd diff the closure against the previous result
```

The package is defined once in `nix/package.nix` and used by both `flake.nix` and
`devenv.nix`. Flakes only see git-tracked files, so `git add` new files before `nix build`.

## Google setup

Calendar push and Health import need your own OAuth client.

1. In the [Google Cloud Console](https://console.cloud.google.com/) create a project.
2. Enable the **Google Calendar API** and the **Google Health API**.
3. Configure the OAuth consent screen (External, Testing) and add your Google account as a
   **test user**. The Health scopes are Restricted, so only test users can consent until the
   app is verified.
4. Create an OAuth client ID of type **Desktop app** and download its JSON.
5. Hand it to tapas and log in:

```sh
tapas google setup client_secret.json   # copies it into the config dir
tapas google login                      # browser consent, caches tokens
```

Scopes requested: `calendar.app.created` (tapas only touches the calendar it creates),
`googlehealth.activity_and_fitness.readonly`,
`googlehealth.health_metrics_and_measurements.readonly`.

Plans live in the platform data dir (`~/.local/share/tapas` on Linux), the OAuth client and
tokens in the config dir (`~/.config/tapas`). Set `TAPAS_HOME` to use
`$TAPAS_HOME/{data,config}` instead.

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
```

## License

MIT, see [LICENSE](LICENSE).
