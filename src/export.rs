//! Calendar export: iCalendar with a weekly RRULE, and the artifact's Google Calendar CSV.

use anyhow::Result;
use chrono::{Days, NaiveDate, NaiveDateTime, TimeDelta, Weekday};
use csv::{QuoteStyle, Terminator, WriterBuilder};
use icalendar::{Calendar, Component, Event, EventLike};

use crate::{
    calc::guide,
    model::{Category, Kind, Plan, Store},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportOpts {
    /// Date of the plan's Monday in the first exported week.
    pub first_monday: NaiveDate,
    pub weeks: u32,
    pub include_life: bool,
}

/// One item placed in the first exported week. Timed events have `start` and `end`
/// (floating local time); all-day markers (duration 0) have neither.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportEvent {
    pub item_id: String,
    pub summary: String,
    pub description: String,
    pub start: Option<NaiveDateTime>,
    pub end: Option<NaiveDateTime>,
    pub date: NaiveDate,
}

/// The Monday after `today` (a week ahead when `today` is a Monday).
#[must_use]
pub fn next_monday(today: NaiveDate) -> NaiveDate {
    today.week(Weekday::Mon).first_day() + Days::new(7)
}

/// Exported items of the first week, day by day in plan order. Life items are left out
/// unless `include_life`, and so are all-day recovery markers other than "Rest day".
#[must_use]
pub fn events(store: &Store, plan: &Plan, opts: &ExportOpts) -> Vec<ExportEvent> {
    let lib = &store.library;
    let mut out = Vec::new();
    for (i, items) in plan.days.iter().enumerate() {
        let date = opts.first_monday + Days::new(i as u64);
        for it in items {
            let Some((t, e)) = lib.resolve(it) else {
                continue;
            };
            if t.category == Category::Life && !opts.include_life {
                continue;
            }
            if t.kind == Kind::Rest && it.dur == 0 && it.effort != "off" {
                continue;
            }
            let g = guide(lib, store.profile.weight, it);
            let description = [
                Some(format!("Effort: {}", e.label)),
                Some(it.notes.clone()),
                g.before.map(|x| format!("Before: {x}")),
                g.during.map(|x| format!("During: {x}")),
                g.after.map(|x| format!("After: {x}")),
            ]
            .into_iter()
            .flatten()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
            let (start, end) = if it.dur == 0 {
                (None, None)
            } else {
                let s = date.and_time(it.start);
                (Some(s), Some(s + TimeDelta::minutes(it.dur.into())))
            };
            out.push(ExportEvent {
                item_id: it.id.clone(),
                summary: format!("{}: {}", t.label, e.label),
                description,
                start,
                end,
                date,
            });
        }
    }
    out
}

/// An iCalendar named after the plan; each event repeats weekly `weeks` times.
#[must_use]
pub fn to_ics(plan: &Plan, events: &[ExportEvent], weeks: u32) -> String {
    let mut cal = Calendar::new();
    cal.name(&plan.name);
    for ev in events {
        let mut e = Event::new();
        e.uid(&format!("{}@tapas", ev.item_id))
            .summary(&ev.summary)
            .description(&ev.description);
        match (ev.start, ev.end) {
            (Some(s), Some(end)) => e.starts(s).ends(end),
            // DTEND of an all-day event is exclusive.
            _ => e.starts(ev.date).ends(ev.date + Days::new(1)),
        };
        if weeks > 1 {
            e.add_property("RRULE", format!("FREQ=WEEKLY;COUNT={weeks}"));
        }
        cal.push(e.done());
    }
    cal.done().to_string()
}

const CSV_HEADER: [&str; 9] = [
    "Subject",
    "Start Date",
    "Start Time",
    "End Date",
    "End Time",
    "All Day Event",
    "Description",
    "Location",
    "Private",
];

