//! Three-way merge of the synced [`Store`], and the `updated_at` stamps it relies on. Pure: no IO.
//!
//! Entities are matched by id: plans by id, items by id within their plan (the day is one of
//! the item's fields, so a move merges like an edit), session types by `key`, profile and
//! export settings as single values. A change on one side wins. If both sides changed the same
//! thing, the later `updated_at` wins; ties go to the higher device id. An edit beats a delete.

use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::model::{
    ExportSettings, Item, Library, Plan, Profile, STORE_VERSION, SessionType, Store,
};

/// Which side of a merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

impl Side {
    fn of<T>(self, local: T, remote: T) -> T {
        match self {
            Side::Local => local,
            Side::Remote => remote,
        }
    }
}

/// What a conflict was about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entity {
    /// A plan's own fields (its name), by plan id.
    Plan(String),
    Item {
        plan: String,
        item: String,
    },
    SessionType(String),
    Profile,
    /// Export settings other than `calendar_id`.
    Export,
    /// Both sides point at a different Google calendar.
    CalendarId,
}

/// Both sides changed `entity` differently; the `kept` side won and the other's edit is lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub entity: Entity,
    pub kept: Side,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Merged {
    pub store: Store,
    pub conflicts: Vec<Conflict>,
}

/// An entity with a last-writer-wins clock.
trait Stamped: Clone + PartialEq + Serialize {
    fn at(&self) -> DateTime<Utc>;
    fn at_mut(&mut self) -> &mut DateTime<Utc>;
}

macro_rules! stamped {
    ($($t:ty),*) => {$(
        impl Stamped for $t {
            fn at(&self) -> DateTime<Utc> {
                self.updated_at
            }
            fn at_mut(&mut self) -> &mut DateTime<Utc> {
                &mut self.updated_at
            }
        }
    )*};
}
stamped!(Plan, Item, SessionType, Profile, ExportSettings);

/// Equal apart from `updated_at`.
fn same<T: Stamped>(a: &T, b: &T) -> bool {
    if a.at() == b.at() {
        return a == b;
    }
    let mut a = a.clone();
    *a.at_mut() = b.at();
    a == *b
}

/// A plan's own fields, without its items.
fn head(p: &Plan) -> Plan {
    Plan {
        id: p.id.clone(),
        name: p.name.clone(),
        days: Default::default(),
        updated_at: p.updated_at,
    }
}

/// Keep `next`'s stamp as it is in `prev` when unchanged, else stamp it `now`. A changed entity
/// whose stamp already moved past `prev` (say, taken from a merge) keeps that stamp.
fn restamp<T: Stamped>(prev: Option<&T>, next: &mut T, now: DateTime<Utc>) {
    *next.at_mut() = match prev {
        Some(p) if same(p, next) => p.at(),
        Some(p) if next.at() > p.at() => next.at(),
        _ => now,
    };
}

/// Set `updated_at` in `next` on every entity that differs from `prev`, the previous save, and
/// carry `prev`'s stamps over to the rest. An item that moved to another day counts as changed;
/// a plan's own stamp covers only its name.
pub fn stamp(prev: &Store, next: &mut Store, now: DateTime<Utc>) {
    restamp(Some(&prev.profile), &mut next.profile, now);
    restamp(Some(&prev.export), &mut next.export, now);
    for t in &mut next.library.types {
        restamp(prev.library.get(&t.key), t, now);
    }
    for p in &mut next.plans {
        let old = prev.plans.iter().find(|q| q.id == p.id);
        let mut h = head(p);
        restamp(old.map(head).as_ref(), &mut h, now);
        p.updated_at = h.updated_at;
        for (d, day) in p.days.iter_mut().enumerate() {
            for it in day {
                let prev_same_day = old.and_then(|o| o.days[d].iter().find(|x| x.id == it.id));
                restamp(prev_same_day, it, now);
            }
        }
    }
}

/// How the two sides changed one thing relative to the base.
enum Change {
    /// Both sides hold the same content.
    Same,
    /// Only one side changed it (or deleted it while the other edited): take that side.
    Take(Side),
    /// Both sides changed it differently; both are present.
    Both,
}

