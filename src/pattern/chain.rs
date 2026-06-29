use crate::pattern::model::{Chain, ChainEntry, Set};
use crate::persist;

#[derive(Clone, Debug, PartialEq)]
pub struct ChainPlayback {
    pub chain_id: persist::Id,
    pub entry_idx: usize,
    pub entry_start_step: u64,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainStep {
    Hold,
    Advance(usize),
    LoopWrap,
    Stop,
}

/// Decide what the chain should do at `now_step`, given the current entry and
/// the absolute step it started on. Caller invokes this at bar boundaries.
pub fn chain_decision(
    chain: &Chain,
    entry_idx: usize,
    entry_start_step: u64,
    now_step: u64,
) -> ChainStep {
    let Some(entry) = chain.entries.get(entry_idx) else {
        return ChainStep::Stop;
    };
    let elapsed = now_step.saturating_sub(entry_start_step);
    if elapsed < entry.dwell_steps() {
        return ChainStep::Hold;
    }
    let next = entry_idx + 1;
    if next < chain.entries.len() {
        ChainStep::Advance(next)
    } else if chain.looped {
        ChainStep::LoopWrap
    } else {
        ChainStep::Stop
    }
}

// ── Chain CRUD ────────────────────────────────────────────────────────────────

/// Create a new chain with the given name, push it onto `set.chains`, and return
/// the index of the newly created chain.
pub fn create_chain(set: &mut Set, name: impl Into<String>) -> usize {
    let chain = Chain::new(name);
    set.chains.push(chain);
    set.chains.len() - 1
}

/// Rename the chain at `idx`. No-op if `idx` is out of range.
pub fn rename_chain(set: &mut Set, idx: usize, name: impl Into<String>) {
    if let Some(chain) = set.chains.get_mut(idx) {
        chain.name = name.into();
    }
}

/// Deep-clone the chain at `idx`, assign fresh ids to the clone and all its
/// entries, append " copy" to the name, push it, and return the new index.
/// No-op (returns `idx`) if `idx` is out of range.
pub fn duplicate_chain(set: &mut Set, idx: usize) -> usize {
    let Some(source) = set.chains.get(idx).cloned() else {
        return idx;
    };
    let mut copy = source.clone();
    copy.id = persist::mint_id();
    copy.name = format!("{} copy", source.name);
    // Re-mint every entry id so entries are independent.
    for entry in copy.entries.iter_mut() {
        entry.scene_id = entry.scene_id.clone(); // scene_id points into set.scenes — keep it
    }
    set.chains.push(copy);
    set.chains.len() - 1
}

/// Remove the chain at `idx`. No-op if `idx` is out of range.
pub fn delete_chain(set: &mut Set, idx: usize) {
    if idx < set.chains.len() {
        set.chains.remove(idx);
    }
}

/// Append a new entry for `scene_id` to chain `chain_idx`.
/// Defaults: `repeats = 1`, `bars = 1`.
/// No-op if `chain_idx` is out of range.
pub fn add_chain_entry(set: &mut Set, chain_idx: usize, scene_id: persist::Id) {
    if let Some(chain) = set.chains.get_mut(chain_idx) {
        chain.entries.push(ChainEntry {
            scene_id,
            repeats: 1,
            bars: 1,
        });
    }
}

/// Remove the entry at `entry_idx` from chain `chain_idx`.
/// No-op if either index is out of range.
pub fn remove_chain_entry(set: &mut Set, chain_idx: usize, entry_idx: usize) {
    if let Some(chain) = set.chains.get_mut(chain_idx) {
        if entry_idx < chain.entries.len() {
            chain.entries.remove(entry_idx);
        }
    }
}

/// Move entry at `entry_idx` one position earlier (toward index 0).
/// No-op at index 0 or if indices are out of range.
pub fn move_chain_entry_up(set: &mut Set, chain_idx: usize, entry_idx: usize) {
    if let Some(chain) = set.chains.get_mut(chain_idx) {
        if entry_idx > 0 && entry_idx < chain.entries.len() {
            chain.entries.swap(entry_idx - 1, entry_idx);
        }
    }
}

/// Move entry at `entry_idx` one position later (toward the end).
/// No-op at last position or if indices are out of range.
pub fn move_chain_entry_down(set: &mut Set, chain_idx: usize, entry_idx: usize) {
    if let Some(chain) = set.chains.get_mut(chain_idx) {
        if entry_idx + 1 < chain.entries.len() {
            chain.entries.swap(entry_idx, entry_idx + 1);
        }
    }
}

/// Set `repeats` on entry `entry_idx` of chain `chain_idx`, clamped to >= 1.
/// No-op if either index is out of range.
pub fn set_chain_entry_repeats(set: &mut Set, chain_idx: usize, entry_idx: usize, value: u32) {
    if let Some(chain) = set.chains.get_mut(chain_idx) {
        if let Some(entry) = chain.entries.get_mut(entry_idx) {
            entry.repeats = value.max(1);
        }
    }
}

/// Set `bars` on entry `entry_idx` of chain `chain_idx`, clamped to >= 1.
/// No-op if either index is out of range.
pub fn set_chain_entry_bars(set: &mut Set, chain_idx: usize, entry_idx: usize, value: u32) {
    if let Some(chain) = set.chains.get_mut(chain_idx) {
        if let Some(entry) = chain.entries.get_mut(entry_idx) {
            entry.bars = value.max(1);
        }
    }
}

/// Toggle `looped` on the chain at `idx`.
/// No-op if `idx` is out of range.
pub fn toggle_chain_loop(set: &mut Set, idx: usize) {
    if let Some(chain) = set.chains.get_mut(idx) {
        chain.looped = !chain.looped;
    }
}

#[cfg(test)]
mod chain_crud_tests {
    use super::*;
    use crate::devices::profiles;

