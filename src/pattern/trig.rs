use crate::pattern::model::TrigCond;

/// Evaluate whether a trigger condition fires on the current loop cycle.
///
/// - `loop_index`: zero-based count of how many times the pattern has looped
/// - `fill_active`: whether fill mode is currently engaged
/// - `is_first`: whether this is the very first playback pass
pub fn trig_fires(cond: &TrigCond, loop_index: u64, fill_active: bool, is_first: bool) -> bool {
    match cond {
        TrigCond::Always => true,
        TrigCond::Ratio { x, y } => {
            if *y == 0 {
                true
            } else {
                (loop_index % *y as u64) + 1 == *x as u64
            }
        }
        TrigCond::Fill => fill_active,
        TrigCond::NotFill => !fill_active,
        TrigCond::First => is_first,
        TrigCond::NotFirst => !is_first,
    }
}

#[cfg(test)]
mod trig_tests {
    use super::*;
    use crate::pattern::model::TrigCond::*;

    #[test]
    fn always_fires() {
        assert!(trig_fires(&Always, 7, false, false));
    }

    #[test]
    fn ratio_1_4_fires_on_loops_0_4_8() {
        for l in 0..12 {
            assert_eq!(
                trig_fires(&Ratio { x: 1, y: 4 }, l, false, false),
                l % 4 == 0
            );
        }
    }

    #[test]
    fn ratio_2_4_fires_on_loop_1_5_9() {
        for l in 0..12 {
            assert_eq!(
                trig_fires(&Ratio { x: 2, y: 4 }, l, false, false),
                l % 4 == 1
            );
        }
    }

    #[test]
    fn ratio_y_zero_treated_as_always() {
        assert!(trig_fires(&Ratio { x: 1, y: 0 }, 3, false, false));
    }

    #[test]
    fn fill_and_notfill() {
        assert!(trig_fires(&Fill, 0, true, false));
        assert!(!trig_fires(&Fill, 0, false, false));
        assert!(!trig_fires(&NotFill, 0, true, false));
        assert!(trig_fires(&NotFill, 0, false, false));
    }

    #[test]
    fn first_and_notfirst() {
        assert!(trig_fires(&First, 0, false, true));
        assert!(!trig_fires(&First, 3, false, false));
        assert!(!trig_fires(&NotFirst, 0, false, true));
        assert!(trig_fires(&NotFirst, 3, false, false));
    }
}
