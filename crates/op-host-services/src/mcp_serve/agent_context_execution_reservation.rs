//! Checked ledger arithmetic for the actor allowance held through group I/O.
pub(super) fn adjusted(total: usize, previous: usize, next: usize, limit: usize) -> Option<usize> {
    let total = total.checked_sub(previous)?.checked_add(next)?;
    (total <= limit).then_some(total)
}

pub(super) fn pin(
    actual: usize,
    previous: usize,
    total: usize,
    allowance: usize,
    path: usize,
    limit: usize,
) -> Option<(usize, usize)> {
    let floor = actual.checked_add(allowance)?.checked_add(path)?;
    Some((floor, adjusted(total, previous, floor, limit)?))
}

pub(crate) fn checkout(
    actual: usize,
    pinned: usize,
    transferred: bool,
    allowance: usize,
    path: usize,
) -> Option<usize> {
    if transferred {
        (pinned >= actual).then_some(pinned)
    } else {
        Some(
            actual
                .checked_add(allowance)?
                .checked_add(path)?
                .max(pinned),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_file_cannot_consume_the_final_save_or_rollback_allowance() {
        let limit = 1_000_000;
        let (floor, used) = pin(500, 500, 50_000, 65_536, 100, limit).unwrap();
        assert_eq!(used, 115_636);
        // Other document calls fill every remaining byte after the first stage.
        let occupied = limit;
        let grown = 32_000;
        let reserve = checkout(grown, floor, true, 65_536, 100).unwrap();
        assert_eq!(adjusted(occupied, floor, reserve, limit), Some(limit));
        let ordinary = checkout(grown, 0, false, 65_536, 100).unwrap();
        assert_eq!(adjusted(occupied, floor, ordinary, limit), None);
    }

    #[test]
    fn limits_and_overflows_refuse_before_changing_the_ledger() {
        assert!(pin(100, 100, 100, 100, 1, 200).is_none());
        assert!(pin(usize::MAX, 0, 0, 1, 0, usize::MAX).is_none());
        assert_eq!(adjusted(5, 6, 0, 100), None);
        assert_eq!(checkout(11, 10, true, 100, 0), None);
    }

    #[test]
    fn releasing_a_pin_refunds_only_unused_reserved_bytes() {
        let (floor, total) = pin(500, 500, 10_000, 65_536, 100, 1_000_000).unwrap();
        let actual_after_call = 1200;
        assert_eq!(
            adjusted(total, floor, actual_after_call, 1_000_000),
            Some(10_700)
        );
    }
}
