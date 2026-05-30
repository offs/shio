use crate::message::{SortCol, SortDirection};
use shio_core::Download;
use std::cmp::Ordering;

pub(super) fn sorted(
    downloads: Vec<(&Download, Option<Vec<u32>>)>,
    col: SortCol,
    dir: SortDirection,
) -> Vec<(&Download, Option<Vec<u32>>)> {
    let (mut pinned, mut unpinned): (Vec<_>, Vec<_>) =
        downloads.into_iter().partition(|(d, _)| d.pinned);
    let cmp = |a: &(&Download, Option<Vec<u32>>), b: &(&Download, Option<Vec<u32>>)| {
        compare(a.0, b.0, col, dir)
    };
    pinned.sort_by(cmp);
    unpinned.sort_by(cmp);
    pinned.extend(unpinned);
    pinned
}

pub(super) fn pinned_first(
    downloads: Vec<(&Download, Option<Vec<u32>>)>,
) -> Vec<(&Download, Option<Vec<u32>>)> {
    let (mut pinned, unpinned): (Vec<_>, Vec<_>) =
        downloads.into_iter().partition(|(d, _)| d.pinned);
    pinned.extend(unpinned);
    pinned
}

fn compare(a: &Download, b: &Download, col: SortCol, dir: SortDirection) -> Ordering {
    let ord = match col {
        SortCol::Name => a.filename.to_lowercase().cmp(&b.filename.to_lowercase()),
        SortCol::Size => a.total_size.cmp(&b.total_size),
        SortCol::Progress => a
            .progress_percent()
            .partial_cmp(&b.progress_percent())
            .unwrap_or(Ordering::Equal),
        SortCol::Speed => a.speed.cmp(&b.speed),
        SortCol::Eta => a.eta_seconds().cmp(&b.eta_seconds()),
        SortCol::DateAdded => a.created_at.cmp(&b.created_at),
    };
    match dir {
        SortDirection::Ascending => ord,
        SortDirection::Descending => ord.reverse(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dl(name: &str, size: u64, pinned: bool) -> Download {
        let mut d = Download::new("http://example.com/x".to_string(), PathBuf::from("/tmp/x"));
        d.filename = name.to_string();
        d.total_size = Some(size);
        d.pinned = pinned;
        d
    }

    #[test]
    fn pinned_float_above_unpinned_by_name_asc() {
        let a = dl("aaa.zip", 100, false);
        let b = dl("zzz.zip", 200, true);
        let c = dl("mmm.zip", 300, false);
        let input = vec![(&a, None), (&b, None), (&c, None)];
        let sorted_out = sorted(input, SortCol::Name, SortDirection::Ascending);
        assert_eq!(sorted_out[0].0.filename, "zzz.zip");
        assert_eq!(sorted_out[1].0.filename, "aaa.zip");
        assert_eq!(sorted_out[2].0.filename, "mmm.zip");
    }

    #[test]
    fn pinned_float_above_unpinned_by_size_desc() {
        let a = dl("a.zip", 10, false);
        let b = dl("b.zip", 500, false);
        let c = dl("c.zip", 50, true);
        let input = vec![(&a, None), (&b, None), (&c, None)];
        let sorted_out = sorted(input, SortCol::Size, SortDirection::Descending);
        assert_eq!(sorted_out[0].0.filename, "c.zip");
        assert_eq!(sorted_out[1].0.filename, "b.zip");
        assert_eq!(sorted_out[2].0.filename, "a.zip");
    }

    #[test]
    fn within_pinned_group_current_sort_applies() {
        let a = dl("a.zip", 100, true);
        let b = dl("b.zip", 50, true);
        let input = vec![(&a, None), (&b, None)];
        let sorted_out = sorted(input, SortCol::Size, SortDirection::Ascending);
        assert_eq!(sorted_out[0].0.filename, "b.zip");
        assert_eq!(sorted_out[1].0.filename, "a.zip");
    }

    #[test]
    fn no_pinned_degenerates_to_flat_sort() {
        let a = dl("a.zip", 100, false);
        let b = dl("b.zip", 50, false);
        let input = vec![(&a, None), (&b, None)];
        let sorted_out = sorted(input, SortCol::Size, SortDirection::Ascending);
        assert_eq!(sorted_out[0].0.filename, "b.zip");
        assert_eq!(sorted_out[1].0.filename, "a.zip");
    }

    #[test]
    fn manual_order_still_keeps_pinned_first() {
        let a = dl("manual-first.zip", 100, false);
        let b = dl("pinned.zip", 50, true);
        let c = dl("manual-last.zip", 25, false);
        let input = vec![(&a, None), (&b, None), (&c, None)];

        let sorted_out = pinned_first(input);

        assert_eq!(sorted_out[0].0.filename, "pinned.zip");
        assert_eq!(sorted_out[1].0.filename, "manual-first.zip");
        assert_eq!(sorted_out[2].0.filename, "manual-last.zip");
    }
}
