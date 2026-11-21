//! Network cameras (ONVIF/RTSP), straight to the hub: the module `cameras` (docs/plans/cameras.md,
//! "Network cameras"). Here: the cameras' sessions and video; [`store`]: the table and who may
//! watch; [`page`]: the pages, the player and the video route.
//!
//! The RTSP side is ported from retina's own example, function by function:
//! `reference/retina/examples/client/src/webcodecs.rs` (`run`, `receive_frames`) at
//! scottlamb/retina `2b6b7fe0a6f6a5bf69d5e9c8d4fd785ccd090873`, Copyright (C) The Retina
//! Authors, SPDX-License-Identifier: MIT OR Apache-2.0; the RTSP client is the `retina` crate
//! 0.4.22. The browser side is go2rtc's (owner, 2026-11-16: "MSE"): [`serve_mse`], [`mp4`],
//! [`iso`] and the player `cameras/video-rtc.js`, from go2rtc v1.9.15 (`7c05066`, MIT);
//! audit `docs/audits/go2rtc-mse.md`.
//!
//! What differs from retina's example, and only on the hub's side (towards the camera it
//! sends what the example sends: the same describe, user agent, stream choice, setup, play):
//! - one session per camera of the hub (`cameras` table) instead of one per program: a
//!   [`Running`] per camera id (its task and, while it plays, its `frames_tx`); a session
//!   starts with the hub and when its camera is added or saved, and stops when it is removed
//!   or saved again (owner, 2026-11-16);
//! - its viewers are go2rtc's MSE websocket (`/cameras/{id}/ws`, [`serve_mse`]) instead of the
//!   example's WebCodecs websocket: `FrameItem` carries the frame, its RTP timestamp and its
//!   parameters (`avcC`, size) instead of the example's two WebCodecs messages; the example's
//!   `serve_websocket`, its `index.html` and `webcodecs.js` are not used;
//! - a camera's user and password come from its form, an empty field being "not given" as a
//!   flag left out of the example's command line; the example's `creds()` is inline, and its
//!   `(None, Some)` arm (`unreachable!()`, clap forbids it) is an error before connecting;
//! - `log::` lines are `eprintln!`, as everywhere in the hub; no `--tls` (MSE needs none);
//! - the example ends the program when the RTSP session ends; here the camera is opened again
//!   as go2rtc v1.9.15 does it (`internal/streams/producer.go`, `worker` and `reconnect`: at
//!   once, then after 1 s, 5 s, 10 s or a minute; owner, 2026-11-20: "pak de reconnect op
//!   zoals go2rtc"), and its viewers stay connected;
//! - the example serves only after the camera is open; here the websocket answers 503 until
//!   then (when the first open failed, it is not retried: go2rtc retries it with the next
//!   viewer, the hub when the camera is saved or the hub starts);
//! - `run` is split into `open` and `run`; no ctrl-C `select!` (the hub has its own);
//! - error types (`anyhow` → `Box<dyn Error>`/`retina::Error`) and log texts are the hub's;
//!   the two `debug!` lines and `next_frame_num` (only for them) are left out; the URL is
//!   logged without user and password (the example logs no URL).

pub mod iso;
pub mod mp4;
pub mod onvif;
pub mod page;
pub mod store;

use std::collections::HashMap;
use std::sync::Mutex;

use axum::extract::ws::Message;
use bytes::Bytes;
use futures_util::StreamExt as _;
use retina::{client::SetupOptions, codec::VideoParameters};

use crate::Store;

/// A camera to open, as the example's `Source`.
pub struct Source {
    /// `rtsp://` URL to connect to.
    pub url: url::Url,
    /// Username to send if the server requires authentication.
    pub username: Option<String>,
    /// Password; requires username.
    pub password: Option<String>,
}

/// A camera's session: its task, and its frames for every viewer (the example's
/// `frames_tx`), there while it plays.
struct Running {
    task: tokio::task::AbortHandle,
    frames: Option<tokio::sync::broadcast::Sender<FrameItem>>,
    /// The frames since the last key frame, for a new viewer to start at once (ours, owner
    /// 2026-11-16: "doe a maar nu"; go2rtc's viewer waits for the next key frame).
    gop: Gop,
}

