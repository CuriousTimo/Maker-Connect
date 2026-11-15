use super::*;

impl Store {
    /// The owner gives a machine a command: its id, or None when the machine is not in
    /// the space or cannot take commands (yet: only printers on built-in Prusa Connect).
    pub fn queue_command(
        &mut self,
        space: i64,
        machine: &str,
        label: &str,
        gcode: &str,
    ) -> rusqlite::Result<Option<u32>> {
        if self
            .machine(space, machine)?
            .is_none_or(|m| !takes_commands(&m.link))
        {
            return Ok(None);
        }
        self.next_command = self.next_command.wrapping_add(1);
        let id = self.next_command;
        let commands = self.commands.entry(machine.to_owned()).or_default();
        commands.push(Command {
            id,
            label: label.to_owned(),
            gcode: gcode.to_owned(),
            json: false,
            action: false,
            background: false,
            output: Vec::new(),
            state: CommandState::Queued,
            reason: None,
            at: now(),
        });
        trim(commands);
        self.changed();
        Ok(Some(id))
    }

    /// A typed command for a machine behind an agent: its id, or None when the machine is not
    /// in the space or takes no commands.
    pub fn queue_action(
        &mut self,
        space: i64,
        machine: &str,
        label: &str,
        action: &maker_connect_protocol::Action,
    ) -> rusqlite::Result<Option<u32>> {
        let body = serde_json::to_string(action).expect("an Action is JSON");
        let Some(id) = self.queue_command(space, machine, label, &body)? else {
            return Ok(None);
        };
        if let Some(command) = self.commands.get_mut(machine).and_then(|c| c.iter_mut().find(|c| c.id == id)) {
            command.action = true;
        }
        Ok(Some(id))
    }

    /// The next command for the machine asking with this token, once the one before is
    /// done: one at a time, as a printer takes them.
    pub fn next_command(&mut self, token: &str) -> rusqlite::Result<Option<(u32, String)>> {
        Ok(self
            .next_command_of_any_kind(token)?
            .map(|(id, body, _)| (id, body)))
    }

    /// A command of Prusa Connect's for a printer on its built-in Connect (its id): queued
    /// like G-code, sent as JSON (`SEND_FILE_INFO`, `START_PRINT`, `START_ENCRYPTED_DOWNLOAD`).
    pub fn queue_connect_command(
        &mut self,
        space: i64,
        machine: &str,
        label: &str,
        command: &serde_json::Value,
        background: bool,
    ) -> rusqlite::Result<Option<u32>> {
        if self
            .machine(space, machine)?
            .is_none_or(|m| m.link != "prusa-connect")
        {
            return Ok(None);
        }
        let body = command.to_string();
        // The hub's own reading (a folder, a file's picture) once: asked again while the
        // same is still waiting or on its way, it is not queued twice.
        if background
            && let Some(same) = self.commands.get(machine).and_then(|commands| {
                commands.iter().find(|c| {
                    c.gcode == body
                        && matches!(
                            c.state,
                            CommandState::Queued | CommandState::Sent | CommandState::Accepted
                        )
                })
            })
        {
            return Ok(Some(same.id));
        }
        self.next_command = self.next_command.wrapping_add(1);
        let id = self.next_command;
        let commands = self.commands.entry(machine.to_owned()).or_default();
        commands.push(Command {
            id,
            label: label.to_owned(),
            gcode: body,
            json: true,
            action: false,
            background,
            output: Vec::new(),
            state: CommandState::Queued,
            reason: None,
            at: now(),
        });
        trim(commands);
        self.changed();
        Ok(Some(id))
    }

    /// The commands of a machine still open (waiting, sent or taken on): what they send.
    pub fn open_commands(&self, machine: &str) -> Vec<String> {
        self.commands
            .get(machine)
            .into_iter()
            .flatten()
            .filter(|c| {
                matches!(
                    c.state,
                    CommandState::Queued | CommandState::Sent | CommandState::Accepted
                )
            })
            .map(|c| c.gcode.clone())
            .collect()
    }