/// `None` means absent: deleted, or not created yet.
fn changes<T>(
    base: Option<&T>,
    local: Option<&T>,
    remote: Option<&T>,
    eq: impl Fn(&T, &T) -> bool,
) -> Change {
    let eq = |x: Option<&T>, y: Option<&T>| match (x, y) {
        (Some(x), Some(y)) => eq(x, y),
        (None, None) => true,
        _ => false,
    };
    if eq(local, remote) {
        Change::Same
    } else if eq(base, local) {
        Change::Take(Side::Remote)
    } else if eq(base, remote) || remote.is_none() {
        Change::Take(Side::Local)
    } else if local.is_none() {
        Change::Take(Side::Remote)
    } else {
        Change::Both
    }
}

/// Which side of a field to take when both sides changed the entity it belongs to: the one
/// that changed it, else `newer` (flagging a conflict).
fn field<T>(
    base: Option<&T>,
    local: &T,
    remote: &T,
    eq: impl Fn(&T, &T) -> bool,
    newer: Side,
    conflict: &mut bool,
) -> Side {
    match changes(base, Some(local), Some(remote), eq) {
        Change::Same => Side::Local,
        Change::Take(side) => side,
        Change::Both => {
            *conflict = true;
            newer
        }
    }
}

/// `ids` of all three lists, deduplicated, in base, then local, then remote order.
fn ordered(lists: [Vec<&str>; 3]) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for id in lists.into_iter().flatten() {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn type_keys(l: &Library) -> Vec<&str> {
    l.types.iter().map(|t| t.key.as_str()).collect()
}

fn plan_ids(ps: &[Plan]) -> Vec<&str> {
    ps.iter().map(|p| p.id.as_str()).collect()
}

fn find_plan<'a>(ps: &'a [Plan], id: &str) -> Option<&'a Plan> {
    ps.iter().find(|p| p.id == id)
}

fn item_ids(p: Option<&Plan>) -> Vec<&str> {
    p.map(|p| p.days.iter().flatten().map(|x| x.id.as_str()).collect())
        .unwrap_or_default()
}

/// An item with its day index.
fn placed<'a>(p: Option<&'a Plan>, id: &str) -> Option<(usize, &'a Item)> {
    let p = p?;
    p.find(id).map(|(d, k)| (d, &p.days[d][k]))
}

fn same_placed(a: &(usize, &Item), b: &(usize, &Item)) -> bool {
    a.0 == b.0 && same(a.1, b.1)
}

/// The plan or any of its items differs between `a` and `b`.
fn plan_changed(a: &Plan, b: &Plan) -> bool {
    !same(&head(a), &head(b))
        || ordered([item_ids(Some(a)), item_ids(Some(b)), Vec::new()])
            .into_iter()
            .any(|id| match (placed(Some(a), id), placed(Some(b), id)) {
                (Some(x), Some(y)) => !same_placed(&x, &y),
                _ => true,
            })
}

struct Merger {
    local: Uuid,
    remote: Option<Uuid>,
    conflicts: Vec<Conflict>,
}

impl Merger {
    /// The side whose version wins a conflict: the later stamp, then the higher device id.
    /// Without a distinct remote device id, the larger JSON wins, so both devices still agree.
    fn newer<T: Stamped>(&self, local: &T, remote: &T) -> Side {
        match local.at().cmp(&remote.at()) {
            Ordering::Greater => Side::Local,
            Ordering::Less => Side::Remote,
            Ordering::Equal => match self.remote {
                Some(r) if r != self.local => {
                    if self.local > r {
                        Side::Local
                    } else {
                        Side::Remote
                    }
                }
                _ => {
                    let json = |x: &T| serde_json::to_vec(x).unwrap_or_default();
                    if json(local) >= json(remote) {
                        Side::Local
                    } else {
                        Side::Remote
                    }
                }
            },
        }
    }

    fn conflict(&mut self, entity: Entity, kept: Side) {
        self.conflicts.push(Conflict { entity, kept });
    }

    /// Three-way merge of one entity taken whole.
    fn whole<T: Stamped>(
        &mut self,
        base: Option<&T>,
        local: Option<&T>,
        remote: Option<&T>,
        entity: impl FnOnce() -> Entity,
    ) -> Option<T> {
        let side = match (changes(base, local, remote, same), local, remote) {
            (Change::Take(side), ..) => side,
            (Change::Same, Some(l), Some(r)) => self.newer(l, r),
            (Change::Both, Some(l), Some(r)) => {
                let side = self.newer(l, r);
                self.conflict(entity(), side);
                side
            }
            _ => return None,
        };
        side.of(local, remote).cloned()
    }

