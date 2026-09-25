use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{ArgValueCandidates, CompleteEnv, CompletionCandidate};
use google_calendar3::yup_oauth2::read_application_secret;
use tapas::{
    calc,
    export::{self, ExportOpts},
    google::auth,
    model::{DAYS, Device, Plan, Store, hm},
    services,
    storage::{self, Paths},
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
    /// Export a plan as iCalendar or Google Calendar CSV.
    Export {
        format: Format,
        /// Plan name or id; the active plan by default.
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
    /// List plans.
    Plans,
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
    /// Replace a plan's events in the tapas Google calendar.
    Push {
        /// Plan name or id; the active plan by default.
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
    /// Planned training next to recorded workouts, per day.
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
    let (mut store, device) = storage::load(&paths)?;
    match cli.command {
        None => tapas::tui::run(paths, store, device)?,
        Some(Command::Plans) => list_plans(&store, &device),
        Some(Command::Export {
            format,
            plan,
            start,
            weeks,
            life,
            output,
        }) => {
            let plan = pick_plan(&store, &device, plan.as_deref())?;
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
        Some(Command::Google { command }) => {
            runtime()?.block_on(google(command, &paths, &mut store, &device))?;
        }
        Some(Command::Health { command }) => {
            runtime()?.block_on(health(command, &paths, &mut store, &device))?;
        }
    }
    Ok(())
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

async fn google(
    cmd: GoogleCommand,
    paths: &Paths,
    store: &mut Store,
    device: &Device,
) -> Result<()> {
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
            let plan = pick_plan(store, device, plan.as_deref())?.clone();
            let opts = ExportOpts {
                first_monday: start
                    .unwrap_or_else(|| export::next_monday(Local::now().date_naive())),
                weeks: weeks.unwrap_or(store.export.weeks),
                include_life: store.export.include_life,
            };
            let r = services::push_plan(paths, store, &plan, &opts).await?;
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

async fn health(
    cmd: HealthCommand,
    paths: &Paths,
    store: &mut Store,
    device: &Device,
) -> Result<()> {
    match cmd {
        HealthCommand::Weight { apply } => {
            let Some((kg, at)) = services::latest_weight(paths).await? else {
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
            let monday = services::week_monday(start.unwrap_or_else(|| Local::now().date_naive()));
            let workouts = services::week_workouts(paths, monday).await?;
            let plan = store.plan_or_first(device.active_plan.as_deref());
            let days = services::planned_vs_done(
                &store.library,
                plan,
                monday,
                &workouts,
                store.profile.weight,
            );
            println!("{} vs Google Health, week of {monday}", plan.name);
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

/// The named plan, or the active one.
fn pick_plan<'a>(store: &'a Store, device: &Device, name: Option<&str>) -> Result<&'a Plan> {
    match name {
        Some(n) => store
            .plan_by_name(n)
            .with_context(|| format!("no plan named {n:?}")),
        None => Ok(store.plan_or_first(device.active_plan.as_deref())),
    }
}

fn list_plans(store: &Store, device: &Device) {
    let active = &store.plan_or_first(device.active_plan.as_deref()).id;
    for p in &store.plans {
        let hours = calc::summary(&store.library, p).hours;
        let mark = if &p.id == active { "*" } else { " " };
        println!("{mark} {:<24} {hours:>5} h", p.name);
    }
}
