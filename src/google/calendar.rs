//! Push a plan's first week to an app-created Google Calendar as weekly recurring events.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use chrono::{Days, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use google_calendar3::api::{Calendar, Event, EventDateTime, EventExtendedProperties};
use google_calendar3::{CalendarHub, Error as ApiError, hyper_util};

use super::auth::{self, Auth, CALENDAR_SCOPE};

const PLAN_KEY: &str = "tapasPlan";
const ITEM_KEY: &str = "tapasItem";

/// One event in the first week; all-day when `start` is `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct CalEvent {
    pub item_id: String,
    pub summary: String,
    pub description: String,
    pub date: NaiveDate,
    pub start: Option<NaiveDateTime>,
    pub end: Option<NaiveDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushReport {
    /// Calendar that received the events, new when the given one was missing.
    pub calendar_id: String,
    pub deleted: usize,
    pub created: usize,
}

/// Convert a [`CalEvent`] to a Calendar API event tagged with the plan and item ids.
pub fn to_event(ev: &CalEvent, plan_id: &str, weeks: u32, tz: &str) -> Result<Event> {
    let (start, end) = if let (Some(start), end) = (ev.start, ev.end) {
        let zone: Tz = tz.parse().map_err(|e| anyhow!("time zone {tz}: {e}"))?;
        let at = |t: NaiveDateTime| -> Result<EventDateTime> {
            let utc = zone
                .from_local_datetime(&t)
                .earliest()
                .with_context(|| format!("{t} does not exist in {tz}"))?
                .with_timezone(&Utc);
            Ok(EventDateTime {
                date_time: Some(utc),
                time_zone: Some(tz.to_owned()),
                ..Default::default()
            })
        };
        (at(start)?, at(end.unwrap_or(start))?)
    } else {
        let day = |d: NaiveDate| EventDateTime {
            date: Some(d),
            ..Default::default()
        };
        (day(ev.date), day(ev.date + Days::new(1)))
    };
    let private = HashMap::from([
        (PLAN_KEY.to_owned(), plan_id.to_owned()),
        (ITEM_KEY.to_owned(), ev.item_id.clone()),
    ]);
    Ok(Event {
        summary: Some(ev.summary.clone()),
        description: (!ev.description.is_empty()).then(|| ev.description.clone()),
        start: Some(start),
        end: Some(end),
        recurrence: (weeks > 1).then(|| vec![format!("RRULE:FREQ=WEEKLY;COUNT={weeks}")]),
        extended_properties: Some(EventExtendedProperties {
            private: Some(private),
            shared: None,
        }),
        ..Default::default()
    })
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

/// Ids of every event tagged with this plan.
async fn plan_event_ids(hub: &Hub, calendar_id: &str, plan_id: &str) -> Result<Vec<String>> {
    let tag = format!("{PLAN_KEY}={plan_id}");
    let mut ids = Vec::new();
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
        let (_, events) = call.doit().await.context("listing plan events")?;
        ids.extend(events.items.into_iter().flatten().filter_map(|e| e.id));
        page = events.next_page_token;
        if page.is_none() {
            return Ok(ids);
        }
    }
}

/// Replace every event of `plan_id` in the tapas calendar with `events`, repeating `weeks` times.
pub async fn push(
    auth: &Auth,
    calendar_name: &str,
    calendar_id: Option<&str>,
    plan_id: &str,
    events: &[CalEvent],
    weeks: u32,
    tz: &str,
) -> Result<PushReport> {
    // Convert first so a bad time zone fails before anything is deleted.
    let new_events = events
        .iter()
        .map(|e| to_event(e, plan_id, weeks, tz))
        .collect::<Result<Vec<_>>>()?;
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(auth::connector()?);
    let hub = CalendarHub::new(client, auth.clone());

    let calendar_id = ensure_calendar(&hub, calendar_name, calendar_id, tz).await?;
    let old = plan_event_ids(&hub, &calendar_id, plan_id).await?;
    for id in &old {
        match hub
            .events()
            .delete(&calendar_id, id)
            .add_scope(CALENDAR_SCOPE)
            .doit()
            .await
        {
            Ok(_) => {}
            Err(e) if is_not_found(&e) => {}
            Err(e) => return Err(e).context("deleting old event"),
        }
    }
    for ev in new_events {
        hub.events()
            .insert(ev, &calendar_id)
            .add_scope(CALENDAR_SCOPE)
            .doit()
            .await
            .context("creating event")?;
    }
    Ok(PushReport {
        calendar_id,
        deleted: old.len(),
        created: events.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(start: Option<&str>, end: Option<&str>) -> CalEvent {
        let dt = |s: &str| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        CalEvent {
            item_id: "item-1".into(),
            summary: "Run: Easy".into(),
            description: "Zone 2".into(),
            date: NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(),
            start: start.map(dt),
            end: end.map(dt),
        }
    }

    #[test]
    fn timed_event_uses_zone_and_rrule() {
        let e = to_event(
            &ev(Some("2026-03-02 07:00"), Some("2026-03-02 08:00")),
            "plan-1",
            4,
            "Europe/Berlin",
        )
        .unwrap();
        let start = e.start.unwrap();
        assert_eq!(
            start.date_time.unwrap().to_rfc3339(),
            "2026-03-02T06:00:00+00:00"
        );
        assert_eq!(start.time_zone.as_deref(), Some("Europe/Berlin"));
        assert!(start.date.is_none());
        assert_eq!(
            e.end.unwrap().date_time.unwrap().to_rfc3339(),
            "2026-03-02T07:00:00+00:00"
        );
        assert_eq!(e.recurrence.unwrap(), vec!["RRULE:FREQ=WEEKLY;COUNT=4"]);
        let private = e.extended_properties.unwrap().private.unwrap();
        assert_eq!(private["tapasPlan"], "plan-1");
        assert_eq!(private["tapasItem"], "item-1");
        assert_eq!(e.summary.as_deref(), Some("Run: Easy"));
        assert_eq!(e.description.as_deref(), Some("Zone 2"));
    }

    #[test]
    fn all_day_event_ends_next_day_without_rrule_for_one_week() {
        let e = to_event(&ev(None, None), "plan-1", 1, "Europe/Berlin").unwrap();
        let (start, end) = (e.start.unwrap(), e.end.unwrap());
        assert_eq!(start.date, NaiveDate::from_ymd_opt(2026, 3, 2));
        assert_eq!(end.date, NaiveDate::from_ymd_opt(2026, 3, 3));
        assert!(start.date_time.is_none() && start.time_zone.is_none());
        assert!(e.recurrence.is_none());
    }

    #[test]
    fn bad_zone_is_an_error() {
        assert!(to_event(&ev(Some("2026-03-02 07:00"), None), "p", 1, "Mars/Olympus").is_err());
    }
}