    fn profile(&mut self, base: &Profile, local: &Profile, remote: &Profile) -> Profile {
        self.whole(Some(base), Some(local), Some(remote), || Entity::Profile)
            .unwrap_or_else(|| local.clone())
    }

    /// `calendar_id` merges on its own, so that it neither loses nor wins with the other
    /// settings, and a calendar id beats none.
    fn export(
        &mut self,
        base: &ExportSettings,
        local: &ExportSettings,
        remote: &ExportSettings,
    ) -> ExportSettings {
        let rest = |e: &ExportSettings| ExportSettings {
            calendar_id: None,
            ..e.clone()
        };
        let newer = self.newer(local, remote);
        let mut conflict = false;
        let mut out = field(
            Some(&rest(base)),
            &rest(local),
            &rest(remote),
            same,
            newer,
            &mut conflict,
        )
        .of(local, remote)
        .clone();
        if conflict {
            self.conflict(Entity::Export, newer);
        }
        let (b, l, r) = (&base.calendar_id, &local.calendar_id, &remote.calendar_id);
        out.calendar_id = match changes(Some(b), Some(l), Some(r), PartialEq::eq) {
            Change::Same => l.clone(),
            Change::Take(side) => side.of(l, r).clone(),
            Change::Both => match (l, r) {
                (Some(_), None) => l.clone(),
                (None, Some(_)) => r.clone(),
                _ => {
                    self.conflict(Entity::CalendarId, newer);
                    newer.of(l, r).clone()
                }
            },
        };
        out.updated_at = local.updated_at.max(remote.updated_at);
        out
    }

    /// Session types by key. A type deleted on one side comes back if a merged item uses it.
    fn library(
        &mut self,
        base: &Library,
        local: &Library,
        remote: &Library,
        plans: &[Plan],
    ) -> Library {
        let types = ordered([type_keys(base), type_keys(local), type_keys(remote)])
            .into_iter()
            .filter_map(|key| {
                let (b, l, r) = (base.get(key), local.get(key), remote.get(key));
                self.whole(b, l, r, || Entity::SessionType(key.to_owned()))
                    .or_else(|| {
                        plans
                            .iter()
                            .any(|p| p.uses_type(key))
                            .then(|| l.or(r).or(b).cloned())
                            .flatten()
                    })
            })
            .collect();
        Library { types }
    }

    /// Plans by id. A plan deleted on one side survives, whole, if the other side changed it
    /// or any of its items.
    fn plans(&mut self, base: &[Plan], local: &[Plan], remote: &[Plan]) -> Vec<Plan> {
        let mut out = Vec::new();
        for id in ordered([plan_ids(base), plan_ids(local), plan_ids(remote)]) {
            let (b, l, r) = (
                find_plan(base, id),
                find_plan(local, id),
                find_plan(remote, id),
            );
            let plan = match (l, r) {
                (Some(l), Some(r)) => Some(self.plan(b, l, r)),
                (Some(p), None) | (None, Some(p)) => {
                    b.is_none_or(|b| plan_changed(b, p)).then(|| p.clone())
                }
                (None, None) => None,
            };
            out.extend(plan);
        }
        out
    }

