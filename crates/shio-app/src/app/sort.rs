use super::state::Row;
use crate::message::SortKey;
use std::cmp::Ordering;

pub(super) fn sort_rows<'a, T: Ord>(
    rows: &mut [Row],
    key: SortKey,
    descending: bool,
    lookup: impl Fn(&Row) -> (&'a str, T),
) {
    rows.sort_by(|a, b| {
        let order = match key {
            SortKey::Name => compare_names(lookup(a).0, lookup(b).0),
            SortKey::Size => a.total.cmp(&b.total),
            SortKey::Progress => compare_progress(a, b),
            SortKey::Added => lookup(a).1.cmp(&lookup(b).1),
        };
        b.pinned
            .cmp(&a.pinned)
            .then(if descending { order.reverse() } else { order })
    });
}

fn compare_names(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

fn compare_progress(a: &Row, b: &Row) -> Ordering {
    match (a.total.filter(|&t| t > 0), b.total.filter(|&t| t > 0)) {
        (Some(a_total), Some(b_total)) => (u128::from(a.downloaded) * u128::from(b_total))
            .cmp(&(u128::from(b.downloaded) * u128::from(a_total))),
        (a_total, b_total) => a_total.is_some().cmp(&b_total.is_some()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::RowSource;
    use shio_core::DownloadStatus;

    fn row(index: usize, downloaded: u64, total: u64, pinned: bool) -> Row {
        Row {
            id: crate::app::tests::id(index),
            source: RowSource::Download(index),
            status: DownloadStatus::Queued,
            downloaded,
            total: Some(total),
            speed: 0,
            pinned,
            torrent: None,
        }
    }

    const fn index(row: &Row) -> usize {
        match row.source {
            RowSource::Download(index) | RowSource::Package(index) => index,
        }
    }

    #[test]
    fn pinned_rows_stay_first_whatever_the_key_and_direction() {
        let names = ["b", "A", "c", "D"];
        let mut rows = vec![
            row(0, 250, 500, false),
            row(1, 100, 100, true),
            row(2, 1, 10, false),
            row(3, 0, 50, true),
        ];
        for (key, descending, expected) in [
            (SortKey::Size, false, [3, 1, 2, 0]),
            (SortKey::Size, true, [1, 3, 0, 2]),
            (SortKey::Name, false, [1, 3, 0, 2]),
            (SortKey::Progress, true, [1, 3, 0, 2]),
            (SortKey::Added, true, [3, 1, 2, 0]),
        ] {
            sort_rows(&mut rows, key, descending, |row| {
                (names[index(row)], index(row))
            });
            let order: Vec<usize> = rows.iter().map(index).collect();
            assert_eq!(order, expected, "{key:?} descending={descending}");
        }
    }
}
