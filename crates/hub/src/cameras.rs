//! Network cameras (ONVIF/RTSP), straight to the hub: the module `cameras` (docs/hub-side.md,
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
//! - the example ends the program when the RTSP session ends; here only that camera's
//!   session ends and its viewers are closed (no reconnecting: not in the example);
//! - the example serves only after the camera is open; here the websocket answers 503 until
//!   then (and after the session ended);
//! - `run` is split into `open` and `run`; no ctrl-C `select!` (the hub has its own);
//! - error types (`anyhow` → `Box<dyn Error>`/`retina::Error`) and log texts are the hub's;
//!   the two `debug!` lines and `next_frame_num` (only for them) are left out; the URL is
//!   logged without user and password (the example logs no URL).

pub mod iso;
pub mod mp4;
pub mod page;
pub mod store;

use std::collections::HashMap;
use std::sync::Mutex;

use axum::extract::ws::Message;
use bytes::Bytes;
use futures_util::StreamExt as _;
use retina::{client::SetupOptions, codec::VideoParameters};

use crate::Shared;

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
}

/// The sessions, by camera id.
static RUNNING: Mutex<Option<HashMap<i64, Running>>> = Mutex::new(None);

fn running<T>(f: impl FnOnce(&mut HashMap<i64, Running>) -> T) -> T {
    f(RUNNING.lock().unwrap().get_or_insert_with(HashMap::new))
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
            let _ = frames_tx.send(FrameItem {
                data: Bytes::copy_from_slice(frame.data()),
                rtp_timestamp: frame.timestamp().timestamp() as u32,
                avcc: parameters.0.clone(),
                size: parameters.1,
            });
        }
    }
    Ok(())
}

/// The example's `run`, up to serving: describe, pick the video stream, set up, play.
async fn open(source: &Source) -> Result<retina::client::Demuxed, Box<dyn std::error::Error>> {
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

/// Opens the camera and keeps its frames going to the viewers, until the session ends.
async fn run(id: i64, source: Source) {
    let (frames_tx, _) = tokio::sync::broadcast::channel(64);
    let shown = {
        let mut url = source.url.clone();
        let _ = url.set_password(None);
        let _ = url.set_username("");
        url
    };
    let session = match open(&source).await {
        Ok(session) => session,
        Err(error) => {
            eprintln!("camera {id} {shown}: {error}");
            return;
        }
    };
    eprintln!("camera {id} {shown}: playing");
    running(|r| {
        if let Some(r) = r.get_mut(&id) {
            r.frames = Some(frames_tx.clone());
        }
    });
    let res = receive_frames(session, frames_tx).await;
    // The last sender goes: every viewer's `recv` ends with `Closed`, as when the example ends.
    running(|r| {
        if let Some(r) = r.get_mut(&id) {
            r.frames = None;
        }
    });
    eprintln!("camera {id} {shown}: RTSP session ending: {res:?}");
}

/// Starts a camera's session, ending the one it had (its viewers are closed).
pub fn start(camera: &store::Camera) {
    let url = match camera.url.parse::<url::Url>() {
        Ok(url) => url,
        Err(error) => {
            eprintln!("camera {}: not an address: {error}", camera.id);
            stop(camera.id);
            return;
        }
    };
    let source = Source {
        url,
        username: camera.username.clone(),
        password: camera.password.clone(),
    };
    let id = camera.id;
    running(|r| {
        if let Some(old) = r.remove(&id) {
            old.task.abort();
        }
        let task = tokio::spawn(run(id, source)).abort_handle();
        r.insert(id, Running { task, frames: None });
    });
}

/// Ends a camera's session; its viewers are closed.
pub fn stop(id: i64) {
    running(|r| {
        if let Some(old) = r.remove(&id) {
            old.task.abort();
        }
    });
}

/// The frames of a camera that plays, for one more viewer.
pub(crate) fn subscribe(id: i64) -> Option<tokio::sync::broadcast::Receiver<FrameItem>> {
    running(|r| {
        r.get(&id)
            .and_then(|r| r.frames.as_ref())
            .map(|tx| tx.subscribe())
    })
}

/// Starts the sessions of every camera, when the hub starts.
pub fn start_all(store: &Shared) {
    let cameras = store.lock().unwrap().all_cameras().unwrap_or_default();
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
/// once). No pings (go2rtc's websocket has none). When the viewer's codecs have no H.264 the
/// answer is go2rtc's error form; its text ("codecs not matched") without go2rtc's lists.
pub(crate) async fn serve_mse(
    mut socket: axum::extract::ws::WebSocket,
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
    let mut muxer = mp4::Muxer::new();
    let mut ps: Option<Vec<u8>> = None;
    let mut start = false;
    loop {
        tokio::select! {
            res = frames_rx.recv() => {
                let item = match res {
                    Ok(item) => item,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                };
                let (_, sps, pps) = mp4::decode_config(&item.avcc);
                if ps.is_none() {
                    // The answer and the init, once the parameters are known.
                    if !ws_send(&mut socket, ws_json("mse", &mp4::content_type(sps))).await {
                        return;
                    }
                    let init = mp4::Muxer::get_init(sps, pps, Some(item.size));
                    if !ws_send(&mut socket, Message::Binary(init.into())).await {
                        return;
                    }
                    ps = Some(mp4::join_nalu(&[sps.unwrap_or_default(), pps.unwrap_or_default()]));
                }
                let Some(payload) = mp4::start_of_access_unit(&item.data, ps.as_deref().unwrap_or_default()) else {
                    continue;
                };
                if !start {
                    if !mp4::is_keyframe(&payload) {
                        continue;
                    }
                    start = true;
                }
                let fragment = muxer.get_payload(item.rtp_timestamp, &payload);
                if !ws_send(&mut socket, Message::Binary(fragment.into())).await {
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
