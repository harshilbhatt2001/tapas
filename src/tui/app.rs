//! App state, event dispatch and background-task plumbing.

use std::{
    collections::BTreeSet, future::Future, path::PathBuf, sync::mpsc::Sender, time::Instant,
};

use anyhow::Result;
use chrono::{DateTime, Local, NaiveDate, Utc};
use ratatui::{
    crossterm::event::{Event, KeyCode, KeyEvent, MouseButton, MouseEventKind},
    layout::{Position, Rect},
    widgets::ListState,
};

use super::{editor, export, library, plans, profile, sync, week, widgets::Form};
use crate::{
    export as ex,
    google::{calendar::PushReport, health::Workout},
    model::{Device, Plan, Store},
    services::{self, SyncError},
    storage::{self, Paths},
    sync::Report,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Week,
    Plans,
    Library,
    Profile,
    Export,
}

impl Screen {
    pub const ALL: [Screen; 5] = [
        Screen::Week,
        Screen::Plans,
        Screen::Library,
        Screen::Profile,
        Screen::Export,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Screen::Week => "Week",
            Screen::Plans => "Plans",
            Screen::Library => "Library",
            Screen::Profile => "Profile",
            Screen::Export => "Export & Google",
        }
    }
    pub fn index(self) -> usize {
        Screen::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// Mutations that need a yes/no first.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    DeleteItem(String),
    LoadStarter,
    ClearWeek,
    DeletePlan(usize),
    DeleteType(usize),
    DeleteEffort(usize, usize),
    ResetLibrary,
    ApplyWeight(f64),
}

/// What a submitted form edits.
#[derive(Clone, Debug, PartialEq)]
pub enum FormKind {
    Item(String),
    NewPlan,
    RenamePlan(usize),
    /// Session type by index; `None` creates one.
    Type(Option<usize>),
    /// Effort of a type; `None` creates one.
    Effort(usize, Option<usize>),
    Profile,
    Export,
}

#[derive(Default)]
pub enum Modal {
    #[default]
    None,
    Help,
    Confirm {
        msg: String,
        action: Action,
    },
    /// Library type to add to the selected day.
    AddPicker(ListState),
    Form(Box<Form>, FormKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Task {
    Push,
    Weight,
    Workouts,
    /// Drive store sync; shown in the header, not the status line.
    Sync,
}

impl Task {
    fn label(self) -> &'static str {
        match self {
            Task::Push => "Pushing to Google Calendar",
            Task::Weight => "Fetching weight from Google Health",
            Task::Workouts => "Fetching workouts from Google Health",
            Task::Sync => "Syncing with Google Drive",
        }
    }
}

pub enum BgResult {
    Pushed {
        plan_id: String,
        res: Result<PushReport>,
    },
    Weight(Result<Option<(f64, DateTime<Utc>)>>),
    Workouts {
        monday: NaiveDate,
        res: Result<Vec<Workout>>,
    },
    /// `sent` is the store the sync started from, `store` what it left on disk.
    Synced {
        manual: bool,
        sent: Box<Store>,
        store: Box<Store>,
        res: Result<Report, SyncError>,
    },
}

impl BgResult {
    fn task(&self) -> Task {
        match self {
            BgResult::Pushed { .. } => Task::Push,
            BgResult::Weight(_) => Task::Weight,
            BgResult::Workouts { .. } => Task::Workouts,
            BgResult::Synced { .. } => Task::Sync,
        }
    }
}

pub struct Background {
    pub handle: tokio::runtime::Handle,
    pub tx: Sender<BgResult>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

/// A clickable region of the week board recorded while drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub area: Rect,
    pub day: usize,
    pub card: Option<usize>,
}

/// Outcome of the last Drive sync; a running one shows as `Task::Sync` in `App::busy`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncStatus {
    /// Not set up on this machine; says how to turn it on.
    Off(String),
    /// Set up; nothing finished yet.
    Idle,
    Synced(DateTime<Local>),
    /// Merged with edits from another machine; this many collided and one side won.
    Merged(usize),
    Offline,
    LoginNeeded,
    Error,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LibFocus {
    #[default]
    Types,
    Efforts,
}

pub struct App {
    pub paths: Paths,
    pub store: Store,
    pub device: Device,
    pub screen: Screen,
    pub modal: Modal,
    /// Selected day and card (index into the day sorted by start).
    pub day: usize,
    pub card: usize,
    pub plan_sel: usize,
    pub lib_type: usize,
    pub lib_effort: usize,
    pub lib_focus: LibFocus,
    pub first_monday: NaiveDate,
    pub last_file: Option<PathBuf>,
    /// Workouts fetched from Google Health for the week starting at the date.
    pub done: Option<(NaiveDate, Vec<Workout>)>,
    /// Show commute rides next to the Done line (off by default; `c` on the week screen).
    pub show_commutes: bool,
    pub status: Status,
    pub busy: BTreeSet<Task>,
    pub tick: usize,
    pub hits: Vec<Hit>,
    pub quit: bool,
    pub bg: Option<Background>,
    pub sync: SyncStatus,
    /// Edits saved here that are not on Drive yet, as far as this session knows.
    pub unsynced: bool,
    /// Push this long after the last edit.
    pub push_at: Option<Instant>,
    /// Try again after going offline.
    pub retry_at: Option<Instant>,
}

impl App {
    #[must_use]
    pub fn new(paths: Paths, store: Store, device: Device) -> App {
        let plan_sel = store.plan_index(device.active_plan.as_deref());
        let sync = services::sync_off(&paths).map_or(SyncStatus::Idle, SyncStatus::Off);
        App {
            paths,
            store,
            device,
            screen: Screen::Week,
            modal: Modal::None,
            day: 0,
            card: 0,
            plan_sel,
            lib_type: 0,
            lib_effort: 0,
            lib_focus: LibFocus::Types,
            first_monday: ex::next_monday(Local::now().date_naive()),
            last_file: None,
            done: None,
            show_commutes: false,
            status: Status {
                text: "Loaded. Press ? for keys.".into(),
                error: false,
            },
            busy: BTreeSet::new(),
            tick: 0,
            hits: Vec::new(),
            quit: false,
            bg: None,
            sync,
            unsynced: false,
            push_at: None,
            retry_at: None,
        }
    }