/// The frames from the last key frame on, kept as they are sent; filled and read under its
/// lock, so a new viewer gets these and then exactly the frames after them.
type Gop = std::sync::Arc<Mutex<Vec<FrameItem>>>;

/// The sessions, by camera id.
static RUNNING: Mutex<Option<HashMap<i64, Running>>> = Mutex::new(None);

fn running<T>(f: impl FnOnce(&mut HashMap<i64, Running>) -> T) -> T {
    f(RUNNING.lock().unwrap().get_or_insert_with(HashMap::new))
}

/// Why a camera does not play, by camera id: what its session's last start or end said (the
/// hub's log line, without user and password), for its pages; gone when it starts again.
static PROBLEMS: Mutex<Option<HashMap<i64, String>>> = Mutex::new(None);

fn problems<T>(f: impl FnOnce(&mut HashMap<i64, String>) -> T) -> T {
    f(PROBLEMS.lock().unwrap().get_or_insert_with(HashMap::new))
}

/// How a camera's session is doing, for its pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    /// Its frames go to the viewers.
    Playing,
    /// Its session is opening the camera.
    Connecting,
    /// It does not play: what went wrong.
    Failed(String),
    /// No session (the hub has not started it).
    Off,
}

/// A camera's [`Status`] now.
pub(crate) fn status(id: i64) -> Status {
    if let Some(problem) = problems(|p| p.get(&id).cloned()) {
        return Status::Failed(problem);
    }
    running(|r| match r.get(&id) {
        Some(r) if r.frames.is_some() => Status::Playing,
        Some(_) => Status::Connecting,
        None => Status::Off,
    })
}

/// An error's text as a camera's page shows it: without its password, should the error
/// quote it.
fn shown_error(error: &dyn std::fmt::Display, source: &Source) -> String {
    let text = error.to_string();
    match source.password.as_deref().filter(|p| !p.is_empty()) {
        Some(password) => text.replace(password, "…"),
        None => text,
    }
}

/// A frame ready to be broadcast to all websockets (the example's `FrameItem`, carrying what
/// go2rtc's muxer needs instead of the example's WebCodecs messages): the frame in AVC form,
/// its 32-bit RTP timestamp, and its parameters (`avcC`, width and height).
#[derive(Clone)]
pub(crate) struct FrameItem {
    pub(crate) data: Bytes,
    pub(crate) rtp_timestamp: u32,
    pub(crate) avcc: Bytes,
    pub(crate) size: (u16, u16),
}

/// retina's `avcC` record and the picture's width and height (for the muxer).
fn mse_parameters(parameters: &VideoParameters) -> (Bytes, (u16, u16)) {
    let (width, height) = parameters.pixel_dimensions();
    (
        Bytes::copy_from_slice(parameters.extra_data()),
        (
            u16::try_from(width).unwrap_or(u16::MAX),
            u16::try_from(height).unwrap_or(u16::MAX),
        ),
    )
}

/// Receives frames from the RTSP server and sends them to `frames_tx` (the example's
/// `receive_frames`: parameters kept until a frame has new ones, nothing before the first).
async fn receive_frames(
    mut session: retina::client::Demuxed,
    frames_tx: tokio::sync::broadcast::Sender<FrameItem>,
    gop: Gop,
) -> Result<(), retina::Error> {
    let mut cur_parameters: Option<(Bytes, (u16, u16))> = None;
    while let Some(item) = session.next().await {
        if let retina::codec::CodecItem::VideoFrame(frame) = item? {
            let stream_params = session.streams()[frame.stream_id()].parameters();
            let parameters = if let Some(cur_parameters) = cur_parameters.as_ref()
                && !frame.has_new_parameters()
            {
                cur_parameters
            } else if let Some(retina::codec::ParametersRef::Video(p)) = stream_params {
                cur_parameters.insert(mse_parameters(p))
            } else if cur_parameters.is_some() {
                unreachable!()
            } else {
                continue; // wait for initial parameters.
            };
            let item = FrameItem {
                data: Bytes::copy_from_slice(frame.data()),
                rtp_timestamp: frame.timestamp().timestamp() as u32,
                avcc: parameters.0.clone(),
                size: parameters.1,
            };
            // Ours: keep the frames since the last key frame (retina's random access point).
            let mut kept = gop.lock().unwrap();
            if frame.is_random_access_point() {
                kept.clear();
            }
            if frame.is_random_access_point() || !kept.is_empty() {
                kept.push(item.clone());
            }
            let _ = frames_tx.send(item);
            drop(kept);
        }
    }
    Ok(())
}

