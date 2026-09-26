//! Property tests for the pure three-way merge (`tapas::sync::merge`).
//!
//! Random "edit scripts" are applied to a shared base [`Store`] to produce `local` and `remote`
//! copies, mirroring how two machines diverge between syncs. Properties hold regardless of what
//! the scripts did, so failures shrink to a small script rather than one hand-picked case.

use std::collections::HashSet;

use chrono::{DateTime, Days, NaiveDate};
use proptest::prelude::*;
use tapas::{
    library::{new_item, starter_plan},
    model::{Store, Week},
    sync::merge::merge,
};
use uuid::Uuid;

const LOCAL: Uuid = Uuid::from_u128(11);
const REMOTE: Uuid = Uuid::from_u128(22);

/// Number of distinct Mondays scripts can touch: two already hold a dated week in the base
/// store, one is always empty so ops can exercise creating a week from scratch.
const N_MONDAYS: usize = 3;

fn monday_at(i: usize) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 21).unwrap() + Days::new(7 * i as u64)
}

/// Two plans with the starter week's items spread across every day, plus a dated week already
/// applied from each template, so scripts have something to edit, move, delete or add to from
/// the very first op, in both templates and dated weeks.
fn base_store() -> Store {
    let mut s = Store {
        plans: vec![starter_plan("Base"), starter_plan("Build")],
        ..Store::default()
    };
    let base_id = s.plans[0].id.clone();
    let build_id = s.plans[1].id.clone();
    assert!(s.apply_template(&base_id, monday_at(0), 1));
    assert!(s.apply_template(&build_id, monday_at(1), 1));
    s
}

/// One random edit. Indices are taken modulo whatever exists when the op runs, so every value
/// is valid input: out-of-range ops just wrap instead of being rejected by the strategy.
#[derive(Clone, Debug)]
enum Op {
    EditNotes(usize, usize, usize, String),
    Move(usize, usize, usize, usize),
    Add(usize, usize, usize),
    Delete(usize, usize, usize),
    RenamePlan(usize, String),
    SetWeight(f64),
    EditWeekNotes(usize, usize, usize, String),
    MoveWeekItem(usize, usize, usize, usize),
    AddWeekItem(usize, usize, usize),
    DeleteWeekItem(usize, usize, usize),
    ApplyTemplate(usize, usize),
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (
            any::<usize>(),
            any::<usize>(),
            any::<usize>(),
            "[A-Za-z ]{0,12}"
        )
            .prop_map(|(p, d, i, t)| Op::EditNotes(p, d, i, t)),
        (
            any::<usize>(),
            any::<usize>(),
            any::<usize>(),
            any::<usize>()
        )
            .prop_map(|(p, d, i, to)| Op::Move(p, d, i, to)),
        (any::<usize>(), any::<usize>(), any::<usize>()).prop_map(|(p, d, t)| Op::Add(p, d, t)),
        (any::<usize>(), any::<usize>(), any::<usize>()).prop_map(|(p, d, i)| Op::Delete(p, d, i)),
        (any::<usize>(), "[A-Za-z ]{1,12}").prop_map(|(p, n)| Op::RenamePlan(p, n)),
        (40.0f64..120.0).prop_map(Op::SetWeight),
        (
            any::<usize>(),
            any::<usize>(),
            any::<usize>(),
            "[A-Za-z ]{0,12}"
        )
            .prop_map(|(m, d, i, t)| Op::EditWeekNotes(m, d, i, t)),
        (
            any::<usize>(),
            any::<usize>(),
            any::<usize>(),
            any::<usize>()
        )
            .prop_map(|(m, d, i, to)| Op::MoveWeekItem(m, d, i, to)),
        (any::<usize>(), any::<usize>(), any::<usize>())
            .prop_map(|(m, d, t)| Op::AddWeekItem(m, d, t)),
        (any::<usize>(), any::<usize>(), any::<usize>())
            .prop_map(|(m, d, i)| Op::DeleteWeekItem(m, d, i)),
        (any::<usize>(), any::<usize>()).prop_map(|(p, m)| Op::ApplyTemplate(p, m)),
    ]
}

fn script_strategy() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(op_strategy(), 0..8)
}

/// Maps a raw plan index into range, optionally forced away from plan 0 so a script never
/// touches it (used to prove an untouched item's edit survives on the other side).
fn plan_index(p: usize, n: usize, exclude_first: bool) -> Option<usize> {
    if n == 0 {
        return None;
    }
    Some(if exclude_first && n > 1 {
        1 + p % (n - 1)
    } else {
        p % n
    })
}

