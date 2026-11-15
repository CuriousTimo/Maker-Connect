use super::*;

impl Store {
    pub(super) fn sample(&mut self, machine: &str, telemetry: &Telemetry) {
        let t = now();
        let history = self.history.entry(machine.to_owned()).or_default();
        if history
            .back()
            .is_some_and(|last| t - last.t < SAMPLE_SECONDS)
        {
            return;
        }
        history.push_back(Sample {
            t,
            nozzle: telemetry.nozzle_c,
            nozzle_target: telemetry.nozzle_target_c,
            bed: telemetry.bed_c,
            bed_target: telemetry.bed_target_c,
            print_fan: telemetry.print_fan_rpm,
            hotend_fan: telemetry.hotend_fan_rpm,
        });
        while history
            .front()
            .is_some_and(|first| t - first.t > HISTORY_SECONDS)
        {
            history.pop_front();
        }
    }

    /// A machine's temperatures over the last half hour, oldest first.
    pub fn history(&self, space: i64, machine: &str) -> rusqlite::Result<Option<Vec<Sample>>> {
        Ok(self.machine(space, machine)?.map(|_| {
            self.history
                .get(machine)
                .map(|h| h.iter().copied().collect())
                .unwrap_or_default()
        }))
    }
}
