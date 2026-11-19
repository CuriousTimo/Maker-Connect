use super::*;

impl Legacy {
    /// What holds a machine in its space ([`Store::move_machine`]): work under way, a plate to
    /// confirm, a print on its bed, a file on its way.
    pub(crate) fn why_it_stays(&self, machine: &str) -> rusqlite::Result<Option<String>> {
        if self.active_job(machine)?.is_some() {
            return Ok(Some(
                "it has a job that is not over: stop it, or let it finish".to_owned(),
            ));
        }
        if self.waiting_confirm(machine)?.is_some() {
            return Ok(Some(
                "a plate on it waits to be confirmed: Set ready or Bad print first".to_owned(),
            ));
        }
        if self.on_bed(machine)?.is_some() {
            return Ok(Some(
                "a print is still on its bed: say it was taken first".to_owned(),
            ));
        }
        let sending: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM printer_transfers WHERE machine_id = ?1 AND state IN ('waiting', 'sent')",
            [machine],
            |r| r.get(0),
        )?;
        if sending > 0 {
            return Ok(Some(
                "a file is being sent to it: wait until it is there".to_owned(),
            ));
        }
        Ok(None)
    }

    /// A machine moved to another space ([`Store::move_machine`]): what stays behind in the old.
    pub(crate) fn moved_away(&mut self, from: i64, machine: &str) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("UPDATE machines SET farm = 0 WHERE id = ?1", [machine])?;
        tx.execute(
            "UPDATE spool SET machine = NULL WHERE machine = ?1 AND space_id = ?2",
            params![machine, from],
        )?;
        tx.execute(
            "UPDATE printer_transfers SET state = 'failed', reason = 'the machine moved to another space'
             WHERE machine_id = ?1 AND state NOT IN ('finished', 'done', 'failed')",
            [machine],
        )?;
        tx.commit()?;
        // Commands asked in the old space are not the new one's.
        self.commands.remove(machine);
        self.changed();
        Ok(())
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

    /// One machine of a space, from the SQLite copy (transitional, lane 3a: for the modules still on
    /// SQLite; the website reads [`Store::machine`]).
    pub(crate) fn machine_on_mirror(
        &self,
        space: i64,
        id: &str,
    ) -> rusqlite::Result<Option<Machine>> {
        Ok(self
            .machines_on_mirror(space)?
            .into_iter()
            .find(|m| m.id == id))
    }

    /// A space's machines, by name, from the SQLite copy (transitional, lane 3a: for the modules
    /// still on SQLite; the website reads [`Store::machines`]).
    pub(crate) fn machines_on_mirror(&self, space: i64) -> rusqlite::Result<Vec<Machine>> {
        let mut query = self.db.prepare(
            "SELECT id, name, kind, agent_version, token, last_seen, telemetry, profile, profile_chosen, link,
                    details, mesh, media, powered_by, power_channel, reached_through, agent, peer, out_of_use,
                    out_of_use_at, laser_w, labels
             FROM machines WHERE space_id = ?1 ORDER BY name",
        )?;
        let rows = query
            .query_map([space], |r| {
                Ok(super::pg_mirror::MachineRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    agent_version: r.get(3)?,
                    token: r.get(4)?,
                    last_seen: r.get(5)?,
                    telemetry: r.get(6)?,
                    profile: r.get(7)?,
                    profile_chosen: r.get(8)?,
                    link: r.get(9)?,
                    details: r.get(10)?,
                    mesh: r.get(11)?,
                    media: r.get(12)?,
                    powered_by: r.get(13)?,
                    power_channel: r.get(14)?,
                    reached_through: r.get(15)?,
                    agent: r.get(16)?,
                    peer: r.get(17)?,
                    out_of_use: r.get(18)?,
                    out_of_use_at: r.get(19)?,
                    laser_w: r.get(20)?,
                    labels: r.get(21)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.complete_machines(space, rows)
    }

    /// Machines from their rows, with what the modules on SQLite and the hub's memory add.
    pub(crate) fn complete_machines(
        &self,
        space: i64,
        rows: Vec<super::pg_mirror::MachineRow>,
    ) -> rusqlite::Result<Vec<Machine>> {
        let now = now();
        let mut machines: Vec<Machine> = rows.into_iter().map(|r| r.into_machine(now)).collect();
        // What the modules on SQLite keep in the machine's row (pg_mirror.rs).
        for machine in &mut machines {
            let owned: Option<(String, i64, Option<String>, Option<String>)> = self
                .db
                .query_row(
                    "SELECT kind, induction_required, ppe, agent_update_to FROM machines WHERE id = ?1",
                    [&machine.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let (kind, induction, ppe, update_to) = owned.unwrap_or_else(|| {
                (
                    serde_json::to_string(&machine.kind).unwrap_or_default(),
                    0,
                    None,
                    None,
                )
            });
            machine.induction_required = induction != 0;
            machine.training = induction == 2;
            machine.ppe = crate::safety::ppe_of(&kind, ppe.as_deref());
            machine.agent_update_to = update_to;
        }
        let offered = self.offered_build();
        let items = self.items(space)?;
        let materials = self.materials(space)?;
        for machine in &mut machines {
            machine.loaded = items
                .iter()
                .find(|i| i.machine.as_deref() == Some(&machine.id) && i.state == "loaded")
                .map(|item| {
                    let material = materials.iter().find(|m| m.id == item.material);
                    Loaded {
                        item: item.id.clone(),
                        name: material
                            .map(|m| m.color_name.clone().unwrap_or_else(|| m.name.clone()))
                            .unwrap_or_else(|| item.material.clone()),
                        color: material.and_then(|m| m.color.clone()),
                        nozzle_c: material.and_then(|m| m.nozzle_c),
                        bed_c: material.and_then(|m| m.bed_c),
                        material: item.material.parse::<i64>().ok().and_then(|id| {
                            self.db
                                .query_row(
                                    "SELECT material FROM filament WHERE id = ?1 AND kind = 'filament'",
                                    [id],
                                    |r| r.get::<_, Option<String>>(0),
                                )
                                .ok()
                                .flatten()
                        }),
                        place: item.location.as_ref().and_then(|code| {
                            self.db
                                .query_row(
                                    "SELECT name FROM locations WHERE id = ?1 AND space_id = ?2",
                                    params![code, space],
                                    |r| r.get(0),
                                )
                                .ok()
                        }),
                        slot: crate::stock::spool_number(&item.id).and_then(|id| {
                            self.db
                                .query_row("SELECT slot FROM spool WHERE id = ?1", [id], |r| {
                                    r.get::<_, Option<i64>>(0)
                                })
                                .ok()
                                .flatten()
                        }),
                        amount: item.amount,
                        unit: material.map_or("pcs".to_owned(), |m| m.unit.clone()),
                    }
                });
            if machine.kind == MachineKind::Fdm {
                machine.on_bed = self.on_bed(&machine.id)?;
                machine.ready = self.ready_after(&machine.id)?;
                machine.queue = self.queue_of(&machine.id)?;
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
                    self.bed_unchecked(&machine.id)?
                };
                machine.usb_print_by = self
                    .db
                    .query_row(
                        "SELECT usb_print_by FROM machines WHERE id = ?1",
                        [&machine.id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                // A plate for a part waits for someone to confirm it (crate::farm): that, not
                // "take the print".
                machine.confirm = self.waiting_confirm(&machine.id)?;
                if machine.confirm.is_some() {
                    machine.on_bed = None;
                }
                machine.farm = self.takes_work(&machine.id)?;
                machine.upkeep = self.upkeep(&machine.id)?;
                machine.sheet = self.sheet_of(&machine.id)?;
            }
            // An agent from before builds were named is behind any build on offer.
            if let (Some(offered), Some(agent)) = (&offered, machine.agent.as_ref())
                && agent.build.as_ref() != Some(&offered.commit)
            {
                machine.agent_update = Some(format!("{} ({})", offered.commit, offered.built));
            }
            machine.job = self.active_job(&machine.id)?;
            machine.laser_framed = machine.job.as_ref().is_some_and(|j| {
                j.state == JobState::Loaded
                    && self.laser_framed.get(&machine.id) == Some(&j.file_id)
            });
            machine.laser_origin_set = self.laser_origin_set.contains(&machine.id);
            if machine.job.is_none() {
                machine.resumable = self.resumable_job(&machine.id)?;
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
        Ok(machines)
    }
}
