use super::*;

/// The machine asking with a token, as its commands need it (read from Postgres before the lock).
#[derive(Debug, Clone)]
pub struct Caller {
    pub id: String,
    pub link: String,
    pub seen_seconds_ago: Option<i64>,
}

impl Store {
    /// The machine behind this token, if the token is known (a `Caller`).
    pub async fn caller(&self, token: &str) -> sqlx::Result<Option<Caller>> {
        Ok(self.machine_by_token(token).await?.map(|(m, _)| Caller {
            seen_seconds_ago: m.seen_seconds_ago,
            id: m.id,
            link: m.link,
        }))
    }

    /// How a machine of the space is reached (its `link`), or None when it is not in the space:
    /// what queueing a command for it needs.
    pub async fn link_of(&self, space: i64, machine: &str) -> sqlx::Result<Option<String>> {
        let mut tx = self.pg().space(space).await?;
        let link = sqlx::query_scalar!(
            "SELECT link FROM machines WHERE id = $1 AND space_id = $2",
            machine,
            space
        )
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(link)
    }
}

impl Legacy {
    /// The owner gives a machine a command: its id, or None when the machine is not in
    /// the space (`link` None, [`Store::link_of`]) or cannot take commands (yet: only printers on
    /// built-in Prusa Connect).
    pub fn queue_command(
        &mut self,
        link: Option<&str>,
        machine: &str,
        label: &str,
        gcode: &str,
    ) -> Option<u32> {
        if link.is_none_or(|link| !takes_commands(link)) {
            return None;
        }
        self.next_command = self.next_command.wrapping_add(1);
        let id = self.next_command;
        let commands = self.commands.entry(machine.to_owned()).or_default();
        commands.push(Command {
            id,
            label: label.to_owned(),
            gcode: gcode.to_owned(),
            json: false,
            background: false,
            output: Vec::new(),
            state: CommandState::Queued,
            reason: None,
            at: now(),
        });
        trim(commands);
        self.changed();
        Some(id)
    }

    /// The next command for the machine asking ([`Store::caller`]), once the one before is
    /// done: one at a time, as a printer takes them.
    pub fn next_command(&mut self, caller: Option<&Caller>) -> Option<(u32, String)> {
        self.next_command_of_any_kind(caller)
            .map(|(id, body, _)| (id, body))
    }

