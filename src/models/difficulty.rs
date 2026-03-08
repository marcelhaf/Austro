// Difficulty adjustment — Bitcoin retarget algorithm.
// Every RETARGET_INTERVAL blocks, compare actual elapsed time against
// the expected time. Clamp adjustment to 4x in either direction.
// Bitcoin reference: src/pow.cpp GetNextWorkRequired()

/// How many blocks between each difficulty retarget.
pub const RETARGET_INTERVAL: u64 = 2016;

/// Target time per block in seconds (10 minutes, same as Bitcoin).
pub const TARGET_BLOCK_TIME_SECS: u64 = 60; // 1 minute for demo purposes

/// Expected total time for RETARGET_INTERVAL blocks.
pub const TARGET_TIMESPAN: u64 = RETARGET_INTERVAL * TARGET_BLOCK_TIME_SECS;

/// Maximum adjustment factor per retarget (4x up or down).
pub const MAX_ADJUSTMENT_FACTOR: u64 = 4;

/// Minimum difficulty (number of leading zero bits).
pub const MIN_DIFFICULTY: usize = 1;

/// Maximum difficulty cap for this implementation.
pub const MAX_DIFFICULTY: usize = 64;

/// Calculate the new difficulty given the current difficulty and
/// the actual elapsed time across the last RETARGET_INTERVAL blocks.
///
/// Returns the new difficulty (number of leading zero hex chars).
pub fn calculate_next_difficulty(current_difficulty: usize, actual_timespan: u64) -> usize {
    // Clamp actual timespan to [TARGET/4, TARGET*4]
    // This prevents extreme jumps from slow/fast mining periods
    let clamped = actual_timespan
        .max(TARGET_TIMESPAN / MAX_ADJUSTMENT_FACTOR)
        .min(TARGET_TIMESPAN * MAX_ADJUSTMENT_FACTOR);

    // new_difficulty = current * (target_timespan / actual_timespan)
    // We scale by 1000 to avoid integer division precision loss
    let scaled = (current_difficulty as u64) * 1000 * TARGET_TIMESPAN / clamped;
    let new_difficulty = ((scaled + 500) / 1000) as usize; // round

    new_difficulty.max(MIN_DIFFICULTY).min(MAX_DIFFICULTY)
}

/// Returns true if this block height triggers a retarget.
pub fn is_retarget_block(height: u64) -> bool {
    height > 0 && height % RETARGET_INTERVAL == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_change_when_on_target() {
        let new = calculate_next_difficulty(2, TARGET_TIMESPAN);
        assert_eq!(new, 2);
    }

    #[test]
    fn test_increase_when_too_fast() {
        // Blocks mined 4x faster than target
        let new = calculate_next_difficulty(2, TARGET_TIMESPAN / 4);
        assert_eq!(new, 8);
    }

    #[test]
    fn test_decrease_when_too_slow() {
        // Blocks mined 4x slower than target
        let new = calculate_next_difficulty(8, TARGET_TIMESPAN * 4);
        assert_eq!(new, 2);
    }

    #[test]
    fn test_clamp_prevents_extreme_drop() {
        // Even if 100x slower, max drop is 4x
        let new = calculate_next_difficulty(8, TARGET_TIMESPAN * 100);
        assert_eq!(new, 2); // 8/4 = 2
    }

    #[test]
    fn test_clamp_prevents_extreme_rise() {
        // Even if 100x faster, max rise is 4x
        let new = calculate_next_difficulty(2, TARGET_TIMESPAN / 100);
        assert_eq!(new, 8); // 2*4 = 8
    }

    #[test]
    fn test_minimum_difficulty_floor() {
        let new = calculate_next_difficulty(1, TARGET_TIMESPAN * 100);
        assert_eq!(new, MIN_DIFFICULTY);
    }
}
