use std::time::Duration;

use super::timing::PhaseTimer;

/// Collects tabular rows and prints an aligned summary after detailed logs.
#[derive(Debug, Default)]
pub struct SummaryReport {
    title: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl SummaryReport {
    pub fn new(
        title: impl Into<String>,
        columns: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Self {
        Self {
            title: title.into(),
            columns: columns
                .into_iter()
                .map(|column| column.as_ref().to_string())
                .collect(),
            rows: Vec::new(),
        }
    }

    pub fn push_row(&mut self, cells: Vec<String>) {
        debug_assert_eq!(
            cells.len(),
            self.columns.len(),
            "summary row width must match column count"
        );
        self.rows.push(cells);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn print(&self) {
        if self.rows.is_empty() {
            return;
        }

        let mut widths: Vec<usize> = self.columns.iter().map(|column| column.len()).collect();
        for row in &self.rows {
            for (index, cell) in row.iter().enumerate() {
                if let Some(width) = widths.get_mut(index) {
                    *width = (*width).max(cell.len());
                }
            }
        }

        println!();
        println!("{}", self.title);
        println!(
            "{}",
            "─".repeat(widths.iter().sum::<usize>() + (widths.len().saturating_sub(1)) * 3)
        );

        print_table_row(&self.columns, &widths);
        println!(
            "{}",
            widths
                .iter()
                .map(|width| "─".repeat(*width))
                .collect::<Vec<_>>()
                .join(" │ ")
        );

        for row in &self.rows {
            print_table_row(row, &widths);
        }
        println!();
    }
}

fn print_table_row(cells: &[String], widths: &[usize]) {
    let line = cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| format!("{cell:width$}"))
        .collect::<Vec<_>>()
        .join(" │ ");
    println!("{line}");
}

pub fn format_duration(duration: Duration) -> String {
    let micros = duration.as_secs_f64() * 1_000_000.0;
    if micros < 1_000.0 {
        format!("{micros:.1} µs")
    } else if micros < 1_000_000.0 {
        format!("{:.2} ms", micros / 1_000.0)
    } else {
        format!("{:.2} s", micros / 1_000_000.0)
    }
}

pub fn format_fps(frames: u32, elapsed: Duration) -> String {
    if frames == 0 || elapsed.is_zero() {
        return "-".to_string();
    }
    format!("{:.1}", f64::from(frames) / elapsed.as_secs_f64().max(1e-9))
}

pub fn phase_mean(timer: &PhaseTimer, phase: &str) -> Option<Duration> {
    let samples: Vec<Duration> = timer
        .phases()
        .iter()
        .filter(|(name, _)| *name == phase)
        .map(|(_, duration)| *duration)
        .collect();
    if samples.is_empty() {
        return None;
    }
    let total_micros: u128 = samples.iter().map(|duration| duration.as_micros()).sum();
    let mean_micros = (total_micros / samples.len() as u128).max(1);
    Some(Duration::from_micros(mean_micros as u64))
}

pub fn phase_first(timer: &PhaseTimer, phase: &str) -> Option<Duration> {
    timer
        .phases()
        .iter()
        .find(|(name, _)| *name == phase)
        .map(|(_, duration)| *duration)
}
