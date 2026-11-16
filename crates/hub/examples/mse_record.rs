//! For the MSE audit (docs/audits/go2rtc-mse.md): records frames of a camera through retina,
//! as the hub's `cameras` does, and writes two files: the muxer's input (`frames.bin`: the
//! `avcC`, the size, then per frame its RTP timestamp and its payload after go2rtc's
//! start-of-access-unit filter) and our muxer's output (`ours.bin`: the content type, the init segment, then
//! one fragment per frame from the first key frame), each record length-prefixed (u32 BE).
//! go2rtc's own muxer reads `frames.bin` (scripts/go2rtc-mse-check/) and writes `go2rtc.bin`.
//!
//!   cargo run -p maker-connect-hub --features cameras --example mse_record -- \
//!       rtsp://IP:8554/Streaming/Channels/101 admin admin 200 out/

use futures_util::StreamExt as _;
use maker_connect_hub::cameras::mp4;
use std::io::Write as _;

fn record(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    out.extend_from_slice(b);
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (url, user, pass, count, dir) = (&a[0], &a[1], &a[2], a[3].parse::<usize>()?, &a[4]);
    let creds = Some(retina::client::Credentials { username: user.clone(), password: pass.clone() });
    let mut session = retina::client::Session::describe(
        url.parse()?,
        retina::client::SessionOptions::default()
            .creds(creds)
            .user_agent("Retina webcodecs example".to_owned()),
    )
    .await?;
    let video = session
        .streams()
        .iter()
        .position(|s| s.media() == "video" && s.encoding_name() == "h264")
        .ok_or("no h264 stream")?;
    session
        .setup(
            video,
            retina::client::SetupOptions::default().frame_format(retina::codec::FrameFormat::MP4),
        )
        .await?;
    let mut session = session
        .play(retina::client::PlayOptions::default())
        .await?
        .demuxed()?;
    let (mut frames, mut ours) = (Vec::new(), Vec::new());
    let mut muxer = mp4::Muxer::new();
    let (mut ps, mut start, mut n) = (None::<Vec<u8>>, false, 0);
    while let Some(item) = session.next().await {
        let retina::codec::CodecItem::VideoFrame(frame) = item? else {
            continue;
        };
        let Some(retina::codec::ParametersRef::Video(p)) =
            session.streams()[frame.stream_id()].parameters()
        else {
            continue;
        };
        if ps.is_none() {
            let avcc = p.extra_data().to_vec();
            let (w, h) = p.pixel_dimensions();
            record(&mut frames, &avcc);
            record(&mut frames, &[(w >> 8) as u8, w as u8, (h >> 8) as u8, h as u8]);
            let (_, sps, pps) = mp4::decode_config(&avcc);
            record(&mut ours, mp4::content_type(sps).as_bytes());
            record(&mut ours, &mp4::Muxer::get_init(sps, pps, Some((w as u16, h as u16))));
            ps = Some(mp4::join_nalu(&[sps.unwrap_or_default(), pps.unwrap_or_default()]));
        }
        let Some(payload) = mp4::start_of_access_unit(frame.data(), ps.as_deref().unwrap()) else {
            continue;
        };
        if !start {
            if !mp4::is_keyframe(&payload) {
                continue;
            }
            start = true;
        }
        let ts = frame.timestamp().timestamp() as u32;
        let mut rec = ts.to_be_bytes().to_vec();
        rec.extend_from_slice(&payload);
        record(&mut frames, &rec);
        record(&mut ours, &muxer.get_payload(ts, &payload));
        n += 1;
        if n == count {
            break;
        }
    }
    std::fs::create_dir_all(dir)?;
    std::fs::File::create(format!("{dir}/frames.bin"))?.write_all(&frames)?;
    std::fs::File::create(format!("{dir}/ours.bin"))?.write_all(&ours)?;
    println!("{n} frames");
    Ok(())
}
