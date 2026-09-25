//! Integration tests for `export`: parse generated ICS and CSV back with the crates that
//! wrote them (`icalendar`, `csv`) rather than checking raw strings.

use chrono::NaiveDate;
use csv::StringRecord;
use icalendar::{Calendar, Component};
use tapas::export::{ExportOpts, events, to_google_csv, to_ics};
use tapas::library::new_item;
use tapas::model::{Item, Plan, Store};

fn opts() -> ExportOpts {
    ExportOpts {
        first_monday: NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(),
        weeks: 1,
        include_life: false,
    }
}

/// A single run item on Monday with notes containing a comma, a double quote and a newline,
/// the three characters CSV and iCalendar need to escape.
fn tricky_plan() -> (Store, Plan) {
    let store = Store::default();
    let mut plan = Plan::new("Tricky");
    plan.days[0].push(Item {
        notes: "Hills, \"repeats\"\nBring gels".into(),
        ..new_item(&store.library, "run").unwrap()
    });
    (store, plan)
}

#[test]
fn csv_notes_with_comma_quote_newline_round_trip() {
    let (store, plan) = tricky_plan();
    let opts = opts();
    let ev = events(&store, &plan, &opts);
    let csv = to_google_csv(&ev, opts.weeks).unwrap();

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
    let (store, plan) = tricky_plan();
    let opts = opts();
    let ev = events(&store, &plan, &opts);
    let ics = to_ics(&plan, &ev, 1);
    let cal: Calendar = ics.parse().unwrap();
    let parsed: Vec<_> = cal.events().collect();
    assert_eq!(parsed.len(), 1);
    let desc = parsed[0].get_description().unwrap();
    assert!(desc.contains("Hills, \"repeats\"\nBring gels"));
}

/// The same item id always maps to the same ICS UID, across independent calls.
#[test]
fn uid_is_stable_across_calls() {
    let (store, plan) = tricky_plan();
    let opts = opts();
    let ev1 = events(&store, &plan, &opts);
    let ev2 = events(&store, &plan, &opts);
    let ics1 = to_ics(&plan, &ev1, 1);
    let ics2 = to_ics(&plan, &ev2, 1);
    let uid = |ics: &str| -> String {
        let cal: Calendar = ics.parse().unwrap();
        cal.events().next().unwrap().get_uid().unwrap().to_string()
    };
    let u1 = uid(&ics1);
    let u2 = uid(&ics2);
    assert_eq!(u1, u2);
    assert_eq!(u1, format!("{}@tapas", plan.days[0][0].id));
}

/// RRULE with COUNT only appears when exporting more than one week.
#[test]
fn rrule_count_only_when_weeks_gt_1() {
    let (store, plan) = tricky_plan();
    let one_week = to_ics(&plan, &events(&store, &plan, &opts()), 1);
    assert!(!one_week.contains("RRULE"));

    let four_weeks_opts = ExportOpts { weeks: 4, ..opts() };
    let ev = events(&store, &plan, &four_weeks_opts);
    let four_weeks = to_ics(&plan, &ev, 4);
    let cal: Calendar = four_weeks.parse().unwrap();
    let ev0 = cal.events().next().unwrap();
    assert_eq!(ev0.property_value("RRULE"), Some("FREQ=WEEKLY;COUNT=4"));
}

/// All-day markers get an exclusive-end DTEND of the next day; timed events crossing
/// midnight land on the following calendar date.
#[test]
fn all_day_dtend_is_exclusive_next_day() {
    let store = Store::default();
    let mut plan = Plan::new("R");
    plan.days[0].push(new_item(&store.library, "rest").unwrap()); // "off", dur 0
    let opts = opts();
    let ev = events(&store, &plan, &opts);
    let ics = to_ics(&plan, &ev, 1);
    let cal: Calendar = ics.parse().unwrap();
    let e = cal.events().next().unwrap();
    assert_eq!(
        e.get_start(),
        Some(icalendar::DatePerhapsTime::Date(
            NaiveDate::from_ymd_opt(2026, 9, 28).unwrap()
        ))
    );
    assert_eq!(
        e.get_end(),
        Some(icalendar::DatePerhapsTime::Date(
            NaiveDate::from_ymd_opt(2026, 9, 29).unwrap()
        ))
    );
}