    /// The active plan: this device's choice, or the first plan.
    #[must_use]
    pub fn plan(&self) -> &Plan {
        self.store.plan_or_first(self.device.active_plan.as_deref())
    }

    pub fn plan_mut(&mut self) -> &mut Plan {
        self.store
            .plan_or_first_mut(self.device.active_plan.as_deref())
    }

    /// Index of the active plan in `store.plans`.
    #[must_use]
    pub fn active(&self) -> usize {
        self.store.plan_index(self.device.active_plan.as_deref())
    }

    /// Make plan `i` active on this device; saved by the next [`App::commit`].
    pub fn set_active(&mut self, i: usize) {
        self.device.active_plan = self.store.plans.get(i).map(|p| p.id.clone());
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Status {
            text: text.into(),
            error: false,
        };
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.status = Status {
            text: text.into(),
            error: true,
        };
    }

    /// Repair invariants and save; every mutation ends here.
    pub fn commit(&mut self) {
        self.store.normalize();
        self.clamp();
        let saved = storage::save(&self.paths, &mut self.store)
            .and_then(|()| storage::save_device(&self.paths, &self.device));
        match saved {
            Ok(()) => {
                self.info("Saved");
                sync::edited(self);
            }
            Err(e) => self.error(format!("Save failed: {e:#}")),
        }
    }

    /// Keep selections inside their lists.
    pub fn clamp(&mut self) {
        self.day = self.day.min(6);
        let n = self.plan().days[self.day].len();
        self.card = self.card.min(n.saturating_sub(1));
        self.plan_sel = self.plan_sel.min(self.store.plans.len() - 1);
        let types = &self.store.library.types;
        self.lib_type = self.lib_type.min(types.len().saturating_sub(1));
        let efforts = types.get(self.lib_type).map_or(0, |t| t.efforts.len());
        self.lib_effort = self.lib_effort.min(efforts.saturating_sub(1));
    }

    pub fn confirm(&mut self, msg: impl Into<String>, action: Action) {
        self.modal = Modal::Confirm {
            msg: msg.into(),
            action,
        };
    }

    pub fn open_form(&mut self, form: Form, kind: FormKind) {
        self.modal = Modal::Form(Box::new(form), kind);
    }

    /// Run `fut` on the runtime and deliver its result to [`App::on_bg`].
    pub fn spawn<F>(&mut self, task: Task, fut: F)
    where
        F: Future<Output = BgResult> + Send + 'static,
    {
        if self.busy.contains(&task) {
            self.info(format!("{}…", task.label()));
            return;
        }
        let Some(bg) = &self.bg else {
            self.error("No background runtime");
            return;
        };
        let tx = bg.tx.clone();
        bg.handle.spawn(async move {
            // The receiver is gone only when the app is quitting.
            let _ = tx.send(fut.await);
        });
        self.busy.insert(task);
    }

    /// Status line text for running tasks, with a spinner.
    #[must_use]
    pub fn busy_text(&self) -> Option<String> {
        const SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];
        let first = self.busy.iter().find(|t| **t != Task::Sync)?;
        Some(format!("{} {}…", SPIN[self.tick % 4], first.label()))
    }

