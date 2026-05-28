//! JSON composition fixtures used only by parse benchmarks (not render workloads).

pub struct JsonFixture {
    pub name: &'static str,
    pub source: &'static str,
}

pub const JSON_FIXTURES: &[JsonFixture] = &[
    JsonFixture {
        name: "announcement_gpu",
        source: include_str!("../../../local/demo/announcement-gpu.json"),
    },
    JsonFixture {
        name: "feature_showcase",
        source: include_str!("../../../local/demo/feature-showcase.json"),
    },
    JsonFixture {
        name: "antialiasing_worst_cases_aa",
        source: include_str!("../../../local/demo/antialiasing-worst-cases.json"),
    },
    JsonFixture {
        name: "antialiasing_worst_cases_noaa",
        source: include_str!("../../../local/demo/antialiasing-worst-cases-noaa.json"),
    },
    JsonFixture {
        name: "antialiasing_check",
        source: include_str!("../../../local/demo/antialiasing-check.json"),
    },
];

pub fn fixture(name: &str) -> Option<&'static JsonFixture> {
    JSON_FIXTURES.iter().find(|fixture| fixture.name == name)
}
