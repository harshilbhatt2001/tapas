//! Push dated weeks to an app-created Google Calendar: one event per item or workout, tagged
//! with its week and key, brought up to date by a diff per week.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use google_calendar3::api::{Calendar, Event, EventDateTime, EventExtendedProperties};
use google_calendar3::{CalendarHub, Error as ApiError, hyper_util};

use super::auth::{self, Auth, CALENDAR_SCOPE};
use crate::export::{EventKey, ExportEvent};

const WEEK_KEY: &str = "tapasWeek";
const ITEM_KEY: &str = "tapasItem";
const WORKOUT_KEY: &str = "tapasWorkout";

/// Start or end of an event, comparable between what tapas wants and what Google holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventTime {
    /// All-day; an end date is exclusive.
    Date(NaiveDate),
    At(DateTime<Utc>),
}

impl EventTime {
    fn of_api(t: &EventDateTime) -> Option<EventTime> {
        t.date_time
            .map(EventTime::At)
            .or_else(|| t.date.map(EventTime::Date))
    }

    fn to_api(self, tz: &str) -> EventDateTime {
        match self {
            EventTime::Date(date) => EventDateTime {
                date: Some(date),
                ..Default::default()
            },
            EventTime::At(at) => EventDateTime {
                date_time: Some(at),
                time_zone: Some(tz.to_owned()),
                ..Default::default()
            },
        }
    }
}

/// A tagged event already in the calendar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExistingEvent {
    pub event_id: String,
    /// `None` when the event carries no item or workout tag.
    pub key: Option<EventKey>,
    pub summary: String,
    pub description: String,
    pub start: EventTime,
    pub end: EventTime,
}

impl ExistingEvent {
    /// `None` for events without an id or times, which Google only returns when cancelled.
    fn of_api(e: Event) -> Option<ExistingEvent> {
        let private = e.extended_properties.and_then(|p| p.private);
        let tag = |name: &str| private.as_ref().and_then(|p| p.get(name)).cloned();
        let key = tag(ITEM_KEY)
            .map(EventKey::Item)
            .or_else(|| tag(WORKOUT_KEY).map(EventKey::Workout));
        Some(ExistingEvent {
            event_id: e.id?,
            key,
            summary: e.summary.unwrap_or_default(),
            description: e.description.unwrap_or_default(),
            start: EventTime::of_api(e.start.as_ref()?)?,
            end: EventTime::of_api(e.end.as_ref()?)?,
        })
    }
}

