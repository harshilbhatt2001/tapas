use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{Days, Local, NaiveDate};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{ArgValueCandidates, CompleteEnv, CompletionCandidate};
use google_calendar3::yup_oauth2::read_application_secret;
use tapas::{
    calc,
    export::{self, ExportOpts},
    google::auth,
    model::{
        DAY_FMT, DAYS, Device, Plan, STORE_VERSION, Store, TIME_FMT, Week, hm, week_monday,
        week_title,
    },
    services::{self, Prompt},
    storage::{self, Paths},
    sync::{self, Drift, Outcome, merge::Side},
};

/// Terminal training-week planner. Runs the TUI without a subcommand.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Export a template as iCalendar or Google Calendar CSV.
    Export {
        format: Format,
        /// Template name or id; the first template by default.
        #[arg(long, add = ArgValueCandidates::new(plan_names))]
        plan: Option<String>,
        /// Monday of the first week (YYYY-MM-DD); next Monday by default.
        #[arg(long)]
        start: Option<NaiveDate>,
        /// Number of weeks, 1 to 52; the saved setting by default.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=52))]
        weeks: Option<u32>,
        /// Include work and commute.
        #[arg(long)]
        life: bool,
        /// Output file; stdout by default.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// List templates.
    Plans,
    /// Copy a template's sessions into consecutive dated weeks.
    Apply {
        /// Template name or id.
        #[arg(add = ArgValueCandidates::new(plan_names))]
        template: String,
        /// Any day of the first week (YYYY-MM-DD); next Monday by default.
        #[arg(long)]
        from: Option<NaiveDate>,
        /// Number of weeks, 1 to 52.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=52))]
        weeks: u32,
        /// Replace the sessions of weeks that already have some.
        #[arg(long)]
        replace: bool,
    },
    /// Print the dated week containing a date.
    Week {
        /// Any day of the week (YYYY-MM-DD); today by default.
        #[arg(long)]
        date: Option<NaiveDate>,
    },
    /// Google account setup and Calendar push.
    Google {
        #[command(subcommand)]
        command: GoogleCommand,
    },
    /// Read from Google Health.
    Health {
        #[command(subcommand)]
        command: HealthCommand,
    },
    /// Sync the store with Google Drive: push, pull or merge.
    #[command(args_conflicts_with_subcommands = true)]
    Sync {
        #[command(subcommand)]
        command: Option<SyncCommand>,
        /// Overwrite the other side with this one instead of merging; the overwritten copy is
        /// saved to sync/conflict-*.json in the data dir.
        #[arg(long, value_enum)]
        keep: Option<Keep>,
    },
}

#[derive(Subcommand)]
enum SyncCommand {
    /// Local changes, last sync and Drive's copy, without writing anything.
    Status,
}

#[derive(Clone, Copy, ValueEnum)]
enum Keep {
    /// This machine's store.
    Local,
    /// Google Drive's store.
    Remote,
}

#[derive(Subcommand)]
enum GoogleCommand {
    /// Install a Google "Desktop app" OAuth client JSON.
    Setup { client_secret: PathBuf },
    /// Open the browser consent pages (one per API) and cache tokens.
    Login {
        /// Log in to just one API.
        #[arg(long, value_parser = ["calendar", "health"])]
        only: Option<String>,
    },
    /// Replace a template's events in the tapas Google calendar.
    Push {
        /// Template name or id; the first template by default.
        #[arg(long, add = ArgValueCandidates::new(plan_names))]
        plan: Option<String>,
        /// Monday of the first week (YYYY-MM-DD); next Monday by default.
        #[arg(long)]
        start: Option<NaiveDate>,
        /// Number of weeks, 1 to 52; the saved setting by default.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=52))]
        weeks: Option<u32>,
    },
}