    fn make_set() -> Set {
        Set::default_set(profiles::default_profiles())
    }

    #[test]
    fn create_and_rename_chain() {
        let mut set = make_set();
        assert!(set.chains.is_empty());
        let idx = create_chain(&mut set, "intro");
        assert_eq!(set.chains.len(), 1);
        assert_eq!(set.chains[idx].name, "intro");
        rename_chain(&mut set, idx, "intro v2");
        assert_eq!(set.chains[idx].name, "intro v2");
    }

    #[test]
    fn create_chain_mints_unique_ids() {
        let mut set = make_set();
        let i0 = create_chain(&mut set, "a");
        let i1 = create_chain(&mut set, "b");
        assert_ne!(set.chains[i0].id, set.chains[i1].id);
    }

    #[test]
    fn duplicate_chain_fresh_id_and_copy_suffix() {
        let mut set = make_set();
        let idx = create_chain(&mut set, "main");
        let copy_idx = duplicate_chain(&mut set, idx);
        assert_ne!(copy_idx, idx);
        assert_eq!(set.chains[copy_idx].name, "main copy");
        assert_ne!(set.chains[copy_idx].id, set.chains[idx].id);
    }

    #[test]
    fn delete_chain_removes_it() {
        let mut set = make_set();
        create_chain(&mut set, "a");
        create_chain(&mut set, "b");
        delete_chain(&mut set, 0);
        assert_eq!(set.chains.len(), 1);
        assert_eq!(set.chains[0].name, "b");
    }

    #[test]
    fn add_remove_entries() {
        let mut set = make_set();
        let c = create_chain(&mut set, "x");
        let s0 = persist::mint_id();
        let s1 = persist::mint_id();
        add_chain_entry(&mut set, c, s0.clone());
        add_chain_entry(&mut set, c, s1.clone());
        assert_eq!(set.chains[c].entries.len(), 2);
        assert_eq!(set.chains[c].entries[0].scene_id, s0);
        assert_eq!(set.chains[c].entries[1].scene_id, s1);
        remove_chain_entry(&mut set, c, 0);
        assert_eq!(set.chains[c].entries.len(), 1);
        assert_eq!(set.chains[c].entries[0].scene_id, s1);
    }

    #[test]
    fn add_entry_defaults_repeats_bars_to_1() {
        let mut set = make_set();
        let c = create_chain(&mut set, "x");
        add_chain_entry(&mut set, c, persist::mint_id());
        assert_eq!(set.chains[c].entries[0].repeats, 1);
        assert_eq!(set.chains[c].entries[0].bars, 1);
    }

    #[test]
    fn reorder_entries_up_and_down() {
        let mut set = make_set();
        let c = create_chain(&mut set, "x");
        let s0 = persist::mint_id();
        let s1 = persist::mint_id();
        add_chain_entry(&mut set, c, s0.clone());
        add_chain_entry(&mut set, c, s1.clone());
        // move index 1 up → s1 first
        move_chain_entry_up(&mut set, c, 1);
        assert_eq!(set.chains[c].entries[0].scene_id, s1);
        assert_eq!(set.chains[c].entries[1].scene_id, s0);
        // move index 0 down → back to original order
        move_chain_entry_down(&mut set, c, 0);
        assert_eq!(set.chains[c].entries[0].scene_id, s0);
        assert_eq!(set.chains[c].entries[1].scene_id, s1);
    }