/// Maps a raw Monday index into range, optionally forced away from `monday_at(0)` so a script
/// never touches the dated week holding the sentinel edit under test.
fn monday_index(m: usize, exclude_first: bool) -> usize {
    if exclude_first {
        1 + m % (N_MONDAYS - 1)
    } else {
        m % N_MONDAYS
    }
}

fn apply_plan_op(
    store: &mut Store,
    op: &Op,
    now: &mut impl FnMut() -> DateTime<chrono::Utc>,
    exclude_first: bool,
) {
    let plans = store.plans.len();
    match op {
        Op::EditNotes(plan, day, item, text) => {
            let Some(plan) = plan_index(*plan, plans, exclude_first) else {
                return;
            };
            let items = &mut store.plans[plan].days[day % 7];
            if items.is_empty() {
                return;
            }
            let item = item % items.len();
            items[item].notes.clone_from(text);
            items[item].updated_at = now();
        }
        Op::Move(plan, day, item, to_day) => {
            let Some(plan) = plan_index(*plan, plans, exclude_first) else {
                return;
            };
            let day = day % 7;
            let items = &store.plans[plan].days[day];
            if items.is_empty() {
                return;
            }
            let item = item % items.len();
            let mut moved = store.plans[plan].days[day].remove(item);
            moved.updated_at = now();
            store.plans[plan].days[to_day % 7].push(moved);
        }
        Op::Add(plan, day, type_idx) => {
            let Some(plan) = plan_index(*plan, plans, exclude_first) else {
                return;
            };
            let types = &store.library.types;
            if types.is_empty() {
                return;
            }
            let key = types[type_idx % types.len()].key.clone();
            if let Some(mut added) = new_item(&store.library, &key) {
                added.updated_at = now();
                store.plans[plan].days[day % 7].push(added);
            }
        }
        Op::Delete(plan, day, item) => {
            let Some(plan) = plan_index(*plan, plans, exclude_first) else {
                return;
            };
            let items = &mut store.plans[plan].days[day % 7];
            if items.is_empty() {
                return;
            }
            items.remove(item % items.len());
        }
        Op::RenamePlan(plan, name) => {
            let Some(plan) = plan_index(*plan, plans, exclude_first) else {
                return;
            };
            store.plans[plan].name.clone_from(name);
            store.plans[plan].updated_at = now();
        }
        Op::SetWeight(weight) => {
            store.profile.weight = *weight;
            store.profile.updated_at = now();
        }
        _ => unreachable!("not a plan op"),
    }
}

fn apply_week_op(
    store: &mut Store,
    op: &Op,
    now: &mut impl FnMut() -> DateTime<chrono::Utc>,
    exclude_first: bool,
) {
    let plans = store.plans.len();
    match op {
        Op::EditWeekNotes(monday, day, item, text) => {
            let monday = monday_at(monday_index(*monday, exclude_first));
            let items = &mut store.week_mut_or_insert(monday).plan.days[day % 7];
            if items.is_empty() {
                return;
            }
            let item = item % items.len();
            items[item].notes.clone_from(text);
            items[item].updated_at = now();
        }
        Op::MoveWeekItem(monday, day, item, to_day) => {
            let monday = monday_at(monday_index(*monday, exclude_first));
            let week = &mut store.week_mut_or_insert(monday).plan;
            let day = day % 7;
            let items = &week.days[day];
            if items.is_empty() {
                return;
            }
            let item = item % items.len();
            let mut moved = week.days[day].remove(item);
            moved.updated_at = now();
            week.days[to_day % 7].push(moved);
        }
        Op::AddWeekItem(monday, day, type_idx) => {
            let monday = monday_at(monday_index(*monday, exclude_first));
            let types = &store.library.types;
            if types.is_empty() {
                return;
            }
            let key = types[type_idx % types.len()].key.clone();
            if let Some(mut added) = new_item(&store.library, &key) {
                added.updated_at = now();
                store.week_mut_or_insert(monday).plan.days[day % 7].push(added);
            }
        }
        Op::DeleteWeekItem(monday, day, item) => {
            let monday = monday_at(monday_index(*monday, exclude_first));
            let items = &mut store.week_mut_or_insert(monday).plan.days[day % 7];
            if items.is_empty() {
                return;
            }
            items.remove(item % items.len());
        }
        Op::ApplyTemplate(plan, monday) => {
            let Some(plan) = plan_index(*plan, plans, false) else {
                return;
            };
            let template = store.plans[plan].clone();
            let monday = monday_at(monday_index(*monday, exclude_first));
            let applied = Week::apply(&template, monday);
            let stamp = now();
            let week = &mut store.week_mut_or_insert(monday).plan;
            week.name = applied.plan.name;
            week.days = applied.plan.days;
            week.updated_at = stamp;
            for it in week.days.iter_mut().flatten() {
                it.updated_at = stamp;
            }
        }
        _ => unreachable!("not a week op"),
    }
}

