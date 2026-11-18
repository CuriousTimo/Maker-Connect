use super::*;

impl Legacy {
    /// Whether a token belongs to a machine.
    pub fn knows(&self, token: &str) -> rusqlite::Result<bool> {
        self.db.query_row(
            "SELECT COUNT(*) FROM machines WHERE token = ?1",
            [token],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )
    }

    /// A machine's last report, by its token.
    pub fn last_telemetry(&self, token: &str) -> rusqlite::Result<Option<Telemetry>> {
        let text: Option<String> = self
            .db
            .query_row(
                "SELECT telemetry FROM machines WHERE token = ?1",
                [token],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
    }

    /// What a machine tells about itself. Returns whether the token is known.
    pub fn set_details(&self, token: &str, details: &Details) -> rusqlite::Result<bool> {
        let updated = self.db.execute(
            "UPDATE machines SET details = ?1 WHERE token = ?2",
            params![serde_json::to_string(details).unwrap_or_default(), token],
        )?;
        self.changed();
        Ok(updated > 0)
    }

    /// Adds the machine with this pairing code to a space: its name, or None for an
    /// unknown code. A code is used once.
    /// Puts the machine waiting with this code in the space: its (id, name).
    pub fn pair(&self, space: i64, code: &str) -> rusqlite::Result<Option<(String, String)>> {
        let code = code.trim().to_ascii_uppercase();
        let name: Option<(String, String)> = self
            .db
            .query_row(
                "SELECT id, name FROM machines WHERE pairing_code = ?1 AND space_id IS NULL",
                [&code],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if name.is_some() {
            self.db.execute(
                // A Prusa Connect printer still asks with its code for its token.
                "UPDATE machines SET space_id = ?1,
                     pairing_code = CASE link WHEN 'prusa-connect' THEN pairing_code ELSE NULL END
                 WHERE pairing_code = ?2",
                params![space, code],
            )?;
            self.changed();
        }
        Ok(name)
    }

    /// A virtual machine of a profile, straight into the space: its id.
    pub fn add_virtual(
        &mut self,
        space: i64,
        profile: &crate::profiles::Profile,
    ) -> rusqlite::Result<String> {
        let (id, _, _) = self.register_linked(
            &Register {
                protocol: maker_connect_protocol::VERSION,
                name: format!("Virtual {}", profile.model),
                kind: profile.kind,
                agent_version: "simulated in the hub".to_owned(),
            },
            "virtual",
        )?;
        self.db.execute(
            "UPDATE machines SET space_id = ?1, pairing_code = NULL, profile = ?2, profile_chosen = 1
             WHERE id = ?3",
            params![space, profile.id, id],
        )?;
        self.changed();
        Ok(id)
    }

    /// A printer on Klipper, reached through Moonraker (its SimplyPrint component pointed
    /// here), straight into the space: its id.
    pub fn add_moonraker(&mut self, space: i64, name: &str) -> rusqlite::Result<String> {
        let (id, _, _) = self.register_linked(
            &Register {
                protocol: maker_connect_protocol::VERSION,
                name: name.to_owned(),
                kind: MachineKind::Fdm,
                agent_version: "Moonraker".to_owned(),
            },
            "simplyprint",
        )?;
        self.db.execute(
            "UPDATE machines SET space_id = ?1, pairing_code = NULL WHERE id = ?2",
            params![space, id],
        )?;
        self.changed();
        Ok(id)
    }

    /// The name of the Moonraker printer with this token, if it is one in a space.
    pub fn moonraker_name(&self, token: &str) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row(
                "SELECT name FROM machines WHERE token = ?1 AND link = 'simplyprint' AND space_id IS NOT NULL",
                [token],
                |r| r.get(0),
            )
            .optional()
    }

    /// A Moonraker printer's token, for the address its config file names.
    pub fn moonraker_token(&self, space: i64, machine: &str) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row(
                "SELECT token FROM machines WHERE id = ?1 AND space_id = ?2 AND link = 'simplyprint'",
                params![machine, space],
                |r| r.get(0),
            )
            .optional()
    }

