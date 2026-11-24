//! A Marlin printer's job number, as PrusaLink keeps it (`reference/Prusa-Link` at `171148c`,
//! `prusa/link/printer_adapter/job.py`, `const.py:50-59`, `structures/module_data_classes.py:93-100`),
//! ported for what the job number needs: the job's state (`JobState`, `model_classes.py:103-107`),
//! `state_changed` (`job.py:122-133`), `tick` (`:135-138`, called from `post_state_change`,
//! `prusa_link.py:1026-1029`), `job_started` (`:85-110`), `job_ended` (`:111-120`) and
//! `get_job_id_for_api`. What PrusaLink also keeps of a job (the file, its path, the start command)
//! is not here, nor its skip of the next number during a power-panic recovery (`job.py:92-93`): a
//! plain Marlin has none (`marlin.rs`, part 9: `recovering` never).
//!
//! The one difference (owner, 2026-11-24, #201 step 3, option A): PrusaLink keeps the number in
//! the MK3's EEPROM (`job.py:147-158` writes it with `D3 Ax0D05`, `printer_polling.py:615-620`
//! reads it); a plain Marlin printer has no such place, so the agent keeps it in the device's own
//! folder, in the file `job_id`. With the file read at once there is no number still to be read
//! from the printer, so PrusaLink's `job_id_offset` (`job.py:94-95`, `:289-299`) never counts; a
//! missing or unreadable file is a number never written, 0.

use maker_connect_sdk::consts::State;
use std::path::{Path, PathBuf};

/// `JobState` (`model_classes.py:103-107`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobState {
    Idle,
    InProgress,
    Ending,
}

/// The job number and its job's state.
#[derive(Debug)]
pub(crate) struct Job {
    /// PrusaLink's number is a Python int; a u64 does not run out in practice.
    job_id: u64,
    state: JobState,
    file: PathBuf,
    /// The file being printed (`JobData.selected_file_path`, `selected_file_size`,
    /// `selected_file_m_timestamp`), set when a print of the agent's storage starts.
    selected: Option<(String, u64, Option<i64>)>,
    /// The command that started the job (`JobData.job_start_cmd_id`).
    start_cmd_id: Option<u32>,
}

impl Job {
    /// The number kept in `dir/job_id`.
    pub(crate) fn load(dir: &Path) -> Job {
        let file = dir.join("job_id");
        let job_id = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        Job {
            job_id,
            state: JobState::Idle,
            file,
            selected: None,
            start_cmd_id: None,
        }
    }

    /// `state_changed` (`job.py:122-133`): before the state goes out. `JOB_STARTING_STATES`
    /// PRINTING, PAUSED; `JOB_ENDING_STATES` FINISHED, STOPPED; `JOB_DESTROYING_STATES` ERROR, IDLE,
    /// BUSY (`const.py:50-59`).
    pub(crate) fn state_changed(&mut self, to: State) {
        if matches!(to, State::Printing | State::Paused) && self.state == JobState::Idle {
            self.job_started();
        }
        if matches!(to, State::Finished | State::Stopped) && self.state == JobState::InProgress {
            self.state = JobState::Ending;
        }
        if matches!(to, State::Error | State::Idle | State::Busy)
            && self.state == JobState::InProgress
        {
            self.job_ended();
        }
    }

    /// `tick` (`job.py:135-138`): after the state went out, a job that was ending has ended.
    pub(crate) fn tick(&mut self) {
        if self.state == JobState::Ending {
            self.job_ended();
        }
    }

    /// `job_ended` (`job.py:111-120`): the start command forgotten, the state IDLE; the number
    /// stays.
    fn job_ended(&mut self) {
        self.start_cmd_id = None;
        self.state = JobState::Idle;
    }

