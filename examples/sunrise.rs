use chrono::{Duration, Local};
use personal_power_ctrl::identity::Named;
use personal_power_ctrl::settings::{self, SourceSettings};

fn main() {
    let config = settings::read().expect("Failed reading config");

    for source_settings in config.source.day_night {
        let source = source_settings
            .create_source()
            .expect("expected to create source");

        let (is_day, next_switch) = source.state_at(Local::now());
        println!(
            "{}: is day: {} - next switch: {}",
            source_settings.base().name(),
            is_day,
            next_switch
        )
    }
}