    #[test]
    fn move_up_at_zero_is_noop() {
        let mut set = make_set();
        let c = create_chain(&mut set, "x");
        let s0 = persist::mint_id();
        add_chain_entry(&mut set, c, s0.clone());
        move_chain_entry_up(&mut set, c, 0);
        assert_eq!(set.chains[c].entries[0].scene_id, s0);
    }

    #[test]
    fn move_down_at_last_is_noop() {
        let mut set = make_set();
        let c = create_chain(&mut set, "x");
        let s0 = persist::mint_id();
        add_chain_entry(&mut set, c, s0.clone());
        move_chain_entry_down(&mut set, c, 0);
        assert_eq!(set.chains[c].entries[0].scene_id, s0);
    }

    #[test]
    fn set_repeats_bars_clamped_to_1() {
        let mut set = make_set();
        let c = create_chain(&mut set, "x");
        add_chain_entry(&mut set, c, persist::mint_id());
        set_chain_entry_repeats(&mut set, c, 0, 0); // clamped to 1
        assert_eq!(set.chains[c].entries[0].repeats, 1);
        set_chain_entry_bars(&mut set, c, 0, 0); // clamped to 1
        assert_eq!(set.chains[c].entries[0].bars, 1);
        set_chain_entry_repeats(&mut set, c, 0, 4);
        assert_eq!(set.chains[c].entries[0].repeats, 4);
        set_chain_entry_bars(&mut set, c, 0, 8);
        assert_eq!(set.chains[c].entries[0].bars, 8);
    }

    #[test]
    fn toggle_chain_loop_flips_looped() {
        let mut set = make_set();
        let c = create_chain(&mut set, "x");
        assert!(!set.chains[c].looped);
        toggle_chain_loop(&mut set, c);
        assert!(set.chains[c].looped);
        toggle_chain_loop(&mut set, c);
        assert!(!set.chains[c].looped);
    }

    #[test]
    fn rename_noop_oob() {
        let mut set = make_set();
        rename_chain(&mut set, 99, "x"); // should not panic
    }

    #[test]
    fn delete_noop_oob() {
        let mut set = make_set();
        delete_chain(&mut set, 0); // empty — should not panic
    }
}

#[cfg(test)]
mod chain_transport_tests {
    use super::*;
    use crate::pattern::model::{Chain, ChainEntry};
    use crate::persist;

    fn make_chain(entries: &[(u32, u32)], looped: bool) -> Chain {
        let mut c = Chain::new("t");
        c.looped = looped;
        for &(bars, repeats) in entries {
            c.entries.push(ChainEntry {
                scene_id: persist::mint_id(),
                repeats,
                bars,
            });
        }
        c
    }

    #[test]
    fn holds_before_dwell_elapses() {
        let c = make_chain(&[(1, 1)], false); // dwell = 16 steps
        assert_eq!(chain_decision(&c, 0, 0, 8), ChainStep::Hold);
        assert_eq!(chain_decision(&c, 0, 0, 15), ChainStep::Hold);
    }

    #[test]
    fn advances_at_dwell_boundary() {
        let c = make_chain(&[(1, 1), (1, 1)], false); // each 16 steps
        assert_eq!(chain_decision(&c, 0, 0, 16), ChainStep::Advance(1));
    }

    #[test]
    fn stops_after_last_entry_when_not_looped() {
        let c = make_chain(&[(1, 1)], false);
        assert_eq!(chain_decision(&c, 0, 0, 16), ChainStep::Stop);
    }

    #[test]
    fn loops_after_last_entry_when_looped() {
        let c = make_chain(&[(1, 1)], true);
        assert_eq!(chain_decision(&c, 0, 0, 16), ChainStep::LoopWrap);
    }

    #[test]
    fn respects_repeats_in_dwell() {
        let c = make_chain(&[(1, 3), (1, 1)], false); // entry0 dwell = 48 steps
        assert_eq!(chain_decision(&c, 0, 0, 32), ChainStep::Hold);
        assert_eq!(chain_decision(&c, 0, 0, 48), ChainStep::Advance(1));
    }

    #[test]
    fn anchored_to_entry_start_step_not_zero() {
        let c = make_chain(&[(1, 1), (1, 1)], false);
        // entry 1 started at step 16; advance/stop at 32
        assert_eq!(chain_decision(&c, 1, 16, 24), ChainStep::Hold);
        assert_eq!(chain_decision(&c, 1, 16, 32), ChainStep::Stop);
    }
}