    /// A command given to the machine with this token, by its id: what it sent (G-code or
    /// Connect's JSON).
    pub fn command_text(&self, token: &str, id: u32) -> rusqlite::Result<Option<String>> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(None);
        };
        Ok(self
            .commands
            .get(&machine)
            .and_then(|commands| commands.iter().find(|c| c.id == id))
            .map(|c| c.gcode.clone()))
    }

    /// The paths on a printer's stick with a Connect command for them still open (waiting,
    /// sent or taken on), with the command's name (`DELETE_FILE`, ...).
    pub fn open_path_commands(&self, machine: &str) -> Vec<(String, String)> {
        self.commands
            .get(machine)
            .into_iter()
            .flatten()
            .filter(|c| {
                c.json
                    && matches!(
                        c.state,
                        CommandState::Queued | CommandState::Sent | CommandState::Accepted
                    )
            })
            .filter_map(|c| {
                let body: serde_json::Value = serde_json::from_str(&c.gcode).ok()?;
                Some((
                    body["kwargs"]["path"].as_str()?.to_owned(),
                    body["command"].as_str()?.to_owned(),
                ))
            })
            .collect()
    }

    /// Whether the machine behind this token is an agent machine whose port is the Marlin/OctoPrint
    /// port: an agent link, a plain Marlin profile (`plain_marlin`, as K5, and Marlin or no firmware named) and an FDM printer (the
    /// DYMO, LabelWriter and laser ports are agent machines too, and keep the gate). K18.
    pub fn is_marlin_agent(&self, token: &str) -> rusqlite::Result<bool> {
        let Some(space) = self
            .db
            .query_row(
                "SELECT space_id FROM machines WHERE token = ?1 AND space_id IS NOT NULL",
                [token],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        else {
            return Ok(false);
        };
        let Some(id) = self.machine_id(token)? else {
            return Ok(false);
        };
        Ok(self.machine(space, &id)?.is_some_and(|m| {
            // `plain_marlin` alone would take a Buddy or Klipper profile too (it leaves out only the
            // MK3 family and GRBL): the Marlin port's machines are the ones whose profile says Marlin
            // or says nothing
            m.kind == maker_connect_protocol::MachineKind::Fdm
                && crate::machine_page::plain_marlin(&m)
                && m.profile
                    .as_deref()
                    .and_then(crate::profiles::find)
                    .and_then(|p| p.firmware.as_deref())
                    .is_none_or(|f| f == "marlin")
        }))
    }

    /// A test says when the machine's agent last reported.
    #[cfg(test)]
    pub fn set_last_seen(&self, machine: &str, at: i64) {
        self.db
            .execute("UPDATE machines SET last_seen = ?1 WHERE id = ?2", params![at, machine])
            .unwrap();
    }

    /// A test says when the machine's commands last changed.
    #[cfg(test)]
    pub fn set_commands_at(&mut self, machine: &str, at: i64) {
        for c in self.commands.get_mut(machine).unwrap() {
            c.at = at;
        }
    }

    /// K18, the agent-gone rule (ours): a Marlin agent machine not seen for longer than
    /// `ONLINE_SECONDS` has its Sent and Accepted commands (not the Queued ones) failed with "the
    /// agent was gone", checked on its next report, before that report is taken in.
    pub fn fail_commands_of_gone_agent(&mut self, token: &str) -> rusqlite::Result<()> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(());
        };
        let seen: Option<i64> = self
            .db
            .query_row("SELECT last_seen FROM machines WHERE token = ?1", [token], |r| r.get(0))
            .optional()?
            .flatten();
        if seen.is_none_or(|t| now() - t <= ONLINE_SECONDS) || !self.is_marlin_agent(token)? {
            return Ok(());
        }
        let mut any = false;
        for c in self
            .commands
            .entry(machine)
            .or_default()
            .iter_mut()
            .filter(|c| matches!(c.state, CommandState::Sent | CommandState::Accepted))
        {
            c.state = CommandState::Rejected;
            c.reason = Some(AGENT_GONE.to_owned());
            any = true;
        }
        if any {
            self.changed();
        }
        Ok(())
    }

    /// All Queued commands of a Marlin agent machine, in the existing order (someone's before the
    /// hub's own background ones), each now Sent: no gate and no 120 s (K18).
    pub fn next_commands_for_agent(&mut self, token: &str) -> rusqlite::Result<Vec<(u32, String)>> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(Vec::new());
        };
        let Some(commands) = self.commands.get_mut(&machine) else {
            return Ok(Vec::new());
        };
        let now = now();
        let mut out = Vec::new();
        for background in [false, true] {
            for c in commands
                .iter_mut()
                .filter(|c| c.state == CommandState::Queued && c.background == background)
            {
                c.state = CommandState::Sent;
                c.at = now;
                out.push((c.id, c.gcode.clone()));
            }
        }
        if !out.is_empty() {
            self.changed();
        }
        Ok(out)
    }

    /// The next command, with whether it is a Connect (JSON) command.
    pub fn next_command_of_any_kind(
        &mut self,
        token: &str,
    ) -> rusqlite::Result<Option<(u32, String, bool)>> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(None);
        };
        let Some(commands) = self.commands.get_mut(&machine) else {
            return Ok(None);
        };
        let now = now();
        for busy in commands
            .iter_mut()
            .filter(|c| matches!(c.state, CommandState::Sent | CommandState::Accepted))
        {
            if now - busy.at < COMMAND_SECONDS {
                return Ok(None);
            }
            busy.state = CommandState::Rejected;
            busy.reason = Some("no answer from the machine".to_owned());
        }
        // Someone's commands before the hub's own (reading a USB stick in the background).
        let at = commands
            .iter()
            .position(|c| c.state == CommandState::Queued && !c.background)
            .or_else(|| {
                commands
                    .iter()
                    .position(|c| c.state == CommandState::Queued)
            });
        let next = at.map(|i| &mut commands[i]).map(|c| {
            c.state = CommandState::Sent;
            c.at = now;
            (c.id, c.gcode.clone(), c.json)
        });
        if next.is_some() {
            self.changed();
        }
        Ok(next)
    }

    /// The printer says in its telemetry it is still carrying out this command (Buddy sends
    /// the `command_id` of G-code running in the background): not "no answer" while it does.
    pub fn command_still_running(&mut self, token: &str, id: u32) -> rusqlite::Result<()> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(());
        };
        if let Some(command) = self.commands.get_mut(&machine).and_then(|commands| {
            commands.iter_mut().find(|c| {
                c.id == id && matches!(c.state, CommandState::Sent | CommandState::Accepted)
            })
        }) {
            command.state = CommandState::Accepted;
            command.at = now();
        }
        Ok(())
    }

    /// The machine tells how a command went.
    pub fn command_went(
        &mut self,
        token: &str,
        id: u32,
        state: CommandState,
        reason: Option<String>,
        output: Vec<String>,
    ) -> rusqlite::Result<()> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(());
        };
        if let Some(command) = self
            .commands
            .get_mut(&machine)
            .and_then(|commands| commands.iter_mut().find(|c| c.id == id))
        {
            command.state = state;
            command.reason = reason;
            command.output.extend(output);
            let excess = command.output.len().saturating_sub(OUTPUT_KEPT);
            command.output.drain(..excess);
            command.at = now();
            self.changed();
        }
        Ok(())
    }
}