#[derive(Subcommand)]
enum HealthCommand {
    /// Latest body weight.
    Weight {
        /// Store it as the profile weight.
        #[arg(long)]
        apply: bool,
    },
    /// A dated week's planned training next to recorded workouts, per day.
    Week {
        /// Any day of the week (YYYY-MM-DD); this week by default.
        #[arg(long)]
        start: Option<NaiveDate>,
        /// Also list commute rides (bike rides under 30 min or under 6 MET), which never count.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Ics,
    Csv,
}

fn main() -> Result<()> {
    CompleteEnv::with_factory(Cli::command).complete();
    let cli = Cli::parse();
    let paths = Paths::resolve()?;
    if let Some(Command::Google { command }) = cli.command {
        return runtime()?.block_on(google(command, &paths));
    }
    let (mut store, device) = storage::load(&paths)?;
    match cli.command {
        None => tapas::tui::run(paths, store, device)?,
        Some(Command::Plans) => list_plans(&store),
        Some(Command::Apply {
            template,
            from,
            weeks,
            replace,
        }) => apply(&paths, &mut store, &template, from, weeks, replace)?,
        Some(Command::Week { date }) => {
            print_week(&store, date.unwrap_or_else(|| Local::now().date_naive()));
        }
        Some(Command::Export {
            format,
            plan,
            start,
            weeks,
            life,
            output,
        }) => {
            let plan = pick_plan(&store, plan.as_deref())?;
            let opts = ExportOpts {
                first_monday: start
                    .unwrap_or_else(|| export::next_monday(Local::now().date_naive())),
                weeks: weeks.unwrap_or(store.export.weeks),
                include_life: life || store.export.include_life,
            };
            let events = export::events(&store, plan, &opts);
            if events.is_empty() {
                bail!("nothing to export");
            }
            let text = match format {
                Format::Ics => export::to_ics(plan, &events, opts.weeks),
                Format::Csv => export::to_google_csv(&events, opts.weeks)?,
            };
            match output {
                Some(file) => {
                    fs::write(&file, &text)
                        .with_context(|| format!("writing {}", file.display()))?;
                }
                None => print!("{text}"),
            }
        }
        Some(Command::Google { .. }) => unreachable!("handled before loading the store"),
        Some(Command::Health { command }) => {
            runtime()?.block_on(health(command, &paths, &mut store))?;
        }
        Some(Command::Sync { command, keep }) => {
            let rt = runtime()?;
            match command {
                Some(SyncCommand::Status) => rt.block_on(sync_status(&paths, &store))?,
                None => rt.block_on(sync_now(keep, &paths, &mut store, &device))?,
            }
        }
    }
    Ok(())
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

async fn google(cmd: GoogleCommand, paths: &Paths) -> Result<()> {
    match cmd {
        GoogleCommand::Setup { client_secret } => {
            read_application_secret(&client_secret).await.with_context(|| {
                format!(
                    "{} is not a Google OAuth client JSON (create a \"Desktop app\" client in Google Cloud Console)",
                    client_secret.display()
                )
            })?;
            let dest = paths.client_secret_file();
            let bytes = fs::read(&client_secret)
                .with_context(|| format!("reading {}", client_secret.display()))?;
            storage::write_atomic(&dest, &bytes)?;
            println!("Installed OAuth client at {}", dest.display());
            println!("Next: tapas google login");
        }
        GoogleCommand::Login { only } => {
            let secret = paths.client_secret_file();
            if !secret.exists() {
                bail!("no Google OAuth client yet; run `tapas google setup <client_secret.json>`");
            }
            // Separate consents: the Health API rejects tokens that also carry Calendar scopes.
            for api in auth::Api::ALL {
                if only.as_deref().is_some_and(|o| o != api.name()) {
                    continue;
                }
                println!("Logging in to Google {}", api.name());
                let tokens = services::tokens_file(paths, api);
                auth::login(&secret, api, &tokens).await?;
                println!("Logged in; tokens cached at {}", tokens.display());
            }
        }
        GoogleCommand::Push { plan, start, weeks } => {
            let (mut store, _) = storage::load(paths)?;
            let store = &mut store;
            let plan = pick_plan(store, plan.as_deref())?.clone();
            let opts = ExportOpts {
                first_monday: start
                    .unwrap_or_else(|| export::next_monday(Local::now().date_naive())),
                weeks: weeks.unwrap_or(store.export.weeks),
                include_life: store.export.include_life,
            };
            let r = services::push_plan(paths, store, &plan, &opts, Prompt::Browser).await?;
            store.export.calendar_id = Some(r.calendar_id);
            storage::save(paths, store)?;
            println!(
                "Pushed {} to \"{}\": {} events created, {} replaced",
                plan.name, store.export.calendar_name, r.created, r.deleted
            );
        }
    }
    Ok(())
}

async fn health(cmd: HealthCommand, paths: &Paths, store: &mut Store) -> Result<()> {
    match cmd {
        HealthCommand::Weight { apply } => {
            let Some((kg, at)) = services::latest_weight(paths, Prompt::Browser).await? else {
                bail!("no weight in Google Health for the last 90 days");
            };
            let kg = (kg * 10.0).round() / 10.0;
            println!(
                "{kg} kg, measured {}",
                at.with_timezone(&Local).format("%Y-%m-%d %H:%M")
            );
            if apply {
                store.profile.weight = kg;
                storage::save(paths, store)?;
                println!("Profile weight set to {kg} kg");
            }
        }
        HealthCommand::Week { start, all } => {
            let monday = week_monday(start.unwrap_or_else(|| Local::now().date_naive()));
            let workouts = services::fetch_week_actuals(paths, monday, Prompt::Browser)
                .await?
                .workouts;
            let plan = store
                .week(monday)
                .map_or_else(|| Week::new(monday).plan, |w| w.plan.clone());
            let days = services::planned_vs_done(
                &store.library,
                &plan,
                monday,
                &workouts,
                store.profile.weight,
            );
            println!("{} vs Google Health", week_title(monday, &plan.name));
            let mut hidden = 0;
            for (d, day) in days.iter().enumerate() {
                let mut what = services::workouts_text(&day.done);
                if all && !day.commutes.is_empty() {
                    let sep = if what.is_empty() { "" } else { "; " };
                    what = format!(
                        "{what}{sep}commute: {}",
                        services::workouts_text(&day.commutes)
                    );
                }
                hidden += day.commutes.len();
                println!(
                    "{}  planned {:>5} ({})  done {:>5} ({})  {what}",
                    DAYS[d],
                    hm(day.planned_min.into()),
                    day.planned,
                    hm(day.done_min().into()),
                    day.done.len(),
                );
            }
            if !all && hidden > 0 {
                println!("{hidden} commute rides not counted; --all lists them");
            }
        }
    }
    Ok(())
}

async fn sync_now(
    keep: Option<Keep>,
    paths: &Paths,
    store: &mut Store,
    device: &Device,
) -> Result<()> {
    let side = keep.map(|k| match k {
        Keep::Local => Side::Local,
        Keep::Remote => Side::Remote,
    });
    let r = services::sync_store(paths, store, device, side).await?;
    for w in &r.warnings {
        eprintln!("warning: {w}");
    }
    match &r.outcome {
        Outcome::UpToDate => println!("Up to date with Google Drive"),
        Outcome::Pushed => println!("Pushed local changes to Google Drive"),
        Outcome::Pulled => println!("Pulled changes from Google Drive"),
        Outcome::Created => println!(
            "Uploaded this store to Google Drive; `tapas sync` on another machine fetches it"
        ),
        Outcome::Merged { conflicts } => {
            println!("Merged this machine's and Google Drive's changes");
            if !conflicts.is_empty() {
                println!(
                    "{} changed on both sides; the newer edit won:",
                    conflicts.len()
                );
                for c in conflicts {
                    println!("  {}", sync::describe(c, store));
                }
            }
        }
        Outcome::Kept(Side::Local) => println!("Replaced Google Drive's store with this one"),
        Outcome::Kept(Side::Remote) => println!("Replaced this store with Google Drive's"),
    }
    for f in &r.conflict_files {
        println!("Saved the overwritten copy to {}", f.display());
    }
    Ok(())
}

async fn sync_status(paths: &Paths, store: &Store) -> Result<()> {
    let state = sync::read_state(paths)?;
    let local = match &state {
        None => "never synced".to_owned(),
        Some(s) => {
            let at = s.synced_at.with_timezone(&Local).format("%Y-%m-%d %H:%M");
            let changes = if sync::is_dirty(paths, store)? {
                "changes not on Drive yet"
            } else {
                "no changes since"
            };
            format!("last synced {at}, {changes}")
        }
    };
    println!("Local: {local}");
    match services::sync_status(paths).await {
        Ok((drift, files)) => {
            let drive = match drift {
                Drift::Missing => "no store yet; `tapas sync` uploads this one",
                Drift::Unchanged => "unchanged since the last sync",
                Drift::Changed => "changed on another machine since the last sync",
                Drift::Untracked => "holds a store this machine has not synced",
            };
            println!("Drive: {drive}");
            if let Some(f) = files.first() {
                let base = state.as_ref().map_or("none", |s| s.base_head.as_str());
                println!("  head revision {}, last synced {base}", f.head_revision_id);
                if f.schema != Some(STORE_VERSION) {
                    let v = f
                        .schema
                        .map_or_else(|| "none".to_owned(), |v| v.to_string());
                    println!("  schema {v}, but this tapas reads only version {STORE_VERSION}");
                }
            }
            if files.len() > 1 {
                println!(
                    "  {} copies of {}; `tapas sync` merges them",
                    files.len(),
                    sync::FILE_NAME
                );
            }
        }
        Err(e) => println!("Drive: {e}"),
    }
    Ok(())
}

/// Saved plan names, for shell completion of `--plan`.
fn plan_names() -> Vec<CompletionCandidate> {
    let Ok((store, _)) = Paths::resolve().and_then(|p| storage::load(&p)) else {
        return Vec::new();
    };
    store
        .plans
        .iter()
        .map(|p| CompletionCandidate::new(&p.name))
        .collect()
}

/// The named template, or the first one.
fn pick_plan<'a>(store: &'a Store, name: Option<&str>) -> Result<&'a Plan> {
    match name {
        Some(n) => store
            .plan_by_name(n)
            .with_context(|| format!("no template named {n:?}")),
        None => Ok(&store.plans[0]),
    }
}