    /// A command of Prusa Connect's for a printer on its built-in Connect (its id): queued
    /// like G-code, sent as JSON (`SEND_FILE_INFO`, `START_PRINT`, `START_ENCRYPTED_DOWNLOAD`).
    pub fn queue_connect_command(
        &mut self,
        link: Option<&str>,
        machine: &str,
        label: &str,
        command: &serde_json::Value,
        background: bool,
    ) -> Option<u32> {
        if link != Some("prusa-connect") {
            return None;
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
            return Some(same.id);
        }
        self.next_command = self.next_command.wrapping_add(1);
        let id = self.next_command;
        let commands = self.commands.entry(machine.to_owned()).or_default();
        commands.push(Command {
            id,
            label: label.to_owned(),
            gcode: body,
            json: true,
            background,
            output: Vec::new(),
            state: CommandState::Queued,
            reason: None,
            at: now(),
        });
        trim(commands);
        self.changed();
        Some(id)
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

    /// A command given to the machine asking, by its id: what it sent (G-code or Connect's JSON).
    pub fn command_text(&self, caller: Option<&Caller>, id: u32) -> Option<String> {
        let caller = caller?;
        self.commands
            .get(&caller.id)
            .and_then(|commands| commands.iter().find(|c| c.id == id))
            .map(|c| c.gcode.clone())
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

    /// A test says when the machine's commands last changed.
    #[cfg(test)]
    pub fn set_commands_at(&mut self, machine: &str, at: i64) {
        for c in self.commands.get_mut(machine).unwrap() {
            c.at = at;
        }
    }

    /// K18 (b) for a printer on the built-in Prusa Connect (link `prusa-connect`), which has no
    /// 120 s: a command stays pending until Buddy answers it. Not seen for longer than
    /// `ONLINE_SECONDS`: its Sent and Accepted commands fail with "the printer was gone" (ours).
    pub fn fail_commands_of_gone_printer(&mut self, caller: Option<&Caller>) {
        let Some(caller) = caller.filter(|c| c.link == "prusa-connect") else {
            return;
        };
        self.fail_pending_of_gone(caller, PRINTER_GONE)
    }

    fn fail_pending_of_gone(&mut self, caller: &Caller, reason: &str) {
        if self.attached.contains_key(&caller.id)
            || caller.seen_seconds_ago.is_none_or(|s| s <= ONLINE_SECONDS)
        {
            return;
        }
        let mut any = false;
        for c in self
            .commands
            .entry(caller.id.clone())
            .or_default()
            .iter_mut()
            .filter(|c| matches!(c.state, CommandState::Sent | CommandState::Accepted))
        {
            c.state = CommandState::Rejected;
            c.reason = Some(reason.to_owned());
            any = true;
        }
        if any {
            self.changed();
        }
    }

    /// The next command, with whether it is a Connect (JSON) command.
    pub fn next_command_of_any_kind(
        &mut self,
        caller: Option<&Caller>,
    ) -> Option<(u32, String, bool)> {
        let caller = caller?;
        let prusa_connect = caller.link == "prusa-connect";
        let commands = self.commands.get_mut(&caller.id)?;
        let now = now();
        for busy in commands
            .iter_mut()
            .filter(|c| matches!(c.state, CommandState::Sent | CommandState::Accepted))
        {
            // Buddy has no time for a command: it stays pending until it answers
            // (Buddy has no time limit for a command; while one runs it answers the next with REJECTED "Processing other command", planner.cpp:661-663); only the printer being gone ends it (`fail_commands_of_gone_printer`).
            if prusa_connect || now - busy.at < COMMAND_SECONDS {
                return None;
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
        next
    }

    /// The printer says in its telemetry it is still carrying out this command (Buddy sends
    /// the `command_id` of G-code running in the background): not "no answer" while it does.
    pub fn command_still_running(&mut self, caller: Option<&Caller>, id: u32) {
        let Some(caller) = caller else {
            return;
        };
        if let Some(command) = self.commands.get_mut(&caller.id).and_then(|commands| {
            commands.iter_mut().find(|c| {
                c.id == id && matches!(c.state, CommandState::Sent | CommandState::Accepted)
            })
        }) {
            command.state = CommandState::Accepted;
            command.at = now();
        }
    }

    /// The machine tells how a command went.
    pub fn command_went(
        &mut self,
        caller: Option<&Caller>,
        id: u32,
        state: CommandState,
        reason: Option<String>,
        output: Vec<String>,
    ) {
        let Some(caller) = caller else {
            return;
        };
        let machine = caller.id.clone();
        if let Some(command) = self
            .commands
            .get_mut(&machine)
            .and_then(|commands| commands.iter_mut().find(|c| c.id == id))
        {
            // In the hub's log: how each command ended (the reason a machine gave).
            println!(
                "command {} on {machine}: {state:?}{}",
                command.label,
                reason
                    .as_deref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            );
            command.state = state;
            command.reason = reason;
            command.output.extend(output);
            let excess = command.output.len().saturating_sub(OUTPUT_KEPT);
            command.output.drain(..excess);
            command.at = now();
            self.changed();
        }
    }

    /// What a machine's command was asked as ("Probe the bed"), while it is listed.
    pub(crate) fn command_label(&self, machine: &str, id: u32) -> Option<&str> {
        self.commands
            .get(machine)?
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.label.as_str())
    }

    /// HS7b: the file a laser last framed completely, since its last Set origin (none after one).
    pub fn laser_framed_file(&self, machine: &str) -> Option<&str> {
        self.laser_framed.get(machine).map(String::as_str)
    }

    /// HS7b: whether a Set origin was done on this laser since the hub started (its place on the
    /// bed is then not known to the hub: Start burn asks the person to confirm, as LaserFoundry does).
    pub fn laser_origin_was_set(&self, machine: &str) -> bool {
        self.laser_origin_set.contains(machine)
    }
}
