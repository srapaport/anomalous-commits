use crate::env::{AuthorInfo, CommitterInfo};
#[cfg(feature = "plotting")]
use plotters::prelude::*;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Commit {
    pub id: String,
    pub timestamp: i64, // Unix timestamp in nanoseconds
    pub tz_offset: i16, // Timezone offset in minutes from UTC
}

impl Commit {
    pub fn from_tuple(tuple: &(String, i64, i16)) -> Self {
        Commit {
            id: tuple.0.clone(),
            timestamp: tuple.1,
            tz_offset: tuple.2,
        }
    }

    pub fn hour_utc(&self) -> usize {
        let timestamp_secs = self.timestamp / 1_000_000_000;
        let seconds_in_day = timestamp_secs % 86400;
        let hour = (seconds_in_day / 3600) as usize;
        hour % 24
    }
}

/// Time-Of-Day profile for detecting unusual commit times
#[derive(Debug, Clone)]
pub struct TimeOfDayProfile {
    commit_counts: [u32; 24], // Histogram for 24 hours (UTC)
    total_commits: u32,
}

impl TimeOfDayProfile {
    pub fn new() -> Self {
        Self {
            commit_counts: [0; 24],
            total_commits: 0,
        }
    }

    pub fn learn(&mut self, commits: &[Commit]) {
        self.commit_counts = [0; 24];
        self.total_commits = commits.len() as u32;

        for commit in commits {
            let hour = commit.hour_utc();
            self.commit_counts[hour] += 1;
        }
    }

    fn get_tod_count(&self, hour: usize) -> u32 {
        self.commit_counts[hour % 24]
    }

    /// Calculate probability of a commit at a given hour using 3-hour average
    ///
    /// Formula from paper:
    /// p(x) = (tod(x-1) + tod(x) + tod(x+1)) / (3 * sum of all tod(i))
    fn calculate_probability(&self, hour: usize) -> f64 {
        if self.total_commits == 0 {
            return 0.0;
        }

        let prev_hour = if hour == 0 { 23 } else { hour - 1 };
        let next_hour = (hour + 1) % 24;

        let three_hour_sum = self.get_tod_count(prev_hour)
            + self.get_tod_count(hour)
            + self.get_tod_count(next_hour);

        three_hour_sum as f64 / (3.0 * self.total_commits as f64)
    }

    /// Calculate anomaly score for a commit
    ///
    /// Formula from paper:
    /// F(x) = 1 - p(x)^log_24(2)
    ///
    /// Returns:
    /// - 0.5 for average commit time
    /// - Values closer to 1 for unusual (rare) times
    /// - Values closer to 0 for very common times
    pub fn get_anomaly_score(&self, commit: &Commit) -> f64 {
        let hour = commit.hour_utc();
        let probability = self.calculate_probability(hour);

        // Baseline probability: 1/24
        // Normalization: log_24(2) ≈ 0.2181
        const NORMALIZATION_FACTOR: f64 = 0.2181;

        let score = 1.0 - probability.powf(NORMALIZATION_FACTOR);

        // Clamp to [0, 1]
        score.max(0.0).min(1.0)
    }

    // pub fn get_explanation(&self, commit: &Commit) -> String {
    //     let hour = commit.hour_utc();
    //     let probability = self.calculate_probability(hour);
    //     let percentage = probability * 100.0;
    //     let anomaly_score = self.get_anomaly_score(commit);

    //     let time_str = format!("{:02}:00 UTC", hour);

    //     if anomaly_score > 0.6 {
    //         format!(
    //             "Commit made at {} - an unusual time (only {:.1}% of commits occur around this time)",
    //             time_str, percentage
    //         )
    //     } else if anomaly_score < 0.4 {
    //         format!(
    //             "Commit made at {} - a common time ({:.1}% of commits occur around this time)",
    //             time_str, percentage
    //         )
    //     } else {
    //         format!(
    //             "Commit made at {} - a typical time ({:.1}% of commits occur around this time)",
    //             time_str, percentage
    //         )
    //     }
    // }

