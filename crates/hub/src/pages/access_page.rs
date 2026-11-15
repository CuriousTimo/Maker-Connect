//! Access: getting in. Doors with locks, door sensors and RFID badges: physical access, a
//! thing of its own (machine use is crate::permissions_page). Planned, not built: a design
//! placeholder in the Space page's Access tab.
//!
//! The plan: Home Assistant does the hardware (it reads the RFID tags, knows the door
//! sensors, controls the locks); Maker Connect says who may open which door, and when. A
//! scan in Home Assistant asks Maker Connect ("may this tag open the front door now?"),
//! and opens the lock on a yes; Home Assistant keeps the last answers to go on when the
//! server cannot be reached.

/// The Access tab: what it will hold, with example rows, marked as planned.
pub(crate) fn placeholder() -> String {
    let step = |n: u8, title: &str, text: &str| {
        format!(r#"<li><span class="a-n">{n}</span><b>{title}</b><small>{text}</small></li>"#)
    };
    let flow = [
        step(
            1,
            "Scan",
            "Someone holds their badge to the reader at the door.",
        ),
        step(
            2,
            "Home Assistant asks",
            "Its automation asks Maker Connect: may this tag open this door now?",
        ),
        step(
            3,
            "Maker Connect answers",
            "From the badge, the person, their role and the door's times.",
        ),
        step(
            4,
            "The lock opens",
            "Home Assistant opens it; the door sensor tells when it closed again.",
        ),
    ]
    .concat();
    format!(
        r#"<section class="section access-plan"><div class="a-head"><h3>Access</h3><span class="chip planned">Planned</span></div><p class="sub">Getting in: doors with locks, badges, door sensors. Not built yet: this is how it is meant to look. Home Assistant does the hardware; Maker Connect says who may open which door, and when.</p><ol class="a-flow">{flow}</ol></section>

<section class="section access-plan"><div class="a-head"><h3>Home Assistant</h3><span class="chip planned">Planned</span></div><p class="sub">The connection: Home Assistant's address and a long-lived access token (made under your profile in Home Assistant).</p>
<div class="a-form"><input disabled placeholder="http://homeassistant.local:8123" aria-label="Home Assistant address"><input disabled placeholder="Long-lived access token" aria-label="Token"><button disabled>Connect</button></div>
<p class="muted a-note">Not connected.</p></section>

<section class="section access-plan"><div class="a-head"><h3>Doors</h3><span class="chip planned">Planned</span></div><p class="sub">Each door: its lock and its door sensor in Home Assistant, and when it may be opened.</p>
<table class="list a-example"><thead><tr><th>Door</th><th>Lock</th><th>Sensor</th><th>Now</th><th>Open to</th></tr></thead><tbody>
<tr><td><b>Front door</b></td><td><code>lock.front_door</code></td><td><code>binary_sensor.front_door</code></td><td><span class="a-state ok">closed · locked</span></td><td><small>members, 08:00–23:00</small></td></tr>
<tr><td><b>Workshop</b></td><td><code>lock.workshop</code></td><td><code>binary_sensor.workshop_door</code></td><td><span class="a-state warn">open</span></td><td><small>Workshop group, always</small></td></tr>
</tbody></table><p class="muted a-note">Example rows.</p></section>

<section class="section access-plan"><div class="a-head"><h3>Badges</h3><span class="chip planned">Planned</span></div><p class="sub">An RFID tag per person: hold a new one to a reader, and give it to someone. Lost: block it here, at once.</p>
<table class="list a-example"><thead><tr><th>Who</th><th>Badge</th><th>Since</th><th></th></tr></thead><tbody>
<tr><td><b>Mia</b></td><td><code>04:A3:5F:1C</code></td><td><small>2026-11-06</small></td><td><button class="small danger" disabled>Block</button></td></tr>
<tr><td><b>Timo</b></td><td><code>04:7B:22:9E</code></td><td><small>2026-11-01</small></td><td><button class="small danger" disabled>Block</button></td></tr>
</tbody></table><p class="muted a-note">Example rows.</p></section>

<section class="section access-plan"><div class="a-head"><h3>Who may open which door</h3><span class="chip planned">Planned</span></div><p class="sub">Per door: everyone with a role (members, admins), or a group of people, at set times. Apart from machine permissions: getting in is not the same as using a laser.</p>
<table class="list a-example"><thead><tr><th>Door</th><th>Who</th><th>When</th></tr></thead><tbody>
<tr><td><b>Front door</b></td><td>members and up</td><td><small>every day 08:00–23:00</small></td></tr>
<tr><td><b>Workshop</b></td><td>Workshop group</td><td><small>always</small></td></tr>
</tbody></table><p class="muted a-note">Example rows.</p></section>

<section class="section access-plan"><div class="a-head"><h3>Door log</h3><span class="chip planned">Planned</span></div><p class="sub">Who opened which door when, and who was refused.</p>
<ul class="a-log a-example"><li><time>09:12</time><b>Mia</b> opened the front door</li><li><time>08:47</time><b>unknown badge</b> was refused at the workshop</li><li><time>08:30</time><b>Timo</b> opened the front door</li></ul><p class="muted a-note">Example rows.</p></section>"#
    )
}