    /// A print of the agent's storage begins (`StartPrint._run_command` →
    /// `_start_file_print`, `job.set_file_path(path, path_incomplete=False)`,
    /// `command_handlers.py:257-303`; the size and time as `selected_file_*`), started by
    /// `command_id` (`job_start_cmd_id`, `job.py:98`).
    pub(crate) fn select(
        &mut self,
        path: &str,
        size: u64,
        m_timestamp: Option<i64>,
        command_id: Option<u32>,
    ) {
        self.selected = Some((path.to_owned(), size, m_timestamp));
        self.start_cmd_id = command_id;
    }

    /// `JobInfo._run_command` (`command_handlers.py:615-644`) with `get_job_info_data`
    /// (`job.py:208-230`): the job's members, or the reason it has none. `from_sd` is false (the
    /// agent prints from its own storage); there is no mesh (`mbl`, a Prusa's) and the path is
    /// never incomplete.
    pub(crate) fn info(&self) -> Result<serde_json::Map<String, serde_json::Value>, &'static str> {
        if self.state == JobState::Idle {
            return Err("Cannot get job info, when there is no job in progress.");
        }
        let Some((path, size, m_timestamp)) = &self.selected else {
            return Err("Cannot get job info, don't know the file details yet.");
        };
        let mut data = serde_json::Map::new();
        if let Some(id) = self.start_cmd_id {
            data.insert("start_cmd_id".into(), id.into());
        }
        data.insert("path".into(), path.clone().into());
        if let Some(t) = m_timestamp {
            data.insert("m_timestamp".into(), (*t).into());
        }
        data.insert("size".into(), (*size).into());
        data.insert("from_sd".into(), false.into());
        data.insert("job_id".into(), self.job_id.into());
        Ok(data)
    }

    /// `job_started` (`job.py:85-110`): the next number, written where it is kept (`write`).
    fn job_started(&mut self) {
        self.job_id += 1;
        self.state = JobState::InProgress;
        // `job_ended` (`:111-120`) only sets the state back; the number stays.
        // Ours: a number that could not be kept is said in the agent's log (PrusaLink's `write`
        // cannot tell, `job.py:147-158`).
        if let Err(e) = std::fs::write(&self.file, self.job_id.to_string()) {
            log::warn!("job id {}: not kept ({e})", self.job_id);
        }
    }

    /// `get_job_id_for_api` (`module_data_classes.py:93-100`): none while no job runs.
    pub(crate) fn for_api(&self) -> Option<u64> {
        (self.state != JobState::Idle).then_some(self.job_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job: its number goes up when it starts and is kept; it is told while it prints and once
    /// more with its end (ENDING until the end went out), then not; a new agent starts from the
    /// kept number. An ERROR, IDLE or BUSY while it prints ends it at once.
    #[test]
    fn the_job_number_as_prusalink_keeps_it() {
        let dir = std::env::temp_dir().join(format!("mc-job-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("job_id"));
        let mut job = Job::load(&dir);
        assert_eq!(job.for_api(), None);
        job.state_changed(State::Printing);
        assert_eq!(job.for_api(), Some(1));
        job.tick();
        job.state_changed(State::Paused);
        assert_eq!(job.for_api(), Some(1), "the same job");
        job.state_changed(State::Finished);
        assert_eq!(job.for_api(), Some(1), "told with its end");
        job.tick();
        assert_eq!(job.for_api(), None);
        let mut again = Job::load(&dir);
        again.state_changed(State::Printing);
        assert_eq!(again.for_api(), Some(2), "kept across a restart");
        assert_eq!(
            again.info(),
            Err("Cannot get job info, don't know the file details yet."),
            "a job whose file is not known"
        );
        again.select("/gcodes/cube.gcode", 1200, Some(1730000000), Some(42));
        let info = again.info().unwrap();
        assert_eq!(
            (
                info["job_id"].clone(),
                info["path"].clone(),
                info["start_cmd_id"].clone()
            ),
            (2.into(), "/gcodes/cube.gcode".into(), 42.into())
        );
        again.state_changed(State::Error);
        assert_eq!(again.for_api(), None);
        assert_eq!(
            again.info(),
            Err("Cannot get job info, when there is no job in progress.")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