/// Google Calendar import CSV, one row per event per week, every field quoted, CRLF lines.
pub fn to_google_csv(events: &[ExportEvent], weeks: u32) -> Result<String> {
    const DATE: &str = "%m/%d/%Y";
    const TIME: &str = "%-I:%M %p";
    let mut w = WriterBuilder::new()
        .quote_style(QuoteStyle::Always)
        .terminator(Terminator::CRLF)
        .from_writer(Vec::new());
    w.write_record(CSV_HEADER)?;
    for week in 0..weeks {
        let shift = Days::new(7 * u64::from(week));
        for ev in events {
            let d = (ev.date + shift).format(DATE).to_string();
            let row = match (ev.start, ev.end) {
                (Some(s), Some(e)) => {
                    let (s, e) = (s + shift, e + shift);
                    [
                        ev.summary.clone(),
                        d,
                        s.format(TIME).to_string(),
                        e.format(DATE).to_string(),
                        e.format(TIME).to_string(),
                        "False".into(),
                        ev.description.clone(),
                        String::new(),
                        "True".into(),
                    ]
                }
                _ => [
                    ev.summary.clone(),
                    d.clone(),
                    String::new(),
                    d,
                    String::new(),
                    "True".into(),
                    ev.description.clone(),
                    String::new(),
                    "True".into(),
                ],
            };
            w.write_record(&row)?;
        }
    }
    Ok(String::from_utf8(w.into_inner()?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::starter_plan;
    use chrono::NaiveTime;

    fn setup(include_life: bool) -> (Store, Plan, ExportOpts) {
        let mut plan = starter_plan("Base");
        // A late session that runs past midnight.
        plan.days[6].push(crate::model::Item {
            start: NaiveTime::from_hms_opt(23, 30, 0).unwrap(),
            dur: 60,
            ..crate::library::new_item(&Store::default().library, "run").unwrap()
        });
        let opts = ExportOpts {
            first_monday: NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(),
            weeks: 3,
            include_life,
        };
        (Store::default(), plan, opts)
    }

    #[test]
    fn next_monday_is_strictly_after() {
        let d = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        assert_eq!(next_monday(d(9, 25)), d(9, 28));
        assert_eq!(next_monday(d(9, 27)), d(9, 28));
        assert_eq!(next_monday(d(9, 28)), d(10, 5));
    }

    #[test]
    fn skip_rules() {
        let (store, plan, opts) = setup(false);
        let ev = events(&store, &plan, &opts);
        // 22 starter items minus 9 life items, plus the late run.
        assert_eq!(ev.len(), 14);
        assert!(ev.iter().all(|e| !e.summary.starts_with("Work")));
        let with_life = events(
            &store,
            &plan,
            &ExportOpts {
                include_life: true,
                ..opts
            },
        );
        assert_eq!(with_life.len(), 23);
        let first = &ev[0];
        assert_eq!(first.summary, "Run: Run/walk");
        assert_eq!(
            first.description,
            "Effort: Run/walk\n10 × 1 min run / 1 min walk\nBefore: Fasted is fine, or a small snack.\nDuring: Water only.\nAfter: Normal next meal."
        );
    }

    #[test]
    fn ics_round_trip() {
        let (store, plan, opts) = setup(false);
        let ev = events(&store, &plan, &opts);
        let ics = to_ics(&plan, &ev, opts.weeks);
        let cal: Calendar = ics.parse().unwrap();
        assert_eq!(cal.get_name(), Some("Base"));
        let parsed: Vec<&Event> = cal.events().collect();
        assert_eq!(parsed.len(), ev.len());
        for (p, e) in parsed.iter().zip(&ev) {
            assert_eq!(p.get_uid(), Some(format!("{}@tapas", e.item_id).as_str()));
            assert_eq!(p.get_summary(), Some(e.summary.as_str()));
            assert_eq!(p.get_description(), Some(e.description.as_str()));
            assert_eq!(p.property_value("RRULE"), Some("FREQ=WEEKLY;COUNT=3"));
        }
        let late = parsed.last().unwrap();
        assert_eq!(late.property_value("DTSTART"), Some("20261004T233000"));
        assert_eq!(late.property_value("DTEND"), Some("20261005T003000"));
        assert!(!to_ics(&plan, &ev, 1).contains("RRULE"));
    }

    #[test]
    fn all_day_ics() {
        let store = Store::default();
        let mut plan = Plan::new("R");
        plan.days[2].push(crate::library::new_item(&store.library, "rest").unwrap());
        let opts = ExportOpts {
            first_monday: NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(),
            weeks: 1,
            include_life: false,
        };
        let ev = events(&store, &plan, &opts);
        let ics = to_ics(&plan, &ev, 1);
        assert!(ics.contains("DTSTART;VALUE=DATE:20260930"));
        assert!(ics.contains("DTEND;VALUE=DATE:20261001"));
        let csv = to_google_csv(&ev, 1).unwrap();
        assert!(csv.ends_with(
            "\"Recovery: Rest day\",\"09/30/2026\",\"\",\"09/30/2026\",\"\",\"True\",\"Effort: Rest day\nAfter: Aim for 8+ hours of sleep; this is where the adaptation happens.\",\"\",\"True\"\r\n"
        ));
    }

    #[test]
    fn csv_round_trip() {
        let (store, plan, opts) = setup(false);
        let ev = events(&store, &plan, &opts);
        let csv = to_google_csv(&ev, opts.weeks).unwrap();
        assert!(csv.starts_with("\"Subject\",\"Start Date\",\"Start Time\",\"End Date\",\"End Time\",\"All Day Event\",\"Description\",\"Location\",\"Private\"\r\n"));
        let mut r = csv::Reader::from_reader(csv.as_bytes());
        let rows: Vec<csv::StringRecord> = r.records().map(|x| x.unwrap()).collect();
        assert_eq!(rows.len(), ev.len() * 3);
        assert_eq!(
            &rows[0].iter().take(6).collect::<Vec<_>>(),
            &[
                "Run: Run/walk",
                "09/28/2026",
                "7:00 PM",
                "09/28/2026",
                "7:25 PM",
                "False"
            ]
        );
        // Second week of the first row.
        assert_eq!(&rows[ev.len()][1], "10/05/2026");
        let late = &rows[ev.len() - 1];
        assert_eq!(
            late.iter().skip(1).take(4).collect::<Vec<_>>(),
            ["10/04/2026", "11:30 PM", "10/05/2026", "12:30 AM"]
        );
        assert_eq!(&rows[0][6], ev[0].description);
    }
}
