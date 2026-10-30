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
```
