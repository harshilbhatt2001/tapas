//! Property tests for the pure three-way merge (`tapas::sync::merge`).
//!
//! Random "edit scripts" are applied to a shared base [`Store`] to produce `local` and `remote`
//! copies, mirroring how two machines diverge between syncs. Properties hold regardless of what
//! the scripts did, so failures shrink to a small script rather than one hand-picked case.

use std::collections::HashSet;

use chrono::DateTime;
use proptest::prelude::*;
use tapas::{
    library::{new_item, starter_plan},
    model::Store,
    sync::merge::merge,
};
use uuid::Uuid;

const LOCAL: Uuid = Uuid::from_u128(11);
const REMOTE: Uuid = Uuid::from_u128(22);

/// Two plans with the starter week's items spread across every day, so scripts have something
/// to edit, move, delete or add to from the very first op.
fn base_store() -> Store {
    Store {
        plans: vec![starter_plan("Base"), starter_plan("Build")],
        ..Store::default()
    }
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

fn apply(store: &mut Store, op: &Op, clock: &mut i64, exclude_first: bool) {
    let mut now = || {
        *clock += 1;
        DateTime::from_timestamp(*clock, 0).unwrap()
    };
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
    s.plans.sort_by(|a, b| a.id.cmp(&b.id));
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

        let remote = apply_script(&base, &remote_ops, 900_000, true);
        let m = merge(&base, &local, &remote, LOCAL, Some(REMOTE));
        prop_assert_eq!(&m.store.plans[0].days[0][0].notes, &sentinel);

        // And the same from the other device's point of view.
        let m2 = merge(&base, &remote, &local, REMOTE, Some(LOCAL));
        prop_assert_eq!(&m2.store.plans[0].days[0][0].notes, &sentinel);
    }
}
