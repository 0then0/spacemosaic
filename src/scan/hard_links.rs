use super::{LimitReason, bytes, grown, table_bytes};
use crate::memory::Monitor;
use std::collections::{HashMap, hash_map::Entry};

pub(super) struct Observation {
    pub inode: (u64, u64), // Device and inode number.
    pub nlink: u64,
}

#[derive(Clone, Debug)]
pub struct HardLinkGroup {
    pub nodes: Vec<usize>,
    pub min_nlink: u64,
    pub max_nlink: u64,
}

#[derive(Clone, Debug)]
enum Member {
    First(HardLinkGroup),
    Other(usize),
}

/// The sparse index retained with a live local scan. A regular file with no
/// entry was observed once with a link count of one.
#[derive(Clone, Debug, Default)]
pub struct HardLinkGroups {
    members: HashMap<usize, Member>,
    paths_bytes: usize,
}

impl HardLinkGroups {
    pub fn get(&self, node: usize) -> Option<&HardLinkGroup> {
        let member = self.members.get(&node)?;
        match member {
            Member::First(group) => Some(group),
            Member::Other(first) => match self.members.get(first) {
                Some(Member::First(group)) => Some(group),
                _ => unreachable!("hard-link group must have a first member"),
            },
        }
    }

    pub(super) fn heap_bytes(&self) -> usize {
        table_bytes::<(usize, Member)>(self.members.capacity()).saturating_add(self.paths_bytes)
    }
}

#[derive(Default)]
pub(super) struct Tracker {
    // Keep the first path even for nlink == 1: a later observation can disagree.
    first_paths: HashMap<(u64, u64), usize>,
    pub groups: HardLinkGroups,
}

fn table_growth<K, V>(table: &HashMap<K, V>, additional: usize) -> usize {
    if additional > table.capacity() - table.len() {
        // Charge the new buckets while the old table is still allocated.
        table_bytes::<(K, V)>(table.capacity().max(4).saturating_mul(2))
    } else {
        0
    }
}

fn path_bytes(capacity: usize) -> usize {
    bytes::<usize>(capacity).saturating_add(if capacity > 0 { 16 } else { 0 })
}

impl Tracker {
    pub fn heap_bytes(&self) -> usize {
        table_bytes::<((u64, u64), usize)>(self.first_paths.capacity())
            .saturating_add(self.groups.heap_bytes())
    }

    /// Admit all new allocations before indexing a node that will be published.
    pub fn observe(
        &mut self,
        node: usize,
        file: Observation,
        tree_bytes: usize,
        monitor: &mut Monitor,
    ) -> Result<(), LimitReason> {
        let first = self.first_paths.get(&file.inode).copied();
        let new_inode = usize::from(first.is_none());
        let head = first.unwrap_or(node);
        let existing = self.groups.get(head);
        let new_members = match (first, existing, file.nlink) {
            (None, _, 1) => 0,
            (Some(_), None, _) => 2,
            _ => 1,
        };
        let old_path_capacity = existing.map_or(0, |group| group.nodes.capacity());
        let path_capacity = match existing {
            Some(group) => {
                grown(old_path_capacity, group.nodes.len()).ok_or(LimitReason::CapacityLimit)?
            }
            None if new_members > 0 => 2,
            None => 0,
        };
        let growth = table_growth(&self.first_paths, new_inode)
            .saturating_add(table_growth(&self.groups.members, new_members))
            .saturating_add(if path_capacity > old_path_capacity {
                path_bytes(path_capacity)
            } else {
                0
            });
        let current = tree_bytes.saturating_add(self.heap_bytes());
        if growth > 0
            && let Some(reason) =
                monitor.check_growth(current as u64, current.saturating_add(growth) as u64)
        {
            return Err(reason);
        }
        self.first_paths
            .try_reserve(new_inode)
            .map_err(|_| LimitReason::MemoryLimit)?;
        self.groups
            .members
            .try_reserve(new_members)
            .map_err(|_| LimitReason::MemoryLimit)?;

        if new_members > 0 {
            let member = match self.groups.members.entry(head) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let mut nodes = Vec::new();
                    nodes
                        .try_reserve_exact(path_capacity)
                        .map_err(|_| LimitReason::MemoryLimit)?;
                    nodes.extend(first);
                    let nlink = if first.is_some() { 1 } else { file.nlink };
                    entry.insert(Member::First(HardLinkGroup {
                        nodes,
                        min_nlink: nlink,
                        max_nlink: nlink,
                    }))
                }
            };
            let Member::First(group) = member else {
                unreachable!("observation must use the first member of the inode")
            };
            group
                .nodes
                .try_reserve_exact(path_capacity - group.nodes.len())
                .map_err(|_| LimitReason::MemoryLimit)?;
            group.nodes.push(node);
            group.min_nlink = group.min_nlink.min(file.nlink);
            group.max_nlink = group.max_nlink.max(file.nlink);
            self.groups.paths_bytes = self.groups.paths_bytes.saturating_add(
                path_bytes(group.nodes.capacity()).saturating_sub(path_bytes(old_path_capacity)),
            );
            if first.is_some() {
                self.groups.members.insert(node, Member::Other(head));
            }
        }
        if first.is_none() {
            self.first_paths.insert(file.inode, node);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{BudgetMode, Snapshot};

    fn monitor() -> Monitor {
        Monitor::simulated(BudgetMode::Manual { mib: 16 }, Snapshot::default(), None)
    }

    #[test]
    fn identical_inode_numbers_on_different_devices_form_separate_groups() {
        let mut tracker = Tracker::default();
        let mut monitor = monitor();
        for (node, device) in [(1, 10), (2, 11), (3, 10), (4, 11)] {
            let inode = (device, 42);
            tracker
                .observe(node, Observation { inode, nlink: 2 }, 0, &mut monitor)
                .unwrap();
        }
        for (node, paths) in [(1, [1, 3]), (2, [2, 4]), (3, [1, 3]), (4, [2, 4])] {
            assert_eq!(tracker.groups.get(node).unwrap().nodes, paths);
        }
    }

    #[test]
    fn changing_link_counts_include_the_path_that_reported_one_link() {
        for counts in [[1, 3], [3, 1]] {
            let mut tracker = Tracker::default();
            let mut monitor = monitor();
            for (node, nlink) in counts.into_iter().enumerate() {
                tracker
                    .observe(
                        node,
                        Observation {
                            inode: (10, 42),
                            nlink,
                        },
                        0,
                        &mut monitor,
                    )
                    .unwrap();
            }
            for node in 0..2 {
                let group = tracker.groups.get(node).unwrap();
                assert_eq!(group.nodes, [0, 1]);
                assert_eq!((group.min_nlink, group.max_nlink), (1, 3));
            }
        }
    }

    #[test]
    fn path_buffer_growth_is_admitted_before_publishing_another_member() {
        let mut tracker = Tracker::default();
        let mut monitor = monitor();
        let file = || Observation {
            inode: (10, 42),
            nlink: 3,
        };
        for node in 1..=2 {
            tracker.observe(node, file(), 0, &mut monitor).unwrap();
        }
        let tree_bytes = monitor.diagnostics.budget_bytes as usize - tracker.heap_bytes();
        assert_eq!(
            tracker.observe(3, file(), tree_bytes, &mut monitor),
            Err(LimitReason::MemoryLimit)
        );
        assert_eq!(tracker.groups.get(1).unwrap().nodes, [1, 2]);
        assert!(tracker.groups.get(3).is_none());
    }
}
