use crate::pattern::model::Chain;
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