    pub fn get_distribution(&self) -> HashMap<usize, f64> {
        let mut distribution = HashMap::new();

        if self.total_commits == 0 {
            for hour in 0..24 {
                distribution.insert(hour, 0.0);
            }
            return distribution;
        }

        for hour in 0..24 {
            let percentage = (self.commit_counts[hour] as f64 / self.total_commits as f64) * 100.0;
            distribution.insert(hour, percentage);
        }

        distribution
    }

    /// Plot the commit time distribution to a PNG file
    ///
    /// # Arguments
    /// * `output_path` - Path where the PNG file will be saved
    /// * `title` - Title for the plot
    ///
    /// # Returns
    /// Result indicating success or error message
    #[cfg(feature = "plotting")]
    pub fn plot_distribution(
        &self,
        output_path: &str,
        title: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let distribution = self.get_distribution();

        // Create the drawing area
        let root = BitMapBackend::new(output_path, (1200, 600)).into_drawing_area();
        root.fill(&WHITE)?;

        // Find max value for Y axis
        let max_percentage = distribution
            .values()
            .cloned()
            .fold(0.0f64, f64::max)
            .max(5.0); // Minimum 5% for better visualization

        let mut chart = ChartBuilder::on(&root)
            .caption(title, ("sans-serif", 40).into_font())
            .margin(20)
            .x_label_area_size(50)
            .y_label_area_size(60)
            .build_cartesian_2d(0u32..23u32, 0f64..(max_percentage * 1.1))?;

        chart
            .configure_mesh()
            .x_desc("Hour of Day (UTC)")
            .y_desc("Percentage of Commits (%)")
            .x_labels(24)
            .y_labels(10)
            .draw()?;

        // Draw bars
        chart.draw_series((0..24).map(|hour| {
            let percentage = distribution.get(&hour).unwrap_or(&0.0);
            let x0 = hour as u32;
            let x1 = x0;

            Rectangle::new([(x0, 0.0), (x1, *percentage)], BLUE.mix(0.7).filled())
        }))?;

        // Draw bar outlines
        chart.draw_series((0..24).map(|hour| {
            let percentage = distribution.get(&hour).unwrap_or(&0.0);
            let x0 = hour as u32;
            let x1 = x0;

            Rectangle::new([(x0, 0.0), (x1, *percentage)], BLUE.stroke_width(2))
        }))?;

        root.present()?;
        println!("Plot saved to: {}", output_path);

        Ok(())
    }
}

pub struct AuthorProfiles {
    profiles: HashMap<String, TimeOfDayProfile>,
}

impl AuthorProfiles {
    pub fn new() -> Self {
        Self {
            profiles: HashMap::new(),
        }
    }

    pub fn learn_all(&mut self, author_commits: HashMap<String, Vec<(String, i64, i16)>>) {
        for (author_id, commit_data) in author_commits {
            let commits: Vec<Commit> = commit_data
                .iter()
                .map(|tuple| Commit::from_tuple(tuple))
                .collect();

            let mut profile = TimeOfDayProfile::new();
            profile.learn(&commits);
            self.profiles.insert(author_id, profile);
        }
    }

    pub fn learn_from_author_info(&mut self, author_id: String, author_info: &AuthorInfo) {
        let commits: Vec<Commit> = author_info
            .commits
            .iter()
            .map(|tuple| Commit::from_tuple(tuple))
            .collect();

        let mut profile = TimeOfDayProfile::new();
        profile.learn(&commits);
        self.profiles.insert(author_id, profile);
    }

    pub fn learn_from_committer_info(
        &mut self,
        committer_id: String,
        committer_info: &CommitterInfo,
    ) {
        let commits: Vec<Commit> = committer_info
            .commits
            .iter()
            .map(|tuple| Commit::from_tuple(tuple))
            .collect();

        let mut profile = TimeOfDayProfile::new();
        profile.learn(&commits);
        self.profiles.insert(committer_id, profile);
    }

    pub fn get_anomaly_score(&self, person_id: &str, commit: &Commit) -> Option<f64> {
        self.profiles
            .get(person_id)
            .map(|profile| profile.get_anomaly_score(commit))
    }