/// An `onvif://user:pass@host:port[?subtype=N]` address (go2rtc's scheme, [`onvif`]) is turned into
/// the camera's RTSP address by `GetCapabilities`, `GetProfiles` and `GetStreamUri`; any other
/// address is used as it is. The user and password of the camera's form go into the address when it
/// has none (go2rtc takes them from the address only); in the answer they are split off again for
/// the RTSP client, which takes them as credentials (retina refuses them in the URL).
async fn resolved(source: &Source) -> Result<Source, String> {
    if source.url.scheme() != "onvif" {
        return Ok(Source {
            url: source.url.clone(),
            username: source.username.clone(),
            password: source.password.clone(),
        });
    }
    let mut address = source.url.clone();
    if address.username().is_empty()
        && address.password().is_none()
        && let Some(username) = source.username.as_deref().filter(|u| !u.is_empty())
    {
        let _ = address.set_username(username);
        let _ = address.set_password(source.password.as_deref().filter(|p| !p.is_empty()));
    }
    let rtsp = tokio::task::spawn_blocking(move || onvif::resolve(address.as_str()))
        .await
        .map_err(|e| e.to_string())??;
    let mut url: url::Url = rtsp.parse().map_err(|e: url::ParseError| e.to_string())?;
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    };
    let username = (!url.username().is_empty()).then(|| decode(url.username()));
    let password = url.password().map(decode);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    Ok(Source {
        url,
        username: username.or_else(|| source.username.clone()),
        password: password.or_else(|| source.password.clone()),
    })
}

/// The example's `run`, up to serving: describe, pick the video stream, set up, play.
async fn open(source: &Source) -> Result<retina::client::Demuxed, Box<dyn std::error::Error>> {
    let source = &resolved(source).await?;
    let creds = match (source.username.clone(), source.password.clone()) {
        (Some(username), password) => Some(retina::client::Credentials {
            username,
            password: password.unwrap_or_default(),
        }),
        (None, None) => None,
        (None, Some(_)) => return Err("a camera password requires a username".into()),
    };
    let mut session = retina::client::Session::describe(
        source.url.clone(),
        retina::client::SessionOptions::default()
            .creds(creds)
            .user_agent("Retina webcodecs example".to_owned()),
    )
    .await?;

    let video_stream_i = session.streams().iter().position(|s| {
        if s.media() == "video" {
            let encoding_name = s.encoding_name();
            if matches!(encoding_name, "h264" | "h265" | "jpeg") {
                eprintln!("camera: using {encoding_name} video stream");
                return true;
            }
            eprintln!(
                "camera: ignoring {} video stream because it's unsupported",
                s.encoding_name(),
            );
        }
        false
    });
    let Some(video_stream_i) = video_stream_i else {
        return Err("no suitable video stream found".into());
    };
    session
        .setup(
            video_stream_i,
            SetupOptions::default().frame_format(retina::codec::FrameFormat::MP4),
        )
        .await?;
    Ok(session
        .play(retina::client::PlayOptions::default())
        .await?
        .demuxed()?)
}

/// Opens the camera and keeps its frames going to the viewers. When the session ends, with an
/// error or without, it opens the camera again (go2rtc's `Producer.worker`, which calls
/// `reconnect(workerID, 0)` whenever `conn.Start()` returns); the viewers stay connected and get
/// the frames of the new session (go2rtc's `receiver.Replace`). It runs until [`stop`] or
/// [`start`] aborts it (go2rtc: the `workerID` changes).
async fn run(id: i64, source: Source) {
    let (frames_tx, _) = tokio::sync::broadcast::channel(64);
    let shown = {
        let mut url = source.url.clone();
        let _ = url.set_password(None);
        let _ = url.set_username("");
        url
    };
    let mut session = match open(&source).await {
        Ok(session) => session,
        Err(error) => {
            let error = shown_error(&error, &source);
            eprintln!("camera {id} {shown}: {error}");
            problems(|p| p.insert(id, error));
            return;
        }
    };
    let gop = running(|r| {
        r.get_mut(&id).map(|r| {
            r.frames = Some(frames_tx.clone());
            r.gop.clone()
        })
    });
    let Some(gop) = gop else {
        return;
    };
    loop {
        eprintln!("camera {id} {shown}: playing");
        problems(|p| p.remove(&id));
        let res = receive_frames(session, frames_tx.clone(), gop.clone()).await;
        // The kept frames belong to the session that ended.
        gop.lock().unwrap().clear();
        eprintln!("camera {id} {shown}: RTSP session ending: {res:?}");
        let ended = match res {
            Ok(()) => "The camera ended its stream.".to_owned(),
            Err(error) => format!("The stream broke off: {}", shown_error(&error, &source)),
        };
        problems(|p| p.insert(id, format!("{ended} Reconnecting…")));
        session = reconnect(id, &source, &shown, &ended).await;
    }
}