/// One call that brings a week's events in line with what tapas wants.
#[derive(Clone, Debug)]
pub enum Change {
    Insert(Event),
    /// Replaces the whole event.
    Update {
        event_id: String,
        event: Event,
    },
    Delete {
        event_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushReport {
    /// Calendar that received the events, new when the given one was missing.
    pub calendar_id: String,
    pub inserted: usize,
    pub updated: usize,
    pub deleted: usize,
    pub unchanged: usize,
}

fn parse_zone(tz: &str) -> Result<Tz> {
    tz.parse().map_err(|e| anyhow!("time zone {tz}: {e}"))
}

/// Start and end of `ev` in `zone`; all-day events end the next day.
fn event_times(ev: &ExportEvent, zone: Tz) -> Result<(EventTime, EventTime)> {
    let Some(start) = ev.start else {
        return Ok((
            EventTime::Date(ev.date),
            EventTime::Date(ev.date + Days::new(1)),
        ));
    };
    let at = |t: NaiveDateTime| -> Result<EventTime> {
        let utc = zone
            .from_local_datetime(&t)
            .earliest()
            .with_context(|| format!("{t} does not exist in {zone}"))?
            .with_timezone(&Utc);
        Ok(EventTime::At(utc))
    };
    Ok((at(start)?, at(ev.end.unwrap_or(start))?))
}

fn api_event(ev: &ExportEvent, (start, end): (EventTime, EventTime), tz: &str) -> Event {
    let (key_name, id) = match &ev.key {
        EventKey::Item(id) => (ITEM_KEY, id),
        EventKey::Workout(id) => (WORKOUT_KEY, id),
    };
    let private = HashMap::from([
        (WEEK_KEY.to_owned(), ev.week.to_string()),
        (key_name.to_owned(), id.clone()),
    ]);
    Event {
        summary: Some(ev.summary.clone()),
        description: Some(ev.description.clone()),
        start: Some(start.to_api(tz)),
        end: Some(end.to_api(tz)),
        extended_properties: Some(EventExtendedProperties {
            private: Some(private),
            shared: None,
        }),
        ..Default::default()
    }
}

/// `ev` as a Calendar API event in time zone `tz`, tagged with its week and key.
pub fn to_event(ev: &ExportEvent, tz: &str) -> Result<Event> {
    let times = event_times(ev, parse_zone(tz)?)?;
    Ok(api_event(ev, times, tz))
}

/// The changes that turn a week's `existing` events into `wanted`, matched by key. Existing
/// events without a key, with a key no longer wanted, or repeating an earlier event's key are
/// deleted.
pub fn plan_changes(
    existing: &[ExistingEvent],
    wanted: &[ExportEvent],
    tz: &str,
) -> Result<Vec<Change>> {
    let zone = parse_zone(tz)?;
    let wanted_keys: HashSet<&EventKey> = wanted.iter().map(|w| &w.key).collect();
    let mut kept: HashMap<&EventKey, &ExistingEvent> = HashMap::new();
    let mut changes = Vec::new();
    for ex in existing {
        match &ex.key {
            Some(key) if wanted_keys.contains(key) && !kept.contains_key(key) => {
                kept.insert(key, ex);
            }
            _ => changes.push(Change::Delete {
                event_id: ex.event_id.clone(),
            }),
        }
    }
    for ev in wanted {
        let times = event_times(ev, zone)?;
        match kept.get(&ev.key) {
            None => changes.push(Change::Insert(api_event(ev, times, tz))),
            Some(ex)
                if ex.summary == ev.summary
                    && ex.description == ev.description
                    && (ex.start, ex.end) == times => {}
            Some(ex) => changes.push(Change::Update {
                event_id: ex.event_id.clone(),
                event: api_event(ev, times, tz),
            }),
        }
    }
    Ok(changes)
}

type Hub = CalendarHub<auth::Connector>;

fn is_not_found(err: &ApiError) -> bool {
    match err {
        ApiError::BadRequest(v) => matches!(v["error"]["code"].as_u64(), Some(404 | 410)),
        ApiError::Failure(res) => matches!(res.status().as_u16(), 404 | 410),
        _ => false,
    }
}

/// Existing calendar id if it still exists, else a freshly created calendar's id.
async fn ensure_calendar(hub: &Hub, name: &str, id: Option<&str>, tz: &str) -> Result<String> {
    if let Some(id) = id {
        match hub
            .calendars()
            .get(id)
            .add_scope(CALENDAR_SCOPE)
            .doit()
            .await
        {
            Ok(_) => return Ok(id.to_owned()),
            Err(e) if is_not_found(&e) => {}
            Err(e) => return Err(e).context("looking up calendar"),
        }
    }
    let cal = Calendar {
        summary: Some(name.to_owned()),
        time_zone: Some(tz.to_owned()),
        ..Default::default()
    };
    let (_, cal) = hub
        .calendars()
        .insert(cal)
        .add_scope(CALENDAR_SCOPE)
        .doit()
        .await
        .context("creating calendar")?;
    cal.id.context("created calendar has no id")
}

/// Every event tagged with the week of `monday`.
async fn week_events(hub: &Hub, calendar_id: &str, monday: NaiveDate) -> Result<Vec<Event>> {
    let tag = format!("{WEEK_KEY}={monday}");
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    loop {
        let mut call = hub
            .events()
            .list(calendar_id)
            .add_private_extended_property(&tag)
            .max_results(2500)
            .add_scope(CALENDAR_SCOPE);
        if let Some(p) = &page {
            call = call.page_token(p);
        }
        let (_, events) = call.doit().await.context("listing the week's events")?;
        out.extend(events.items.into_iter().flatten());
        page = events.next_page_token;
        if page.is_none() {
            return Ok(out);
        }
    }
}

async fn apply(hub: &Hub, calendar_id: &str, change: Change) -> Result<()> {
    let events = hub.events();
    match change {
        Change::Insert(ev) => {
            events
                .insert(ev, calendar_id)
                .add_scope(CALENDAR_SCOPE)
                .doit()
                .await
                .context("creating event")?;
        }
        Change::Update { event_id, event } => {
            events
                .update(event, calendar_id, &event_id)
                .add_scope(CALENDAR_SCOPE)
                .doit()
                .await
                .context("updating event")?;
        }
        Change::Delete { event_id } => {
            match events
                .delete(calendar_id, &event_id)
                .add_scope(CALENDAR_SCOPE)
                .doit()
                .await
            {
                Ok(_) => {}
                Err(e) if is_not_found(&e) => {}
                Err(e) => return Err(e).context("deleting event"),
            }
        }
    }
    Ok(())
}

/// Bring each week's tagged events in the tapas calendar in line with its wanted events; an
/// empty week loses all of its events.
pub async fn push_weeks(
    auth: &Auth,
    calendar_name: &str,
    calendar_id: Option<&str>,
    weeks: &[(NaiveDate, Vec<ExportEvent>)],
    tz: &str,
) -> Result<PushReport> {
    parse_zone(tz)?;
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(auth::connector()?);
    let hub = CalendarHub::new(client, auth.clone());
    let calendar_id = ensure_calendar(&hub, calendar_name, calendar_id, tz).await?;
    let mut report = PushReport {
        calendar_id,
        inserted: 0,
        updated: 0,
        deleted: 0,
        unchanged: 0,
    };
    for (monday, wanted) in weeks {
        let existing: Vec<ExistingEvent> = week_events(&hub, &report.calendar_id, *monday)
            .await?
            .into_iter()
            .filter_map(ExistingEvent::of_api)
            .collect();
        let changes = plan_changes(&existing, wanted, tz)?;
        let mut touched = 0;
        for change in changes {
            match &change {
                Change::Insert(_) => report.inserted += 1,
                Change::Update { .. } => report.updated += 1,
                Change::Delete { .. } => report.deleted += 1,
            }
            if !matches!(change, Change::Delete { .. }) {
                touched += 1;
            }
            apply(&hub, &report.calendar_id, change).await?;
        }
        report.unchanged += wanted.len() - touched;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BERLIN: &str = "Europe/Berlin";

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn utc(s: &str) -> EventTime {
        EventTime::At(dt(s).and_utc())
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 2).unwrap()
    }

    fn ev(key: &str, start: Option<&str>, end: Option<&str>) -> ExportEvent {
        ExportEvent {
            week: date(),
            key: EventKey::Item(key.into()),
            summary: "Run: Easy".into(),
            description: "Zone 2".into(),
            date: date(),
            start: start.map(dt),
            end: end.map(dt),
        }
    }

    fn timed(key: &str) -> ExportEvent {
        ev(key, Some("2026-03-02 07:00"), Some("2026-03-02 08:00"))
    }

    /// What Google holds for `ev` pushed in Berlin, as event `id`.
    fn existing(id: &str, ev: &ExportEvent) -> ExistingEvent {
        let (start, end) = event_times(ev, BERLIN.parse().unwrap()).unwrap();
        ExistingEvent {
            event_id: id.into(),
            key: Some(ev.key.clone()),
            summary: ev.summary.clone(),
            description: ev.description.clone(),
            start,
            end,
        }
    }

    fn plan(existing: &[ExistingEvent], wanted: &[ExportEvent]) -> Vec<Change> {
        plan_changes(existing, wanted, BERLIN).unwrap()
    }

    fn updated_ids(changes: &[Change]) -> Vec<&str> {
        changes
            .iter()
            .filter_map(|c| match c {
                Change::Update { event_id, .. } => Some(event_id.as_str()),
                _ => None,
            })
            .collect()
    }

    fn deleted_ids(changes: &[Change]) -> Vec<&str> {
        changes
            .iter()
            .filter_map(|c| match c {
                Change::Delete { event_id } => Some(event_id.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn timed_event_uses_zone_and_tags() {
        let e = to_event(&timed("item-1"), BERLIN).unwrap();
        let start = e.start.unwrap();
        assert_eq!(
            start.date_time.unwrap().to_rfc3339(),
            "2026-03-02T06:00:00+00:00"
        );
        assert_eq!(start.time_zone.as_deref(), Some(BERLIN));
        assert!(start.date.is_none());
        assert_eq!(
            e.end.unwrap().date_time.unwrap().to_rfc3339(),
            "2026-03-02T07:00:00+00:00"
        );
        assert!(e.recurrence.is_none());
        let private = e.extended_properties.unwrap().private.unwrap();
        assert_eq!(private.len(), 2);
        assert_eq!(private["tapasWeek"], "2026-03-02");
        assert_eq!(private["tapasItem"], "item-1");
        assert_eq!(e.summary.as_deref(), Some("Run: Easy"));
        assert_eq!(e.description.as_deref(), Some("Zone 2"));
    }

    #[test]
    fn workout_event_is_tagged_with_the_workout() {
        let workout = ExportEvent {
            key: EventKey::Workout("w-1".into()),
            ..timed("")
        };
        let private = to_event(&workout, BERLIN)
            .unwrap()
            .extended_properties
            .unwrap()
            .private
            .unwrap();
        assert_eq!(private["tapasWorkout"], "w-1");
        assert!(!private.contains_key("tapasItem"));
    }

    #[test]
    fn all_day_event_ends_next_day() {
        let e = to_event(&ev("item-1", None, None), BERLIN).unwrap();
        let (start, end) = (e.start.unwrap(), e.end.unwrap());
        assert_eq!(start.date, Some(date()));
        assert_eq!(end.date, NaiveDate::from_ymd_opt(2026, 3, 3));
        assert!(start.date_time.is_none() && start.time_zone.is_none());
    }

    #[test]
    fn bad_zone_is_an_error() {
        assert!(to_event(&timed("a"), "Mars/Olympus").is_err());
        assert!(plan_changes(&[], &[timed("a")], "Mars/Olympus").is_err());
    }

    #[test]
    fn unchanged_events_need_nothing() {
        let wanted = [timed("a"), ev("b", None, None)];
        let have = [existing("1", &wanted[0]), existing("2", &wanted[1])];
        assert!(plan(&have, &wanted).is_empty());
    }

    #[test]
    fn moved_or_renamed_events_are_updated() {
        let old = timed("a");
        let have = [existing("1", &old), existing("2", &timed("b"))];
        let moved = ev("a", Some("2026-03-02 07:15"), Some("2026-03-02 08:00"));
        let renamed = ExportEvent {
            summary: "✓ Run 45m (plan Easy 40m)".into(),
            ..timed("b")
        };
        let changes = plan(&have, &[moved, renamed]);
        assert_eq!(updated_ids(&changes), ["1", "2"]);
        let Change::Update { event, .. } = &changes[1] else {
            unreachable!()
        };
        assert_eq!(event.summary.as_deref(), Some("✓ Run 45m (plan Easy 40m)"));
    }

    #[test]
    fn new_events_are_inserted_and_gone_ones_deleted() {
        let have = [
            existing("1", &timed("gone")),
            ExistingEvent {
                key: None,
                ..existing("2", &timed("x"))
            },
        ];
        let changes = plan(&have, &[timed("new")]);
        assert_eq!(deleted_ids(&changes), ["1", "2"]);
        assert!(matches!(
            &changes[2],
            Change::Insert(e) if e.extended_properties.as_ref().unwrap().private.as_ref().unwrap()["tapasItem"] == "new"
        ));
        assert_eq!(changes.len(), 3);
    }

    #[test]
    fn duplicate_keys_keep_the_first() {
        let wanted = timed("a");
        let have = [existing("1", &wanted), existing("2", &wanted)];
        let changes = plan(&have, &[wanted]);
        assert_eq!(deleted_ids(&changes), ["2"]);
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn times_compare_as_instants_and_all_day_differs_from_timed() {
        let wanted = timed("a");
        let berlin = ExistingEvent {
            start: utc("2026-03-02 06:00"),
            end: utc("2026-03-02 07:00"),
            ..existing("1", &wanted)
        };
        assert!(plan(std::slice::from_ref(&berlin), std::slice::from_ref(&wanted)).is_empty());
        let in_utc = plan_changes(
            std::slice::from_ref(&berlin),
            std::slice::from_ref(&wanted),
            "UTC",
        );
        assert_eq!(updated_ids(&in_utc.unwrap()), ["1"]);

        let all_day = ExistingEvent {
            start: EventTime::Date(date()),
            end: EventTime::Date(date() + Days::new(1)),
            ..berlin
        };
        assert_eq!(
            updated_ids(&plan(std::slice::from_ref(&all_day), &[wanted])),
            ["1"]
        );
        assert!(plan(&[all_day], &[ev("a", None, None)]).is_empty());
    }
}
