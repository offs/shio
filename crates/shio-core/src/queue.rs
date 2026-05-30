use crate::types::DownloadId;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
struct QueueEntry {
    id: DownloadId,
    priority: i32,
}

#[derive(Debug)]
pub(crate) struct DownloadQueue {
    entries: VecDeque<QueueEntry>,
}

impl DownloadQueue {
    pub(crate) const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    pub(crate) fn push(&mut self, id: DownloadId, priority: i32) {
        let pos = self
            .entries
            .iter()
            .position(|e| e.priority < priority)
            .unwrap_or(self.entries.len());
        self.entries.insert(pos, QueueEntry { id, priority });
    }

    pub(crate) fn pop(&mut self) -> Option<DownloadId> {
        self.entries.pop_front().map(|e| e.id)
    }

    pub(crate) fn remove(&mut self, id: &DownloadId) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| &e.id == id) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for DownloadQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pop_empty_returns_none() {
        let mut q = DownloadQueue::new();
        assert_eq!(q.pop(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn higher_priority_pops_first() {
        let mut q = DownloadQueue::new();
        let low = DownloadId::new();
        let high = DownloadId::new();
        q.push(low, 0);
        q.push(high, 10);
        assert_eq!(q.pop(), Some(high));
        assert_eq!(q.pop(), Some(low));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn equal_priority_is_fifo() {
        let mut q = DownloadQueue::new();
        let a = DownloadId::new();
        let b = DownloadId::new();
        let c = DownloadId::new();
        q.push(a, 5);
        q.push(b, 5);
        q.push(c, 5);
        assert_eq!(q.pop(), Some(a));
        assert_eq!(q.pop(), Some(b));
        assert_eq!(q.pop(), Some(c));
    }

    #[test]
    fn mixed_priority_preserves_fifo_within_group() {
        let mut q = DownloadQueue::new();
        let low1 = DownloadId::new();
        let high1 = DownloadId::new();
        let low2 = DownloadId::new();
        let high2 = DownloadId::new();
        q.push(low1, 0);
        q.push(high1, 10);
        q.push(low2, 0);
        q.push(high2, 10);
        assert_eq!(q.pop(), Some(high1));
        assert_eq!(q.pop(), Some(high2));
        assert_eq!(q.pop(), Some(low1));
        assert_eq!(q.pop(), Some(low2));
    }

    #[test]
    fn remove_preserves_order_of_rest() {
        let mut q = DownloadQueue::new();
        let a = DownloadId::new();
        let b = DownloadId::new();
        let c = DownloadId::new();
        q.push(a, 0);
        q.push(b, 0);
        q.push(c, 0);
        assert!(q.remove(&b));
        assert_eq!(q.pop(), Some(a));
        assert_eq!(q.pop(), Some(c));
    }

    #[test]
    fn remove_missing_returns_false() {
        let mut q = DownloadQueue::new();
        let a = DownloadId::new();
        let ghost = DownloadId::new();
        q.push(a, 0);
        assert!(!q.remove(&ghost));
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn len_tracks_push_and_pop() {
        let mut q = DownloadQueue::new();
        assert_eq!(q.len(), 0);
        q.push(DownloadId::new(), 0);
        q.push(DownloadId::new(), 0);
        assert_eq!(q.len(), 2);
        q.pop();
        assert_eq!(q.len(), 1);
        q.pop();
        assert_eq!(q.len(), 0);
        assert!(q.is_empty());
    }
}
