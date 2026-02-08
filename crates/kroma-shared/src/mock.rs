//! Mock implementations of core traits for testing.

use glam::Vec2;

use crate::traits::DataProvider;
use crate::types::SystemStats;

/// A mock `DataProvider` that returns fixed values.
///
/// Useful for deterministic unit tests and offline development.
pub struct MockDataProvider {
    pub audio_spectrum: Vec<f32>,
    pub system_stats: SystemStats,
    pub cursor_pos: Vec2,
}

impl Default for MockDataProvider {
    fn default() -> Self {
        Self {
            audio_spectrum: vec![0.0; 512],
            system_stats: SystemStats {
                cpu_usage: 25.0,
                ram_total: 16_000_000_000,
                ram_used: 8_000_000_000,
                battery: Some(75.0),
            },
            cursor_pos: Vec2::new(0.5, 0.5),
        }
    }
}

impl DataProvider for MockDataProvider {
    fn get_audio_spectrum(&self) -> Vec<f32> {
        self.audio_spectrum.clone()
    }

    fn get_system_stats(&self) -> SystemStats {
        self.system_stats.clone()
    }

    fn get_cursor_pos(&self) -> Vec2 {
        self.cursor_pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_provider_defaults() {
        let mock = MockDataProvider::default();
        assert_eq!(mock.get_audio_spectrum().len(), 512);
        assert_eq!(mock.get_system_stats().cpu_usage, 25.0);
        assert_eq!(mock.get_cursor_pos(), Vec2::new(0.5, 0.5));
    }

    #[test]
    fn mock_provider_custom() {
        let mock = MockDataProvider {
            cursor_pos: Vec2::new(0.0, 1.0),
            system_stats: SystemStats {
                cpu_usage: 99.0,
                ram_total: 32_000_000_000,
                ram_used: 30_000_000_000,
                battery: None,
            },
            ..Default::default()
        };
        assert_eq!(mock.get_cursor_pos(), Vec2::new(0.0, 1.0));
        assert!(mock.get_system_stats().battery.is_none());
        assert_eq!(mock.get_system_stats().cpu_usage, 99.0);
    }
}
