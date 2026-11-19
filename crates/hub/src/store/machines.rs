use super::*;

/// What a space's modules say of its machines, by machine id, for [`Legacy::complete_machines`]:
/// what is loaded (crate::stock), their queues (crate::queue), the plates waiting to be confirmed
/// (crate::farm).
pub(crate) struct Completion {
    pub loaded: std::collections::HashMap<String, Loaded>,
    pub queues: std::collections::HashMap<String, Vec<crate::queue::Queued>>,
    pub confirms: std::collections::HashMap<String, crate::farm::Confirm>,
}

impl Legacy {
    /// What holds a machine in its space ([`Store::move_machine`]): work under way, a plate to
    /// confirm, a print on its bed, a file on its way.
    ///
    /// `sending`: how many files are on their way to it ([`Store::files_on_their_way`]).
    pub(crate) fn why_it_stays(
        &self,
        jobs: &JobFacts,
        machine: &str,
        sending: i64,
    ) -> Option<String> {
        if jobs.active_job(machine).is_some() {
            return Some("it has a job that is not over: stop it, or let it finish".to_owned());
        }
        if jobs.waiting_plate(machine).is_some() {
            return Some(
                "a plate on it waits to be confirmed: Set ready or Bad print first".to_owned(),
            );
        }
        if jobs.on_bed(machine).is_some() {
            return Some("a print is still on its bed: say it was taken first".to_owned());
        }
        if sending > 0 {
            return Some("a file is being sent to it: wait until it is there".to_owned());
        }
        None
    }

    /// A machine moved to another space ([`Store::move_machine`]): what stays behind in the old.
    pub(crate) fn moved_away(&mut self, machine: &str) {
        // Commands asked in the old space are not the new one's.
        self.commands.remove(machine);
        self.changed();
    }

    /// A machine removed ([`Store::remove_machine`]): what the hub keeps of it in memory goes.
    pub(crate) fn forget_machine(&mut self, machine: &str) {
        self.commands.remove(machine);
        self.history.remove(machine);
        self.changed();
    }

    /// The agent build the Downloads page offers: its commit and when it was built, from
    /// `.build` beside the downloads (written by `scripts/build-downloads.sh`).
    pub fn offered_build(&self) -> Option<OfferedBuild> {
        let text = std::fs::read_to_string(self.downloads_dir.join(".build")).ok()?;
        let mut lines = text.lines().map(|l| l.trim().to_owned());
        let commit = lines.next().filter(|c| !c.is_empty())?;
        Some(OfferedBuild {
            commit,
            built: lines.next().unwrap_or_default(),
            version: lines.next().filter(|v| !v.is_empty()),
        })
    }

    /// Machines from their rows, with what the hub's memory adds; `space`: what the space's
    /// modules say of each, by machine id, read before ([`Completion`]).
    pub(crate) fn complete_machines(
        &self,
        rows: Vec<super::pg_machines::MachineRow>,
        jobs: &JobFacts,
        space: Completion,
    ) -> Vec<Machine> {
        let Completion {
            loaded,
            mut queues,
            mut confirms,
        } = space;
        let now = now();
        let mut machines: Vec<Machine> = rows.into_iter().map(|r| r.into_machine(now)).collect();
        let offered = self.offered_build();
        for machine in &mut machines {
            machine.loaded = loaded.get(&machine.id).cloned();
            if machine.kind == MachineKind::Fdm {
                machine.on_bed = jobs.on_bed(&machine.id);
                machine.ready = jobs.ready_after(&machine.id);
                machine.queue = queues.remove(&machine.id).unwrap_or_default();
                machine.asking = self.asking.get(&machine.id).cloned();
                // Only with a dialog the printer named: DIALOG_ACTION needs its `dialog_id`.
                machine.buttons = self
                    .dialogs
                    .get(&machine.id)
                    .filter(|(_, dialog)| dialog.is_some())
                    .map(|(buttons, _)| buttons.clone())
                    .unwrap_or_default();
                // An agent's printer asks through its report.
                if let Some(prompt) = machine
                    .telemetry
                    .as_ref()
                    .and_then(|t| t.prompt.as_ref())
                    .filter(|_| machine.online)
                {
                    machine.asking = Some((prompt.text.clone(), prompt.text.clone()));
                    machine.buttons = prompt.buttons.clone();
                }
                machine.queue_held = if machine.job.is_some() {
                    None
                } else {
                    jobs.bed_unchecked(&machine.id)
                };
                // A plate for a part waits for someone to confirm it (crate::farm): that, not
                // "take the print".
                machine.confirm = confirms.remove(&machine.id);
                if machine.confirm.is_some() {
                    machine.on_bed = None;
                }
                machine.upkeep = jobs.upkeep(&machine.id);
            } else {
                // Shown for a 3D printer only (its row keeps them whatever its kind).
                machine.usb_print_by = None;
                machine.farm = false;
                machine.sheet = None;
            }
            // An agent from before builds were named is behind any build on offer.
            if let (Some(offered), Some(agent)) = (&offered, machine.agent.as_ref())
                && agent.build.as_ref() != Some(&offered.commit)
            {
                machine.agent_update = Some(format!("{} ({})", offered.commit, offered.built));
            }
            machine.job = jobs.active_job(&machine.id);
            machine.laser_framed = machine.job.as_ref().is_some_and(|j| {
                j.state == JobState::Loaded
                    && self.laser_framed.get(&machine.id) == Some(&j.file_id)
            });
            machine.laser_origin_set = self.laser_origin_set.contains(&machine.id);
            if machine.job.is_none() {
                machine.resumable = jobs.resumable_job(&machine.id);
            }
            if let Some(commands) = self.commands.get(&machine.id) {
                machine.commands = commands
                    .iter()
                    .rev()
                    .filter(|c| !c.background)
                    .cloned()
                    .collect();
            }
        }
        machines
    }
}
