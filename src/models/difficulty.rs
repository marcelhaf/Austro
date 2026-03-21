pub const RETARGET_INTERVAL:        u64   = 2016;
pub const TARGET_BLOCK_TIME_SECS:   u64   = 600;
pub const TARGET_TIMESPAN:          u64   = RETARGET_INTERVAL * TARGET_BLOCK_TIME_SECS;
pub const MAX_ADJUSTMENT_FACTOR:    u64   = 4;
pub const MIN_DIFFICULTY:           usize = 4;
pub const MAX_DIFFICULTY:           usize = 64;

pub fn calculate_next_difficulty(current_difficulty: usize, actual_timespan: u64) -> usize {
    let clamped = actual_timespan.clamp(
        TARGET_TIMESPAN / MAX_ADJUSTMENT_FACTOR,
        TARGET_TIMESPAN * MAX_ADJUSTMENT_FACTOR,
    );
    let scaled         = (current_difficulty as u64) * 1000 * TARGET_TIMESPAN / clamped;
    let new_difficulty = ((scaled + 500) / 1000) as usize;
    new_difficulty.clamp(MIN_DIFFICULTY, MAX_DIFFICULTY)
}

pub fn is_retarget_block(height: u64) -> bool {
    height > 0 && height.is_multiple_of(RETARGET_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_change_when_on_target() {
        assert_eq!(calculate_next_difficulty(8, TARGET_TIMESPAN), 8);
    }

    #[test]
    fn test_increase_when_too_fast() {
        assert_eq!(calculate_next_difficulty(8, TARGET_TIMESPAN / 4), 32);
    }

    #[test]
    fn test_decrease_when_too_slow() {
        assert_eq!(calculate_next_difficulty(32, TARGET_TIMESPAN * 4), 8);
    }

    #[test]
    fn test_clamp_prevents_extreme_drop() {
        assert_eq!(calculate_next_difficulty(32, TARGET_TIMESPAN * 100), 8);
    }

    #[test]
    fn test_clamp_prevents_extreme_rise() {
        assert_eq!(calculate_next_difficulty(8, TARGET_TIMESPAN / 100), 32);
    }

    #[test]
    fn test_minimum_difficulty_floor() {
        assert_eq!(calculate_next_difficulty(MIN_DIFFICULTY, TARGET_TIMESPAN * 100), MIN_DIFFICULTY);
    }
}