fn list_plans(store: &Store) {
    for p in &store.plans {
        let hours = calc::summary(&store.library, p).hours;
        println!("{:<24} {hours:>5} h", p.name);
    }
}

/// `tapas apply`: refuses to replace sessions without `replace`.
fn apply(
    paths: &Paths,
    store: &mut Store,
    template: &str,
    from: Option<NaiveDate>,
    weeks: u32,
    replace: bool,
) -> Result<()> {
    let (id, name) = {
        let t = pick_plan(store, Some(template))?;
        (t.id.clone(), t.name.clone())
    };
    let from = week_monday(from.unwrap_or_else(|| export::next_monday(Local::now().date_naive())));
    let filled = store.filled_weeks(from, weeks);
    if !filled.is_empty() && !replace {
        let dates: Vec<String> = filled
            .iter()
            .map(|m| m.format(DAY_FMT).to_string())
            .collect();
        bail!(
            "these weeks already have sessions: {}; --replace overwrites them",
            dates.join(", ")
        );
    }
    if !store.apply_template(&id, from, weeks) {
        bail!("no template with id {id}");
    }
    storage::save(paths, store)?;
    let count = if weeks == 1 { "week" } else { "weeks" };
    println!(
        "Applied {name} to {weeks} {count} from {}",
        from.format(DAY_FMT)
    );
    Ok(())
}

/// `tapas week`: the stored week containing `date`, day by day.
fn print_week(store: &Store, date: NaiveDate) {
    let monday = week_monday(date);
    let Some(week) = store.week(monday) else {
        println!("{}", week_title(monday, ""));
        println!("empty");
        return;
    };
    println!("{}", week_title(monday, &week.plan.name));
    for d in 0..7 {
        println!("{}", (monday + Days::new(d as u64)).format("%a %-d"));
        for it in week.plan.sorted_day(d) {
            let title = store.library.title(it);
            if it.dur == 0 {
                println!("  all day      {title}");
            } else {
                println!(
                    "  {}–{} {title} ({})",
                    it.start.format(TIME_FMT),
                    it.end().format(TIME_FMT),
                    hm(it.dur.into())
                );
            }
        }
    }
}