fn apply(store: &mut Store, op: &Op, clock: &mut i64, exclude_first: bool) {
    let mut now = || {
        *clock += 1;
        DateTime::from_timestamp(*clock, 0).unwrap()
    };
    match op {
        Op::EditNotes(..)
        | Op::Move(..)
        | Op::Add(..)
        | Op::Delete(..)
        | Op::RenamePlan(..)
        | Op::SetWeight(..) => apply_plan_op(store, op, &mut now, exclude_first),
        Op::EditWeekNotes(..)
        | Op::MoveWeekItem(..)
        | Op::AddWeekItem(..)
        | Op::DeleteWeekItem(..)
        | Op::ApplyTemplate(..) => apply_week_op(store, op, &mut now, exclude_first),
    }
}

fn apply_script(base: &Store, ops: &[Op], seed: i64, exclude_first: bool) -> Store {
    let mut s = base.clone();
    let mut clock = seed;
    for op in ops {
        apply(&mut s, op, &mut clock, exclude_first);
    }
    s
}

/// `s` with plans, session types and each day's items sorted by id: merged day order follows
/// the base's id order, not either side's `Vec` order, which `Plan::sorted_day` ignores anyway.
fn canon(mut s: Store) -> Store {
    for p in &mut s.plans {
        for d in &mut p.days {
            d.sort_by(|a, b| a.id.cmp(&b.id));
        }
    }
    for w in &mut s.weeks {
        for d in &mut w.plan.days {
            d.sort_by(|a, b| a.id.cmp(&b.id));
        }
    }
    s.plans.sort_by(|a, b| a.id.cmp(&b.id));
    s.weeks.sort_by_key(|w| w.monday);
    s.library.types.sort_by(|a, b| a.key.cmp(&b.key));
    s
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// `merge(base, x, x) == x`: a store merged with itself changes nothing, ordering included.
    #[test]
    fn merge_with_self_is_idempotent(ops in script_strategy()) {
        let base = base_store();
        let x = apply_script(&base, &ops, 1, false);
        let m = merge(&base, &x, &x, LOCAL, Some(REMOTE));
        prop_assert_eq!(canon(m.store), canon(x));
        prop_assert!(m.conflicts.is_empty());
    }

    /// `merge(base, base, remote) == remote` and `merge(base, local, base) == local`: an
    /// unchanged side never overrides the other's edits (fast-forward).
    #[test]
    fn unchanged_side_fast_forwards_to_the_other(ops in script_strategy()) {
        let base = base_store();
        let x = apply_script(&base, &ops, 1, false);
        prop_assert_eq!(canon(merge(&base, &base, &x, LOCAL, Some(REMOTE)).store), canon(x.clone()));
        prop_assert_eq!(canon(merge(&base, &x, &base, LOCAL, Some(REMOTE)).store), canon(x));
    }

    /// Merging `local` and `remote` gives the same result as merging `remote` and `local` with
    /// devices swapped: both machines converge on one document.
    #[test]
    fn merge_is_symmetric_across_devices(local_ops in script_strategy(), remote_ops in script_strategy()) {
        let base = base_store();
        let local = apply_script(&base, &local_ops, 1, false);
        let remote = apply_script(&base, &remote_ops, 1_000_000, false);
        let a = merge(&base, &local, &remote, LOCAL, Some(REMOTE));
        let b = merge(&base, &remote, &local, REMOTE, Some(LOCAL));
        prop_assert_eq!(canon(a.store), canon(b.store));
        prop_assert_eq!(a.conflicts.len(), b.conflicts.len());
    }

    /// No plan id, item id (within a plan) or session-type key appears twice in the merged
    /// store, and every item's `type_key` resolves in the merged library.
    #[test]
    fn merged_store_has_no_duplicate_ids_and_every_item_resolves(
        local_ops in script_strategy(), remote_ops in script_strategy()
    ) {
        let base = base_store();
        let local = apply_script(&base, &local_ops, 1, false);
        let remote = apply_script(&base, &remote_ops, 1_000_000, false);
        let m = merge(&base, &local, &remote, LOCAL, Some(REMOTE)).store;

        let mut plan_ids = HashSet::new();
        for p in &m.plans {
            prop_assert!(plan_ids.insert(p.id.clone()), "duplicate plan id {}", p.id);
            let mut item_ids = HashSet::new();
            for day in &p.days {
                for it in day {
                    prop_assert!(
                        item_ids.insert(it.id.clone()),
                        "duplicate item id {} in plan {}",
                        it.id,
                        p.id
                    );
                    prop_assert!(
                        m.library.get(&it.type_key).is_some(),
                        "item {} has an unresolved type_key {}",
                        it.id,
                        it.type_key
                    );
                }
            }
        }
        let mut type_keys = HashSet::new();
        for t in &m.library.types {
            prop_assert!(type_keys.insert(t.key.clone()), "duplicate session type key {}", t.key);
        }

        let mut mondays = HashSet::new();
        for w in m.weeks.windows(2) {
            prop_assert!(w[0].monday < w[1].monday, "weeks are not sorted by monday");
        }
        for w in &m.weeks {
            prop_assert!(mondays.insert(w.monday), "duplicate week for monday {}", w.monday);
            let mut item_ids = HashSet::new();
            for day in &w.plan.days {
                for it in day {
                    prop_assert!(
                        item_ids.insert(it.id.clone()),
                        "duplicate item id {} in week {}",
                        it.id,
                        w.monday
                    );
                    prop_assert!(
                        m.library.get(&it.type_key).is_some(),
                        "item {} has an unresolved type_key {}",
                        it.id,
                        it.type_key
                    );
                }
            }
        }
    }

    /// Both sides applying the same template to the same Monday, on top of whatever their
    /// scripts already did, converges on one dated week with no duplicate item ids: applying a
    /// template always replaces a week's items wholesale, so the two applications can never be
    /// seen as independent edits to reconcile.
    #[test]
    fn applying_the_same_template_to_the_same_monday_merges_into_one_week(
        local_ops in script_strategy(), remote_ops in script_strategy(),
        plan in any::<usize>(), monday in any::<usize>()
    ) {
        let base = base_store();
        let mut local = apply_script(&base, &local_ops, 1, false);
        let mut remote = apply_script(&base, &remote_ops, 1_000_000, false);

        let plan = plan_index(plan, base.plans.len(), false).unwrap();
        let monday = monday_at(monday % N_MONDAYS);
        let applied = Week::apply(&base.plans[plan], monday).plan;
        for s in [&mut local, &mut remote] {
            let week = &mut s.week_mut_or_insert(monday).plan;
            week.name.clone_from(&applied.name);
            week.days.clone_from(&applied.days);
        }

        let m = merge(&base, &local, &remote, LOCAL, Some(REMOTE)).store;
        let weeks: Vec<&tapas::model::Week> =
            m.weeks.iter().filter(|w| w.monday == monday).collect();
        prop_assert_eq!(weeks.len(), 1);
        let mut item_ids = HashSet::new();
        for day in &weeks[0].plan.days {
            for it in day {
                prop_assert!(
                    item_ids.insert(it.id.clone()),
                    "duplicate item id {} in week {monday}",
                    it.id
                );
            }
        }
    }

    /// An item edited on only one side keeps that edit, no matter what the other side did
    /// elsewhere. `remote_ops` is steered away from plan 0, which holds the sentinel edit.
    #[test]
    fn a_solo_edit_always_survives(remote_ops in script_strategy(), text in "[A-Za-z ]{1,16}") {
        let base = base_store();
        let mut local = base.clone();
        let sentinel = format!("SENTINEL {text}");
        local.plans[0].days[0][0].notes.clone_from(&sentinel);
        local.plans[0].days[0][0].updated_at = DateTime::from_timestamp(500_000, 0).unwrap();
        let week_sentinel = format!("WEEK SENTINEL {text}");
        local.weeks[0].plan.days[0][0].notes.clone_from(&week_sentinel);
        local.weeks[0].plan.days[0][0].updated_at = DateTime::from_timestamp(500_000, 0).unwrap();

        let remote = apply_script(&base, &remote_ops, 900_000, true);
        let m = merge(&base, &local, &remote, LOCAL, Some(REMOTE));
        prop_assert_eq!(&m.store.plans[0].days[0][0].notes, &sentinel);
        prop_assert_eq!(&m.store.weeks[0].plan.days[0][0].notes, &week_sentinel);

        // And the same from the other device's point of view.
        let m2 = merge(&base, &remote, &local, REMOTE, Some(LOCAL));
        prop_assert_eq!(&m2.store.plans[0].days[0][0].notes, &sentinel);
        prop_assert_eq!(&m2.store.weeks[0].plan.days[0][0].notes, &week_sentinel);
    }
}