/// How long go2rtc's `Producer.reconnect` waits after its `retry`-th failed attempt: a second
/// for the first five, 5 seconds up to ten, 10 seconds up to twenty, then a minute.
fn retry_wait(retry: u32) -> std::time::Duration {
    std::time::Duration::from_secs(match retry {
        0..5 => 1,
        5..10 => 5,
        10..20 => 10,
        _ => 60,
    })
}

/// go2rtc's `Producer.reconnect`: opens the camera again at once, and after each failure again
/// after [`retry_wait`], until it opens.
async fn reconnect(
    id: i64,
    source: &Source,
    shown: &url::Url,
    ended: &str,
) -> retina::client::Demuxed {
    let mut retry = 0;
    loop {
        eprintln!("camera {id} {shown}: retry={retry}");
        // The error's text only: `open`'s error is not `Send` and must not live across the wait.
        let error = match open(source).await {
            Ok(session) => return session,
            Err(error) => shown_error(&error, source),
        };
        eprintln!("camera {id} {shown}: {error}");
        problems(|p| p.insert(id, format!("{ended} Reconnecting: {error}")));
        tokio::time::sleep(retry_wait(retry)).await;
        retry += 1;
    }
}

/// Starts a camera's session, ending the one it had (its viewers are closed).
pub fn start(camera: &store::Camera) {
    let url = match camera.url.parse::<url::Url>() {
        Ok(url) => url,
        Err(error) => {
            eprintln!("camera {}: not an address: {error}", camera.id);
            stop(camera.id);
            problems(|p| p.insert(camera.id, format!("Not an address: {error}")));
            return;
        }
    };
    let source = Source {
        url,
        username: camera.username.clone(),
        password: camera.password.clone(),
    };
    let id = camera.id;
    problems(|p| p.remove(&id));
    running(|r| {
        if let Some(old) = r.remove(&id) {
            old.task.abort();
        }
        let task = tokio::spawn(run(id, source)).abort_handle();
        r.insert(
            id,
            Running {
                task,
                frames: None,
                gop: Gop::default(),
            },
        );
    });
}

/// Ends a camera's session; its viewers are closed.
pub fn stop(id: i64) {
    problems(|p| p.remove(&id));
    running(|r| {
        if let Some(old) = r.remove(&id) {
            old.task.abort();
        }
    });
}

/// For one more viewer of a camera that plays: the frames since the last key frame, and the
/// frames after them.
pub(crate) fn subscribe(
    id: i64,
) -> Option<(Vec<FrameItem>, tokio::sync::broadcast::Receiver<FrameItem>)> {
    running(|r| {
        let r = r.get(&id)?;
        let tx = r.frames.as_ref()?;
        let kept = r.gop.lock().unwrap();
        Some((kept.clone(), tx.subscribe()))
    })
}

/// Starts the sessions of every camera, when the hub starts.
pub async fn start_all(store: &Store) {
    let cameras = store.all_cameras().await.unwrap_or_default();
    for camera in &cameras {
        start(camera);
    }
}

/// go2rtc's `Message` as `WriteJSON` writes it: `{"type":…,"value":…}` and a newline
/// (`internal/api/ws/ws.go:38-41`, gorilla's `WriteJSON` encodes with `json.Encoder`).
fn ws_json(kind: &str, value: &str) -> Message {
    let mut text = serde_json::json!({ "type": kind, "value": value }).to_string();
    text.push('\n');
    Message::Text(text.into())
}

