//! Connection-scoped rendering demand from visible external consumers.

use std::collections::{HashMap, HashSet};

/// Aggregates output names requested by independent IPC connections.
#[derive(Clone, Default)]
pub struct RenderDemand {
    consumers: HashMap<u64, HashSet<String>>,
}

impl RenderDemand {
    /// Replaces a connection's demand; an empty list releases it.
    pub fn set(&mut self, consumer: u64, monitors: Vec<String>) {
        if monitors.is_empty() {
            self.remove(consumer);
        } else {
            self.consumers
                .insert(consumer, monitors.into_iter().collect());
        }
    }

    /// Releases all demand owned by a disconnected consumer.
    pub fn remove(&mut self, consumer: u64) {
        self.consumers.remove(&consumer);
    }

    /// Returns the union of all currently demanded outputs.
    pub fn monitors(&self) -> HashSet<String> {
        self.consumers.values().flatten().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_consumers_are_independent() {
        let mut demand = RenderDemand::default();
        demand.set(1, vec!["eDP-1".into(), "DP-1".into()]);
        demand.set(2, vec!["DP-1".into()]);
        demand.remove(1);
        assert_eq!(demand.monitors(), HashSet::from(["DP-1".into()]));
        demand.remove(2);
        assert!(demand.monitors().is_empty());
    }

    #[test]
    fn replacing_and_clearing_demand_releases_old_outputs() {
        let mut demand = RenderDemand::default();
        demand.set(7, vec!["eDP-1".into()]);
        demand.set(7, vec!["DP-1".into(), "DP-1".into()]);
        assert_eq!(demand.monitors(), HashSet::from(["DP-1".into()]));
        demand.set(7, Vec::new());
        assert!(demand.monitors().is_empty());
    }
}