    pub fn on_bg(&mut self, res: BgResult) {
        self.busy.remove(&res.task());
        match res {
            BgResult::Pushed { plan_id, res } => export::on_pushed(self, &plan_id, res),
            BgResult::Weight(res) => profile::on_weight(self, res),
            BgResult::Workouts { monday, res } => export::on_workouts(self, monday, res),
            BgResult::Synced {
                manual,
                sent,
                store,
                res,
            } => sync::on_synced(self, manual, &sent, &store, res),
        }
    }

    pub fn handle_event(&mut self, ev: &Event) {
        match ev {
            Event::Key(k) if super::widgets::is_press(k) => self.on_key(*k, ev),
            Event::Mouse(m)
                if m.kind == MouseEventKind::Down(MouseButton::Left)
                    && matches!(self.modal, Modal::None)
                    && self.screen == Screen::Week =>
            {
                self.on_click(Position::new(m.column, m.row));
            }
            _ => {}
        }
    }

    fn on_click(&mut self, at: Position) {
        let hit = self
            .hits
            .iter()
            .find(|h| h.card.is_some() && h.area.contains(at))
            .or_else(|| self.hits.iter().find(|h| h.area.contains(at)))
            .copied();
        if let Some(h) = hit {
            self.day = h.day;
            self.card = h.card.unwrap_or(0);
            self.clamp();
        }
    }

    fn on_key(&mut self, k: KeyEvent, ev: &Event) {
        match std::mem::take(&mut self.modal) {
            Modal::None => {}
            modal => {
                self.on_modal_key(modal, k, ev);
                return;
            }
        }
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Char(c @ '1'..='5') => {
                self.screen = Screen::ALL[c as usize - '1' as usize];
            }
            _ => match self.screen {
                Screen::Week => week::on_key(self, k),
                Screen::Plans => plans::on_key(self, k),
                Screen::Library => library::on_key(self, k),
                Screen::Profile => profile::on_key(self, k),
                Screen::Export => export::on_key(self, k),
            },
        }
    }

    fn on_modal_key(&mut self, modal: Modal, k: KeyEvent, ev: &Event) {
        match modal {
            Modal::None | Modal::Help => {}
            Modal::Confirm { msg, action } => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => self.run(action),
                KeyCode::Char('n') | KeyCode::Esc => self.info("Cancelled"),
                _ => self.modal = Modal::Confirm { msg, action },
            },
            Modal::AddPicker(mut state) => match k.code {
                KeyCode::Esc => {}
                KeyCode::Char('j') | KeyCode::Down => {
                    state.select_next();
                    self.modal = Modal::AddPicker(state);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    state.select_previous();
                    self.modal = Modal::AddPicker(state);
                }
                KeyCode::Enter => {
                    let n = self.store.library.types.len();
                    if let Some(i) = state.selected().filter(|_| n > 0) {
                        let key = self.store.library.types[i.min(n - 1)].key.clone();
                        week::add(self, &key);
                    }
                }
                _ => self.modal = Modal::AddPicker(state),
            },
            Modal::Form(mut form, kind) => {
                use super::widgets::FormEvent;
                match form.handle(ev) {
                    FormEvent::Cancel => {}
                    FormEvent::Submit => {
                        form.error = None;
                        if let Err(e) = self.submit(&form, &kind) {
                            form.error = Some(e);
                            self.modal = Modal::Form(form, kind);
                        }
                    }
                    FormEvent::Changed(i) => {
                        if let FormKind::Item(id) = &kind {
                            editor::on_change(self, &mut form, id, i);
                        }
                        self.modal = Modal::Form(form, kind);
                    }
                    FormEvent::None => self.modal = Modal::Form(form, kind),
                }
            }
        }
    }

    fn submit(&mut self, form: &Form, kind: &FormKind) -> Result<(), String> {
        match kind {
            FormKind::Item(id) => editor::apply(self, form, id),
            FormKind::NewPlan => plans::apply_new(self, form),
            FormKind::RenamePlan(i) => plans::apply_rename(self, form, *i),
            FormKind::Type(i) => library::apply_type(self, form, *i),
            FormKind::Effort(t, e) => library::apply_effort(self, form, *t, *e),
            FormKind::Profile => profile::apply(self, form),
            FormKind::Export => export::apply(self, form),
        }
    }

    fn run(&mut self, action: Action) {
        match action {
            Action::DeleteItem(id) => week::delete(self, &id),
            Action::LoadStarter => week::load_starter(self),
            Action::ClearWeek => week::clear(self),
            Action::DeletePlan(i) => plans::delete(self, i),
            Action::DeleteType(i) => library::delete_type(self, i),
            Action::DeleteEffort(t, e) => library::delete_effort(self, t, e),
            Action::ResetLibrary => library::reset(self),
            Action::ApplyWeight(kg) => profile::apply_weight(self, kg),
        }
    }
}