    // pub fn get_explanation(&self, person_id: &str, commit: &Commit) -> Option<String> {
    //     self.profiles.get(person_id).map(|profile| profile.get_explanation(commit))
    // }

    pub fn get_profile(&self, person_id: &str) -> Option<&TimeOfDayProfile> {
        self.profiles.get(person_id)
    }

    // pub fn get_all_ids(&self) -> Vec<&String> {
    //     self.profiles.keys().collect()
    // }

    pub fn detect_anomalies(&self, person_id: &str, threshold: f64) -> Option<Vec<(String, f64)>> {
        let profile = self.profiles.get(person_id)?;
        let person_commits = self.get_profile_commits(person_id)?;

        let mut anomalies = Vec::new();
        for commit in person_commits {
            let score = profile.get_anomaly_score(&commit);
            if score >= threshold {
                //let explanation = profile.get_explanation(&commit);
                anomalies.push((commit.id.clone(), score));
            }
        }

        Some(anomalies)
    }

    fn get_profile_commits(&self, _person_id: &str) -> Option<Vec<Commit>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_of_day_profile() {
        // Create sample commits (timestamps in nanoseconds)
        let commits = vec![
            Commit {
                id: "1".to_string(),
                timestamp: 1705315800_000_000_000,
                tz_offset: 0,
            }, // ~9:30 AM
            Commit {
                id: "2".to_string(),
                timestamp: 1705318500_000_000_000,
                tz_offset: 0,
            }, // ~10:15 AM
            Commit {
                id: "3".to_string(),
                timestamp: 1705334400_000_000_000,
                tz_offset: 0,
            }, // ~2:00 PM
            Commit {
                id: "4".to_string(),
                timestamp: 1705336200_000_000_000,
                tz_offset: 0,
            }, // ~2:30 PM
            Commit {
                id: "5".to_string(),
                timestamp: 1705340700_000_000_000,
                tz_offset: 0,
            }, // ~3:45 PM
        ];

        let mut profile = TimeOfDayProfile::new();
        profile.learn(&commits);

        // Test normal commit time
        let normal_commit = Commit {
            id: "test1".to_string(),
            timestamp: 1705315800_000_000_000,
            tz_offset: 0,
        };
        let score = profile.get_anomaly_score(&normal_commit);
        assert!(score < 0.6, "Normal time should have low anomaly score");

        // Test unusual commit time (late night)
        let unusual_commit = Commit {
            id: "test2".to_string(),
            timestamp: 1705280400_000_000_000, // ~3:00 AM
            tz_offset: 0,
        };
        let score = profile.get_anomaly_score(&unusual_commit);
        assert!(score > 0.6, "Unusual time should have high anomaly score");
    }

    #[test]
    fn test_author_profiles() {
        let mut author_commits = HashMap::new();

        // Author 1: Works during day (timestamps in nanoseconds)
        author_commits.insert(
            "alice".to_string(),
            vec![
                ("c1".to_string(), 1705315800_000_000_000, 0), // 9:30 AM
                ("c2".to_string(), 1705318500_000_000_000, 0), // 10:15 AM
                ("c3".to_string(), 1705334400_000_000_000, 0), // 2:00 PM
            ],
        );

        // Author 2: Night owl
        author_commits.insert(
            "bob".to_string(),
            vec![
                ("c4".to_string(), 1705280400_000_000_000, 0), // 3:00 AM
                ("c5".to_string(), 1705284000_000_000_000, 0), // 4:00 AM
                ("c6".to_string(), 1705287600_000_000_000, 0), // 5:00 AM
            ],
        );

        let mut profiles = AuthorProfiles::new();
        profiles.learn_all(author_commits);

        // Test Alice's profile
        let day_commit = Commit {
            id: "test".to_string(),
            timestamp: 1705315800_000_000_000,
            tz_offset: 0,
        };
        let alice_score = profiles.get_anomaly_score("alice", &day_commit).unwrap();
        assert!(alice_score < 0.6, "Day commit should be normal for Alice");

        // Test Bob's profile
        let bob_score = profiles.get_anomaly_score("bob", &day_commit).unwrap();
        assert!(bob_score > 0.6, "Day commit should be unusual for Bob");
    }
}