/// One message with go2rtc's 5 s write deadline (`ws.go:111-118`); `false` when it failed.
async fn ws_send(socket: &mut axum::extract::ws::WebSocket, msg: Message) -> bool {
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), socket.send(msg)).await,
        Ok(Ok(()))
    )
}

/// A viewer over MSE, as go2rtc's `handlerWSMSE` (`internal/mp4/ws.go:13-43`) with its
/// `mp4.Consumer` (`pkg/mp4/consumer.go`) and `Muxer`: the viewer asks with
/// `{"type":"mse","value":"<its codecs>"}`; the answer is `{"type":"mse","value":<content
/// type>}`, then binary: the init segment, then one fragment per frame from the first key frame.
/// Differences, on the hub's side: go2rtc knows the SPS from the SDP and answers at once; here
/// the answer and the init wait for the first frame's parameters (retina's). A viewer that falls
/// behind loses frames and is not resynced, as go2rtc's full sender drops (`c.start` is set
/// once). No pings (go2rtc's websocket has none). Ours (owner, 2026-11-16): a new viewer first
/// gets the frames since the last key frame (`backlog`), so the picture comes at once instead
/// of at the next key frame; go2rtc's player catches up to live itself (`video-rtc.js:478`). When the viewer's codecs have no H.264 the
/// answer is go2rtc's error form; its text ("codecs not matched") without go2rtc's lists.
pub(crate) async fn serve_mse(
    mut socket: axum::extract::ws::WebSocket,
    backlog: Vec<FrameItem>,
    mut frames_rx: tokio::sync::broadcast::Receiver<FrameItem>,
) {
    // The viewer's first message: what it can play.
    let codecs = loop {
        match socket.recv().await {
            Some(Ok(Message::Text(text))) => {
                let msg: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(msg) => msg,
                    Err(_) => return, // go2rtc: ReadJSON fails, the socket closes
                };
                if msg["type"] == "mse" {
                    break msg["value"].as_str().unwrap_or_default().to_owned();
                }
            }
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
            Some(Ok(_)) => {}
        }
    };
    if !codecs.is_empty() && !codecs.split(',').any(|c| c.starts_with("avc1.")) {
        let _ = ws_send(
            &mut socket,
            ws_json("error", "mse: streams: codecs not matched"),
        )
        .await;
        return;
    }
    let mut viewer = Viewer {
        muxer: mp4::Muxer::new(),
        ps: None,
        start: false,
    };
    // Ours: the frames since the last key frame first, so the picture comes at once.
    for item in backlog {
        if !viewer.send(&mut socket, &item).await {
            return;
        }
    }
    loop {
        tokio::select! {
            res = frames_rx.recv() => {
                let item = match res {
                    Ok(item) => item,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                };
                if !viewer.send(&mut socket, &item).await {
                    return;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

/// One viewer's muxer, as go2rtc's consumer per viewer.
struct Viewer {
    muxer: mp4::Muxer,
    ps: Option<Vec<u8>>,
    start: bool,
}

impl Viewer {
    /// One frame to the viewer: the answer and the init first, once the parameters are known;
    /// then a fragment per frame from the first key frame. `false` when the socket failed.
    async fn send(&mut self, socket: &mut axum::extract::ws::WebSocket, item: &FrameItem) -> bool {
        let (_, sps, pps) = mp4::decode_config(&item.avcc);
        if self.ps.is_none() {
            if !ws_send(socket, ws_json("mse", &mp4::content_type(sps))).await {
                return false;
            }
            let init = mp4::Muxer::get_init(sps, pps, Some(item.size));
            if !ws_send(socket, Message::Binary(init.into())).await {
                return false;
            }
            self.ps = Some(mp4::join_nalu(&[
                sps.unwrap_or_default(),
                pps.unwrap_or_default(),
            ]));
        }
        let Some(payload) =
            mp4::start_of_access_unit(&item.data, self.ps.as_deref().unwrap_or_default())
        else {
            return true;
        };
        if !self.start {
            if !mp4::is_keyframe(&payload) {
                return true;
            }
            self.start = true;
        }
        let fragment = self.muxer.get_payload(item.rtp_timestamp, &payload);
        ws_send(socket, Message::Binary(fragment.into())).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// go2rtc's `Producer.reconnect` timeouts, retry by retry.
    #[test]
    fn retry_waits_as_go2rtc() {
        let secs: Vec<u64> = (0..22).map(|r| retry_wait(r).as_secs()).collect();
        let mut want = vec![1; 5];
        want.extend([5; 5]);
        want.extend([10; 10]);
        want.extend([60; 2]);
        assert_eq!(secs, want);
    }

    fn camera(id: i64, url: &str) -> store::Camera {
        store::Camera {
            id,
            space_id: 1,
            name: "Bench".to_owned(),
            source: "rtsp".to_owned(),
            url: url.to_owned(),
            username: Some("admin".to_owned()),
            password: Some("s3cret".to_owned()),
            machine_id: None,
            view_role: crate::access::Role::Member,
            created_at: 0,
        }
    }

    /// A camera that cannot start says why on its pages, until it starts again or goes.
    #[test]
    fn a_camera_that_cannot_start_says_why() {
        let id = -41;
        assert_eq!(status(id), Status::Off);
        start(&camera(id, "not an address"));
        let Status::Failed(problem) = status(id) else {
            panic!("{:?}", status(id));
        };
        assert!(problem.starts_with("Not an address: "), "{problem}");
        stop(id);
        assert_eq!(status(id), Status::Off);
    }

    /// The password never shows in an error a page shows.
    #[test]
    fn a_shown_error_has_no_password() {
        let source = Source {
            url: "rtsp://192.0.2.1/stream".parse().unwrap(),
            username: Some("admin".to_owned()),
            password: Some("s3cret".to_owned()),
        };
        assert_eq!(
            shown_error(&"401 for admin:s3cret", &source),
            "401 for admin:…"
        );
    }

    /// `resolved`: another address unchanged; an onvif address resolved through a stand-in camera
    /// (the ONVIF fixtures' answers), the credentials taken from the address or else the form, and
    /// split off the RTSP address again.
    #[tokio::test]
    async fn resolved_addresses() {
        let source = |url: &str, user: Option<&str>, pass: Option<&str>| Source {
            url: url.parse().unwrap(),
            username: user.map(str::to_owned),
            password: pass.map(str::to_owned),
        };
        let rtsp = "rtsp://192.168.1.123:554/cam?channel=1&subtype=1";

        let r = resolved(&source("rtsp://a:b@10.0.0.9/x", Some("u"), Some("p")))
            .await
            .unwrap();
        assert_eq!(
            (r.url.as_str(), r.username.as_deref(), r.password.as_deref()),
            ("rtsp://a:b@10.0.0.9/x", Some("u"), Some("p"))
        );

        // User in the address: sent to the camera, and the RTSP credentials (go2rtc copies the
        // address's user into an answer without one, `client.go:85-87`).
        let (host, rx) = onvif::tests::camera();
        let r = resolved(&source(
            &format!("onvif://admin:s3cret@{host}"),
            Some("form"),
            Some("f"),
        ))
        .await
        .unwrap();
        assert_eq!(
            (r.url.as_str(), r.username.as_deref(), r.password.as_deref()),
            (rtsp, Some("admin"), Some("s3cret"))
        );
        assert!(
            rx.recv()
                .unwrap()
                .contains("<wsse:Username>admin</wsse:Username>")
        );

        // No user in the address: the form's, a character that needs encoding included.
        let (host, rx) = onvif::tests::camera();
        let r = resolved(&source(
            &format!("onvif://{host}"),
            Some("form"),
            Some("p@ss:w/rd"),
        ))
        .await
        .unwrap();
        assert_eq!(
            (r.url.as_str(), r.username.as_deref(), r.password.as_deref()),
            (rtsp, Some("form"), Some("p@ss:w/rd"))
        );
        assert!(
            rx.recv()
                .unwrap()
                .contains("<wsse:Username>form</wsse:Username>")
        );

        // No user anywhere: no UsernameToken, no credentials.
        let (host, rx) = onvif::tests::camera();
        let r = resolved(&source(&format!("onvif://{host}"), None, None))
            .await
            .unwrap();
        assert_eq!((r.url.as_str(), r.username, r.password), (rtsp, None, None));
        assert!(!rx.recv().unwrap().contains("UsernameToken"));

        // No camera: an error, not a panic.
        let dead = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(
            resolved(&source(&format!("onvif://{dead}"), None, None))
                .await
                .is_err()
        );
    }
}