    /// Every virtual machine: its id, token and profile.
    pub fn virtual_machines(&self) -> rusqlite::Result<Vec<(String, String, Option<String>)>> {
        let mut query = self
            .db
            .prepare("SELECT id, token, profile FROM machines WHERE link = 'virtual'")?;
        let rows = query.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// The owner removes a machine from the space: gone, with its token.
    /// Moves a machine to another space: what someone may do with it there is that space's
    /// business. Refused (with the reason) while it has work under way. What goes along is
    /// the machine itself (name, labels, plate, service interval) and its agent, which keeps
    /// its token. What stays: its jobs (their files are the old space's), the rights given
    /// on it (people of the old space), the spool said to be loaded (the old space's
    /// Materials), and its place in the old space's queue.
    pub fn move_machine(
        &mut self,
        from: i64,
        machine: &str,
        to: i64,
    ) -> rusqlite::Result<Result<(), String>> {
        let here: bool = self.db.query_row(
            "SELECT COUNT(*) FROM machines WHERE id = ?1 AND space_id = ?2",
            params![machine, from],
            |r| r.get(0),
        )?;
        if !here {
            return Ok(Err("no such machine".to_owned()));
        }
        if from == to {
            return Ok(Err("it is in that space already".to_owned()));
        }
        if self.active_job(machine)?.is_some() {
            return Ok(Err(
                "it has a job that is not over: stop it, or let it finish".to_owned(),
            ));
        }
        if self.waiting_confirm(machine)?.is_some() {
            return Ok(Err(
                "a plate on it waits to be confirmed: Set ready or Bad print first".to_owned(),
            ));
        }
        if self.on_bed(machine)?.is_some() {
            return Ok(Err(
                "a print is still on its bed: say it was taken first".to_owned()
            ));
        }
        let sending: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM printer_transfers WHERE machine_id = ?1 AND state IN ('waiting', 'sent')",
            [machine],
            |r| r.get(0),
        )?;
        if sending > 0 {
            return Ok(Err(
                "a file is being sent to it: wait until it is there".to_owned()
            ));
        }
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE machines SET space_id = ?2, farm = 0 WHERE id = ?1",
            params![machine, to],
        )?;
        tx.execute(
            "DELETE FROM machine_grants WHERE machine_id = ?1",
            [machine],
        )?;
        tx.execute(
            "DELETE FROM machine_group_members WHERE machine_id = ?1",
            [machine],
        )?;
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
        Ok(Ok(()))
    }

    pub fn remove(&mut self, space: i64, machine: &str) -> rusqlite::Result<bool> {
        let removed = self.db.execute(
            "DELETE FROM machines WHERE id = ?1 AND space_id = ?2",
            params![machine, space],
        )?;
        if removed > 0 {
            self.db.execute(
                "DELETE FROM machine_grants WHERE machine_id = ?1",
                [machine],
            )?;
            self.db.execute(
                "DELETE FROM machine_group_members WHERE machine_id = ?1",
                [machine],
            )?;
        }
        self.commands.remove(machine);
        self.history.remove(machine);
        self.changed();
        Ok(removed > 0)
    }

    /// The owner says what is loaded (a label printer's tape), or None. Returns whether
    /// the machine is in the space.
    pub fn set_media(
        &self,
        space: i64,
        machine: &str,
        media: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let updated = self.db.execute(
            "UPDATE machines SET media = ?1 WHERE id = ?2 AND space_id = ?3",
            params![media, machine, space],
        )?;
        self.changed();
        Ok(updated > 0)
    }

    /// The width of the tape a label printer has loaded, as its owner said.
    /// The tape or labels loaded in a label printer, as the owner said: (width across, a
    /// die-cut label's length) in mm.
    pub fn loaded_media_mm(&self, machine: &str) -> rusqlite::Result<Option<(u32, Option<u32>)>> {
        let found: Option<(Option<String>, Option<String>)> = self
            .db
            .query_row(
                "SELECT profile, media FROM machines WHERE id = ?1",
                [machine],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(found.and_then(|(profile, media)| {
            let profile = crate::profiles::find(&profile?)?;
            let media = crate::profiles::media(profile, &media?)?;
            Some((media.width_mm, media.length_mm))
        }))
    }

    /// HS2b (W2b): a LabelWriter's loaded roll as the PPD's `PageSize` choice (its medium id when the
    /// profile takes its media from the port), for the job to name its page.
    pub fn loaded_page_size(&self, machine: &str) -> rusqlite::Result<Option<String>> {
        let found: Option<(Option<String>, Option<String>)> = self
            .db
            .query_row(
                "SELECT profile, media FROM machines WHERE id = ?1",
                [machine],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(found.and_then(|(profile, media)| {
            let profile = crate::profiles::find(&profile?)?;
            let media = media?;
            crate::profiles::labelwriter_page(Some(profile), &media).map(|g| g.name.to_owned())
        }))
    }

    /// Hangs a device behind a channel of another of the space (None: on no plug). Refused
    /// when the other is not in the space, or is behind this one already (a loop).
    pub fn set_powered_by(
        &self,
        space: i64,
        machine: &str,
        power: Option<&PowerLink>,
    ) -> rusqlite::Result<Result<(), &'static str>> {
        let parent = |id: &str| -> rusqlite::Result<Option<Option<String>>> {
            self.db
                .query_row(
                    "SELECT powered_by FROM machines WHERE id = ?1 AND space_id = ?2",
                    params![id, space],
                    |r| r.get(0),
                )
                .optional()
        };
        if parent(machine)?.is_none() {
            return Ok(Err("no such device"));
        }
        if let Some(power) = power {
            // Up from the new parent: reaching this device would close a loop.
            let mut at = Some(power.device.clone());
            let mut steps = 0;
            while let Some(id) = at {
                if id == machine {
                    return Ok(Err("that device gets its power through this one"));
                }
                match parent(&id)? {
                    Some(next) => at = next,
                    None if id == power.device => {
                        return Ok(Err("no such device to hang it behind"));
                    }
                    None => break,
                }
                steps += 1;
                if steps > 64 {
                    return Ok(Err("the power tree is too deep"));
                }
            }
        }
        self.db.execute(
            "UPDATE machines SET powered_by = ?1, power_channel = ?2 WHERE id = ?3 AND space_id = ?4",
            params![power.map(|p| &p.device), power.map(|p| p.channel), machine, space],
        )?;
        self.changed();
        Ok(Ok(()))
    }

    /// The owner renames a machine. Returns whether the machine is in the space.
    pub fn rename(&self, space: i64, machine: &str, name: &str) -> rusqlite::Result<bool> {
        let updated = self.db.execute(
            "UPDATE machines SET name = ?1 WHERE id = ?2 AND space_id = ?3",
            params![name, machine, space],
        )?;
        self.changed();
        Ok(updated > 0)
    }

    /// The owner chooses a machine's profile (None: back to recognising it). Returns
    /// whether the machine is in the space.
    pub fn set_profile(
        &self,
        space: i64,
        machine: &str,
        profile: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let updated = self.db.execute(
            "UPDATE machines SET profile = ?1, profile_chosen = ?2 WHERE id = ?3 AND space_id = ?4",
            params![profile, profile.is_some(), machine, space],
        )?;
        self.changed();
        Ok(updated > 0)
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

    /// A machine's labels: trimmed, without doubles, at most 12 of at most 24 characters.
    pub fn set_labels(
        &mut self,
        space: i64,
        machine: &str,
        labels: &[String],
    ) -> rusqlite::Result<bool> {
        let mut clean: Vec<String> = Vec::new();
        for label in labels {
            let label: String = label.trim().chars().take(24).collect();
            if !label.is_empty()
                && !clean.iter().any(|l| l.eq_ignore_ascii_case(&label))
                && clean.len() < 12
            {
                clean.push(label);
            }
        }
        let n = self.db.execute(
            "UPDATE machines SET labels = ?3 WHERE id = ?1 AND space_id = ?2",
            params![
                machine,
                space,
                serde_json::to_string(&clean).unwrap_or_default()
            ],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// One machine of a space.
    pub fn machine(&self, space: i64, id: &str) -> rusqlite::Result<Option<Machine>> {
        Ok(self.machines(space)?.into_iter().find(|m| m.id == id))
    }

    /// A space's machines, by name.
    pub fn machines(&self, space: i64) -> rusqlite::Result<Vec<Machine>> {
        let mut query = self.db.prepare(
            "SELECT id, name, kind, agent_version, last_seen, telemetry, profile, profile_chosen, link, details, mesh,
                    media, powered_by, power_channel, reached_through, agent, peer, out_of_use, out_of_use_at,
                    laser_w, labels, induction_required, ppe, agent_update_to
             FROM machines WHERE space_id = ?1 ORDER BY name",
        )?;
        let now = now();
        let rows = query.query_map([space], |r| {
            let kind: String = r.get(2)?;
            let last_seen: Option<i64> = r.get(4)?;
            let telemetry: Option<String> = r.get(5)?;
            let seen = last_seen.map(|t| now - t);
            let online = seen.is_some_and(|s| s <= ONLINE_SECONDS);
            Ok(Machine {
                laser_framed: false,
                laser_origin_set: false,
                id: r.get(0)?,
                name: r.get(1)?,
                kind: serde_json::from_str(&kind).unwrap_or(MachineKind::Other),
                agent_version: r.get(3)?,
                seen_seconds_ago: seen,
                online,
                telemetry: telemetry
                    .filter(|_| online)
                    .and_then(|t| serde_json::from_str(&t).ok()),
                profile: r.get(6)?,
                profile_chosen: r.get(7)?,
                link: r.get(8)?,
                details: r
                    .get::<_, Option<String>>(9)?
                    .and_then(|d| serde_json::from_str(&d).ok()),
                commands: Vec::new(),
                job: None,
                resumable: None,
                mesh: r
                    .get::<_, Option<String>>(10)?
                    .and_then(|m| serde_json::from_str(&m).ok()),
                media: r.get(11)?,
                powered_by: r.get::<_, Option<String>>(12)?.map(|device| PowerLink {
                    device,
                    channel: r.get::<_, Option<u32>>(13).ok().flatten().unwrap_or(0),
                }),
                reached_through: r.get(14)?,
                agent: r
                    .get::<_, Option<String>>(15)?
                    .and_then(|a| serde_json::from_str(&a).ok()),
                peer: r.get(16)?,
                agent_update: None,
                agent_update_to: r.get(23)?,
                out_of_use: r.get::<_, Option<String>>(17)?.map(|reason| OutOfUse {
                    reason,
                    since: r.get::<_, Option<i64>>(18).ok().flatten().unwrap_or(0),
                }),
                loaded: None,
                on_bed: None,
                ready: None,
                queue: Vec::new(),
                queue_held: None,
                asking: None,
                buttons: Vec::new(),
                usb_print_by: None,
                moonraker_token: None,
                laser_w: r.get(19)?,
                labels: r
                    .get::<_, Option<String>>(20)?
                    .and_then(|l| serde_json::from_str(&l).ok())
                    .unwrap_or_default(),
                induction_required: r.get::<_, i64>(21)? != 0,
                training: r.get::<_, i64>(21)? == 2,
                ppe: crate::safety::ppe_of(
                    &r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(22)?.as_deref(),
                ),
                confirm: None,
                farm: false,
                upkeep: None,
                sheet: None,
            })
        })?;
        let mut machines: Vec<Machine> = rows.collect::<rusqlite::Result<_>>()?;
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
            if machine.link == "simplyprint" {
                machine.moonraker_token = self.moonraker_token(space, &machine.id)?;
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