    /// A plan on both sides: its own fields, then its items by id.
    fn plan(&mut self, base: Option<&Plan>, local: &Plan, remote: &Plan) -> Plan {
        let (bh, lh, rh) = (base.map(head), head(local), head(remote));
        let mut plan = self
            .whole(bh.as_ref(), Some(&lh), Some(&rh), || {
                Entity::Plan(local.id.clone())
            })
            .unwrap_or(lh);
        for id in ordered([
            item_ids(base),
            item_ids(Some(local)),
            item_ids(Some(remote)),
        ]) {
            let (b, l, r) = (
                placed(base, id),
                placed(Some(local), id),
                placed(Some(remote), id),
            );
            let entity = || Entity::Item {
                plan: local.id.clone(),
                item: id.to_owned(),
            };
            let merged = match (
                changes(b.as_ref(), l.as_ref(), r.as_ref(), same_placed),
                l,
                r,
            ) {
                (Change::Take(side), ..) => side.of(l, r).map(|(d, it)| (d, it.clone())),
                (Change::Same, Some(lp), Some(rp)) => {
                    let (d, it) = self.newer(lp.1, rp.1).of(lp, rp);
                    Some((d, it.clone()))
                }
                (Change::Both, Some((ld, li)), Some((rd, ri))) => {
                    // Day and the rest merge as separate fields: a move on one side and an
                    // edit on the other both apply.
                    let newer = self.newer(li, ri);
                    let mut conflict = false;
                    let day = field(
                        b.map(|b| b.0).as_ref(),
                        &ld,
                        &rd,
                        PartialEq::eq,
                        newer,
                        &mut conflict,
                    );
                    let body = field(b.map(|b| b.1), li, ri, same, newer, &mut conflict);
                    if conflict {
                        self.conflict(entity(), newer);
                    }
                    let mut it = body.of(li, ri).clone();
                    it.updated_at = li.updated_at.max(ri.updated_at);
                    Some((day.of(ld, rd), it))
                }
                _ => None,
            };
            if let Some((d, it)) = merged {
                plan.days[d].push(it);
            }
        }
        plan
    }
}

