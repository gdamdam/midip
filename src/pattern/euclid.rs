/// Bjorklund's algorithm: distribute `pulses` onsets as evenly as possible over `steps`,
/// then left-rotate the resulting mask by `rotation % steps`.
///
/// Edge cases:
/// - `pulses == 0` → all `false`
/// - `pulses >= steps` → all `true`
/// - `steps == 0` → empty vec
pub fn bjorklund(pulses: usize, steps: usize, rotation: usize) -> Vec<bool> {
    if steps == 0 {
        return Vec::new();
    }
    if pulses == 0 {
        return vec![false; steps];
    }
    if pulses >= steps {
        return vec![true; steps];
    }

    // Standard Euclidean rhythm via Bresenham-style distribution.
    // We maintain two groups of patterns: "pattern" (starts with true) and "remainder"
    // (starts with false). Bjorklund repeatedly merges remainder into pattern.
    //
    // Representation: each group is a (prefix, count) pair — all elements of a group
    // are identical, so we only store one exemplar and a count.
    let mut pattern: Vec<bool> = vec![true];
    let mut remainder: Vec<bool> = vec![false];
    let mut pattern_count = pulses;
    let mut remainder_count = steps - pulses;

    loop {
        if remainder_count <= 1 {
            break;
        }
        if pattern_count < remainder_count {
            // Zip each pattern with one remainder.
            let mut new_pattern = pattern.clone();
            new_pattern.extend_from_slice(&remainder);
            let _new_remainder = remainder.clone(); // leftover (nothing to merge into)
            pattern = new_pattern;
            // remainder stays the same shape; now we have fewer of each
            let new_rem_count = remainder_count - pattern_count;
            let new_pat_count = pattern_count;
            remainder_count = new_rem_count;
            pattern_count = new_pat_count;
        } else {
            // More patterns than remainders: each remainder gets merged into one pattern.
            let mut new_pattern = pattern.clone();
            new_pattern.extend_from_slice(&remainder);
            let new_remainder = pattern.clone(); // leftover patterns become new remainder
            let new_rem_count = pattern_count - remainder_count;
            let new_pat_count = remainder_count;
            pattern = new_pattern;
            remainder = new_remainder;
            remainder_count = new_rem_count;
            pattern_count = new_pat_count;
        }
    }

    // Flatten: pattern_count copies of `pattern` + remainder_count copies of `remainder`.
    let mut mask: Vec<bool> = Vec::with_capacity(steps);
    for _ in 0..pattern_count {
        mask.extend_from_slice(&pattern);
    }
    for _ in 0..remainder_count {
        mask.extend_from_slice(&remainder);
    }

    // Left-rotate by `rotation % steps`.
    if !mask.is_empty() {
        let rot = rotation % mask.len();
        if rot > 0 {
            mask.rotate_left(rot);
        }
    }

    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bjorklund_edge_cases() {
        assert_eq!(bjorklund(0, 16, 0), vec![false; 16]);
        assert_eq!(bjorklund(16, 16, 0), vec![true; 16]);
        assert_eq!(bjorklund(4, 0, 0), vec![] as Vec<bool>);
    }

    #[test]
    fn bjorklund_4_16_gives_quarter_notes() {
        let mask = bjorklund(4, 16, 0);
        assert_eq!(mask.len(), 16);
        let on: Vec<usize> = mask.iter().enumerate().filter(|(_, &b)| b).map(|(i, _)| i).collect();
        assert_eq!(on, vec![0, 4, 8, 12]);
    }

    #[test]
    fn bjorklund_1_16_gives_single_onset_at_zero() {
        let mask = bjorklund(1, 16, 0);
        let on: Vec<usize> = mask.iter().enumerate().filter(|(_, &b)| b).map(|(i, _)| i).collect();
        assert_eq!(on, vec![0]);
    }

    #[test]
    fn bjorklund_rotation_shifts_left() {
        // 4 pulses in 16, rotation 1: {0,4,8,12} -> {3,7,11,15}
        let mask = bjorklund(4, 16, 1);
        let on: Vec<usize> = mask.iter().enumerate().filter(|(_, &b)| b).map(|(i, _)| i).collect();
        assert_eq!(on, vec![3, 7, 11, 15]);
    }
}
