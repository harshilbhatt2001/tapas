//! Integration tests for `export`: parse generated ICS and CSV back with the crates that
//! wrote them (`icalendar`, `csv`) rather than checking raw strings.

use chrono::NaiveDate;
use csv::StringRecord;
use icalendar::{Calendar, Component};
use tapas::export::{ExportEvent, range_events, to_google_csv, to_ics, week_events};
use tapas::library::{new_item, starter_plan};
use tapas::model::{Item, Store};

fn monday() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 28).unwrap()
}

/// A day before every exported week, so all items are as planned.
fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
}

fn events(store: &Store) -> Vec<ExportEvent> {
    week_events(store, monday(), None, today(), false)
}

/// A week with a single run item on Monday with notes containing a comma, a double quote and
/// a newline, the three characters CSV and iCalendar need to escape.
fn tricky_store() -> Store {
    let mut store = Store::default();
    let run = Item {
        notes: "Hills, \"repeats\"\nBring gels".into(),
        ..new_item(&store.library, "run").unwrap()
    };
    store.week_mut_or_insert(monday()).plan.days[0].push(run);
    store
}

#[test]
fn csv_notes_with_comma_quote_newline_round_trip() {
    let csv = to_google_csv(&events(&tricky_store())).unwrap();

    let mut r = csv::Reader::from_reader(csv.as_bytes());
    let rows: Vec<StringRecord> = r.records().map(|x| x.unwrap()).collect();
    assert_eq!(rows.len(), 1);
    // Description is column 6 (0-based) and must parse back to exactly what went in,
    // notes included, despite the comma/quote/newline inside it.
    let desc = &rows[0][6];
    assert!(desc.contains("Hills, \"repeats\"\nBring gels"));
}

#[test]
fn ics_notes_with_comma_quote_newline_round_trip() {
    let ics = to_ics("Training", &events(&tricky_store()));
    let cal: Calendar = ics.parse().unwrap();
    let parsed: Vec<_> = cal.events().collect();
    assert_eq!(parsed.len(), 1);
    let desc = parsed[0].get_description().unwrap();
    assert!(desc.contains("Hills, \"repeats\"\nBring gels"));
}

/// The same item always maps to the same ICS UID, across independent calls.
#[test]
fn uid_is_stable_across_calls() {
    let store = tricky_store();
    let uid = |ics: &str| -> String {
        let cal: Calendar = ics.parse().unwrap();
        cal.events().next().unwrap().get_uid().unwrap().to_string()
    };
    let u1 = uid(&to_ics("Training", &events(&store)));
    let u2 = uid(&to_ics("Training", &events(&store)));
    assert_eq!(u1, u2);
    let id = &store.weeks[0].plan.days[0][0].id;
    assert_eq!(u1, format!("{}-item-{id}@tapas", monday()));
}

/// Applied weeks export one event per item and week, never a recurrence.
#[test]
fn applied_weeks_export_single_events() {
    let mut store = Store {
        plans: vec![starter_plan("Base")],
        ..Store::default()
    };
    let id = store.plans[0].id.clone();
    assert!(store.apply_template(&id, monday(), 4));
    let ev = range_events(&store, monday(), 4, |_| None, today(), false);
    assert_eq!(ev.len(), 4 * 13);
    let ics = to_ics("Training", &ev);
    assert!(!ics.contains("RRULE"));
    let cal: Calendar = ics.parse().unwrap();
    assert_eq!(cal.events().count(), ev.len());
    let csv = to_google_csv(&ev).unwrap();
    assert_eq!(
        csv::Reader::from_reader(csv.as_bytes()).records().count(),
        ev.len()
    );
}

/// All-day markers get an exclusive-end DTEND of the next day.
#[test]
fn all_day_dtend_is_exclusive_next_day() {
    let mut store = Store::default();
    let rest = new_item(&store.library, "rest").unwrap(); // "off", dur 0
    store.week_mut_or_insert(monday()).plan.days[0].push(rest);
    let ics = to_ics("Training", &events(&store));
    let cal: Calendar = ics.parse().unwrap();
    let e = cal.events().next().unwrap();
    assert_eq!(
        e.get_start(),
        Some(icalendar::DatePerhapsTime::Date(monday()))
    );
    assert_eq!(
        e.get_end(),
        Some(icalendar::DatePerhapsTime::Date(
            NaiveDate::from_ymd_opt(2026, 9, 29).unwrap()
        ))
    );
}