/// Merge `local` and `remote`, both descended from `base`, the last synced document. Ties
/// between equal stamps go to the higher of `local_device` and `remote_device`; pass `None`
/// when the remote copy's writer is unknown. Plans, items and session types keep `base`'s
/// order, followed by new ones from `local`, then from `remote`.
#[must_use]
pub fn merge(
    base: &Store,
    local: &Store,
    remote: &Store,
    local_device: Uuid,
    remote_device: Option<Uuid>,
) -> Merged {
    let mut m = Merger {
        local: local_device,
        remote: remote_device,
        conflicts: Vec::new(),
    };
    let plans = m.plans(&base.plans, &local.plans, &remote.plans);
    let store = Store {
        version: STORE_VERSION,
        profile: m.profile(&base.profile, &local.profile, &remote.profile),
        library: m.library(&base.library, &local.library, &remote.library, &plans),
        plans,
        export: m.export(&base.export, &local.export, &remote.export),
    };
    Merged {
        store,
        conflicts: m.conflicts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{new_item, starter_plan};

    const LO: Uuid = Uuid::from_u128(1);
    const HI: Uuid = Uuid::from_u128(2);

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn base() -> Store {
        Store {
            plans: vec![starter_plan("Base"), starter_plan("Build")],
            ..Store::default()
        }
    }

    /// Merged as the lower device, against the higher one.
    fn run(base: &Store, local: &Store, remote: &Store) -> Merged {
        merge(base, local, remote, LO, Some(HI))
    }

    /// The same merge seen from the other device.
    fn swapped(base: &Store, local: &Store, remote: &Store) -> Merged {
        merge(base, remote, local, HI, Some(LO))
    }

    fn edit(s: &mut Store, plan: usize, day: usize, k: usize, notes: &str, secs: i64) {
        let it = &mut s.plans[plan].days[day][k];
        notes.clone_into(&mut it.notes);
        it.updated_at = at(secs);
    }

    fn remove(s: &mut Store, plan: usize, day: usize, k: usize) -> Item {
        s.plans[plan].days[day].remove(k)
    }

    fn names(s: &Store) -> Vec<&str> {
        s.plans.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn disjoint_edits_merge() {
        let b = base();
        let (mut l, mut r) = (b.clone(), b.clone());
        l.plans[0].name = "Base 2".into();
        l.plans[0].updated_at = at(10);
        edit(&mut l, 0, 0, 0, "local", 10);
        l.profile.weight = 70.0;
        l.profile.updated_at = at(10);
        edit(&mut r, 0, 1, 0, "remote", 20);
        r.export.weeks = 4;
        r.export.updated_at = at(20);
        let mut yoga = r.library.types[0].clone();
        "yoga".clone_into(&mut yoga.key);
        r.library.types.push(yoga);
        let mut new = new_item(&r.library, "yoga").unwrap();
        new.updated_at = at(20);
        r.plans[1].days[4].push(new.clone());

        let m = run(&b, &l, &r);
        assert_eq!(m.conflicts, []);
        let out = &m.store;
        assert_eq!(out.plans[0].name, "Base 2");
        assert_eq!(out.plans[0].days[0][0].notes, "local");
        assert_eq!(out.plans[0].days[1][0].notes, "remote");
        assert!((out.profile.weight - 70.0).abs() < f64::EPSILON);
        assert_eq!(out.export.weeks, 4);
        assert!(out.library.get("yoga").is_some());
        assert_eq!(out.plans[1].days[4].last(), Some(&new));
        assert_eq!(swapped(&b, &l, &r), m);
    }

    #[test]
    fn same_item_changed_on_both_sides_later_wins() {
        let b = base();
        let (mut l, mut r) = (b.clone(), b.clone());
        edit(&mut l, 0, 0, 0, "local", 20);
        edit(&mut r, 0, 0, 0, "remote", 10);
        let m = run(&b, &l, &r);
        assert_eq!(m.store.plans[0].days[0][0].notes, "local");
        let item = Entity::Item {
            plan: b.plans[0].id.clone(),
            item: b.plans[0].days[0][0].id.clone(),
        };
        let kept = |kept| {
            [Conflict {
                entity: item.clone(),
                kept,
            }]
        };
        assert_eq!(m.conflicts, kept(Side::Local));

        edit(&mut r, 0, 0, 0, "remote", 30);
        let m = run(&b, &l, &r);
        assert_eq!(m.store.plans[0].days[0][0].notes, "remote");
        assert_eq!(m.conflicts, kept(Side::Remote));
    }

    #[test]
    fn profile_and_plan_name_conflicts_are_last_writer_wins() {
        let b = base();
        let (mut l, mut r) = (b.clone(), b.clone());
        l.profile.weight = 70.0;
        l.profile.updated_at = at(10);
        r.profile.weight = 75.0;
        r.profile.updated_at = at(20);
        l.plans[1].name = "L".into();
        l.plans[1].updated_at = at(20);
        r.plans[1].name = "R".into();
        r.plans[1].updated_at = at(10);
        let m = run(&b, &l, &r);
        assert_eq!(m.store.profile, r.profile);
        assert_eq!(m.store.plans[1].name, "L");
        assert_eq!(
            m.conflicts,
            [
                Conflict {
                    entity: Entity::Plan(b.plans[1].id.clone()),
                    kept: Side::Local
                },
                Conflict {
                    entity: Entity::Profile,
                    kept: Side::Remote
                }
            ]
        );
    }

    #[test]
    fn equal_stamps_go_to_the_higher_device() {
        let b = base();
        let (mut l, mut r) = (b.clone(), b.clone());
        edit(&mut l, 0, 0, 0, "local", 10);
        edit(&mut r, 0, 0, 0, "remote", 10);
        let notes = |m: Merged| m.store.plans[0].days[0][0].notes.clone();
        assert_eq!(notes(merge(&b, &l, &r, LO, Some(HI))), "remote");
        assert_eq!(notes(merge(&b, &l, &r, HI, Some(LO))), "local");
        // Without a distinct remote device id both devices still pick the same one.
        let one = merge(&b, &l, &r, LO, None);
        assert_eq!(one.store, merge(&b, &r, &l, HI, None).store);
        assert_eq!(one.store, merge(&b, &l, &r, LO, Some(LO)).store);
    }

    #[test]
    fn edit_beats_delete_both_ways() {
        let b = base();
        let (mut edited, mut deleted) = (b.clone(), b.clone());
        edit(&mut edited, 0, 2, 1, "kept", 10);
        remove(&mut deleted, 0, 2, 1);
        for m in [run(&b, &edited, &deleted), run(&b, &deleted, &edited)] {
            assert_eq!(m.store.plans[0].days[2][1].notes, "kept");
            assert_eq!(m.store.plans[0].days[2].len(), b.plans[0].days[2].len());
            assert_eq!(m.conflicts, []);
        }
        // An unchanged item does not come back.
        for m in [run(&b, &b, &deleted), run(&b, &deleted, &b)] {
            assert_eq!(m.store, deleted);
        }
    }

    #[test]
    fn deleted_plan_survives_an_item_edit() {
        let b = base();
        let (mut edited, mut deleted) = (b.clone(), b.clone());
        edit(&mut edited, 1, 5, 0, "long ride", 10);
        deleted.plans.remove(1);
        for m in [run(&b, &edited, &deleted), run(&b, &deleted, &edited)] {
            assert_eq!(m.store.plans, edited.plans);
        }
        // A new item counts as a change too.
        let mut added = b.clone();
        let it = new_item(&added.library, "run").unwrap();
        added.plans[1].days[3].push(it);
        assert_eq!(run(&b, &added, &deleted).store.plans, added.plans);
        // An untouched plan stays deleted.
        for m in [run(&b, &b, &deleted), run(&b, &deleted, &b)] {
            assert_eq!(m.store.plans, deleted.plans);
        }
    }

    #[test]
    fn move_on_one_side_and_edit_on_the_other_both_apply() {
        let b = base();
        let (mut moved, mut edited) = (b.clone(), b.clone());
        let mut it = remove(&mut moved, 0, 0, 0);
        it.updated_at = at(10);
        moved.plans[0].days[3].push(it.clone());
        edit(&mut edited, 0, 0, 0, "edited", 20);
        for m in [run(&b, &moved, &edited), run(&b, &edited, &moved)] {
            assert_eq!(m.conflicts, []);
            let p = &m.store.plans[0];
            let (d, k) = p.find(&it.id).unwrap();
            assert_eq!(d, 3);
            assert_eq!(p.days[d][k].notes, "edited");
            assert_eq!(p.days[d][k].updated_at, at(20));
            assert_eq!(p.days[0].len(), b.plans[0].days[0].len() - 1);
        }

        // Moved to different days on both sides: the later move wins.
        let mut other = b.clone();
        let mut it = remove(&mut other, 0, 0, 0);
        it.updated_at = at(30);
        other.plans[0].days[5].push(it.clone());
        let m = run(&b, &moved, &other);
        assert_eq!(m.store.plans[0].find(&it.id).unwrap().0, 5);
        assert_eq!(m.conflicts.len(), 1);
    }

    #[test]
    fn new_plans_and_items_on_both_sides_keep_a_stable_order() {
        let b = base();
        let (mut l, mut r) = (b.clone(), b.clone());
        l.plans.push(Plan::new("L1"));
        l.plans.push(Plan::new("L2"));
        r.plans.push(Plan::new("R1"));
        // Reordering on one side does not reorder the merge.
        r.plans.swap(0, 1);
        let li = new_item(&l.library, "run").unwrap();
        let ri = new_item(&r.library, "swim").unwrap();
        l.plans[0].days[6].push(li.clone());
        r.plans[1].days[6].push(ri.clone());
        let m = run(&b, &l, &r);
        assert_eq!(names(&m.store), ["Base", "Build", "L1", "L2", "R1"]);
        let sunday = &m.store.plans[0].days[6];
        let old = sunday.len() - 2;
        assert_eq!(sunday[..old], b.plans[0].days[6]);
        assert_eq!(sunday[old..], [li, ri]);
        assert_eq!(m.conflicts, []);
    }

    #[test]
    fn deleted_type_comes_back_while_an_item_uses_it() {
        let b = base();
        let pos = b.library.types.iter().position(|t| t.key == "padel");
        let mut l = b.clone();
        l.library.types.retain(|t| t.key != "padel");
        for p in &mut l.plans {
            for d in &mut p.days {
                d.retain(|x| x.type_key != "padel");
            }
        }
        // Nothing uses it any more: it stays deleted.
        let m = run(&b, &l, &b);
        assert!(m.store.library.get("padel").is_none());
        assert_eq!(m.store, l);

        let mut r = b.clone();
        let it = new_item(&r.library, "padel").unwrap();
        r.plans[0].days[2].push(it.clone());
        for m in [run(&b, &l, &r), swapped(&b, &l, &r)] {
            assert_eq!(m.store.library.types, b.library.types);
            assert_eq!(
                m.store.library.types.iter().position(|t| t.key == "padel"),
                pos
            );
            assert_eq!(m.store.plans[0].days[2].last(), Some(&it));
        }
    }

    fn with_calendar(s: &Store, id: Option<&str>, secs: i64) -> Store {
        let mut s = s.clone();
        s.export.calendar_id = id.map(str::to_owned);
        s.export.updated_at = at(secs);
        s
    }

    #[test]
    fn calendar_id_merges_on_its_own() {
        let b = base();
        // Set on one side, other settings changed on the other: both apply.
        let l = with_calendar(&b, Some("a"), 10);
        let mut r = with_calendar(&b, None, 20);
        r.export.weeks = 6;
        let m = run(&b, &l, &r);
        assert_eq!(m.store.export.calendar_id.as_deref(), Some("a"));
        assert_eq!(m.store.export.weeks, 6);
        assert_eq!(m.store.export.updated_at, at(20));
        assert_eq!(m.conflicts, []);

        // Two different calendars: the newer one, reported.
        let r = with_calendar(&b, Some("b"), 20);
        let m = run(&b, &l, &r);
        assert_eq!(m.store.export.calendar_id.as_deref(), Some("b"));
        assert_eq!(
            m.conflicts,
            [Conflict {
                entity: Entity::CalendarId,
                kept: Side::Remote
            }]
        );
        assert_eq!(swapped(&b, &l, &r).store, m.store);

        // Cleared on one side, replaced on the other: a calendar beats none.
        let b = with_calendar(&b, Some("a"), 5);
        let cleared = with_calendar(&b, None, 30);
        let replaced = with_calendar(&b, Some("b"), 10);
        for m in [run(&b, &cleared, &replaced), run(&b, &replaced, &cleared)] {
            assert_eq!(m.store.export.calendar_id.as_deref(), Some("b"));
            assert_eq!(m.conflicts, []);
        }
        // Cleared on one side only: stays cleared.
        assert_eq!(run(&b, &cleared, &b).store.export.calendar_id, None);
    }

    #[test]
    fn merging_the_same_change_is_idempotent() {
        let b = base();
        let mut x = b.clone();
        edit(&mut x, 0, 0, 0, "x", 10);
        x.plans.remove(1);
        x.plans.push(Plan::new("New"));
        x.library.types.retain(|t| t.key != "brick");
        let m = run(&b, &x, &x);
        assert_eq!(m.store, x);
        assert_eq!(m.conflicts, []);
        assert_eq!(run(&b, &b, &b).store, b);
    }

    #[test]
    fn stamp_touches_only_changed_entities() {
        let prev = base();
        let mut next = prev.clone();
        edit(&mut next, 0, 0, 0, "edited", 0);
        let moved = remove(&mut next, 0, 1, 0);
        next.plans[0].days[4].push(moved);
        next.plans[1].name = "Renamed".into();
        let new = new_item(&next.library, "run").unwrap();
        next.plans[1].days[0].push(new);
        next.profile.weight = 71.0;
        next.library.types[0].label = "Relabelled".into();
        let edits = next.clone();

        stamp(&prev, &mut next, at(99));
        let n0 = next.plans[0].days[4].len() - 1;
        let n1 = next.plans[1].days[0].len() - 1;
        let changed = |t: &mut DateTime<Utc>| {
            assert_eq!(*t, at(99));
            *t = DateTime::UNIX_EPOCH;
        };
        changed(&mut next.plans[0].days[0][0].updated_at);
        changed(&mut next.plans[0].days[4][n0].updated_at);
        changed(&mut next.plans[1].days[0][n1].updated_at);
        changed(&mut next.plans[1].updated_at);
        changed(&mut next.profile.updated_at);
        changed(&mut next.library.types[0].updated_at);
        // Everything else kept its (epoch) stamp; an item edit leaves its plan's stamp alone.
        assert_eq!(next, edits);
    }

    #[test]
    fn stamp_carries_over_and_keeps_newer_stamps() {
        let mut prev = base();
        prev.plans[0].days[0][0].updated_at = at(5);
        let mut next = prev.clone();
        // Same content, stamp lost: carried over from the previous save.
        next.plans[0].days[0][0].updated_at = DateTime::UNIX_EPOCH;
        // Changed with a stamp past the previous one (from a merge): kept.
        next.export.weeks = 3;
        next.export.updated_at = at(50);
        stamp(&prev, &mut next, at(99));
        assert_eq!(next.plans[0].days[0][0].updated_at, at(5));
        assert_eq!(next.export.updated_at, at(50));
    }
}
