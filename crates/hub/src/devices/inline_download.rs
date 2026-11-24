//! A Buddy printer fetches a file of its space over its `/p/ws` socket, as Buddy 8.0.2 asks for
//! it: the one reference for a Buddy printer (owner, 2026-11-24, #212): `reference/buddy-8.0.2`
//! (Prusa-Firmware-Buddy v8.0.2, `b1043d6`).
//!
//! What Buddy does after `START_CONNECT_DOWNLOAD` (the same as `START_INLINE_DOWNLOAD`,
//! `src/connect/command.cpp:195-202`):
//! - It asks for one segment at a time, a text frame without `event` (`render.cpp:103-121`):
//!   `{"transfer":"inline", "hash", "team_id", "transfer_id", "chunk":4096, "file_id", "start",
//!   "end"}`; `hash`, `team_id` and `transfer_id` only in the first request of a download
//!   (`transfers/download.cpp:567-574`). A segment is at most `512 * 512` bytes
//!   (`download.cpp:70`), `end` inclusive (`:559`), `file_id` its own random number for the
//!   download (`:511`, a `uint32_t`, `download.hpp:159`).
//! - It takes the bytes as frames of type `T`: the letter, the `file_id` as 8 hex digits in the
//!   place of a command id, then the bytes (`connect/connect.cpp:362-365`, `:455-456`,
//!   `:515-516`).
//! - A `T` frame without bytes is the server saying it failed, and so are bytes past `end`, or
//!   another `file_id`: the download fails (`download.cpp:581-588`). It asks for the next segment
//!   once the last one is all there (`:557`).
//!
//! Ours: which file a request may have. The `hash` is the hub's id of a file of the space
//! (`printer_files.rs` `start_transfer`), and the `team_id` must be the printer's space, as for
//! the agent's `/p/teams/{team}/files/{hash}/raw` (`prusa_connect.rs` `raw_file`).

use serde_json::Value;

/// One request for a segment, as Buddy writes it (`render.cpp:103-121`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub file_id: u32,
    pub start: u64,
    /// Inclusive (`download.cpp:559`).
    pub end: u64,
    pub chunk: usize,
    /// `hash` and `team_id`, in the first request of a download only (`download.cpp:567-574`).
    pub first: Option<(String, i64)>,
}

/// A frame is a segment request when it says `"transfer":"inline"` (`render.cpp:108`); `None`
/// for any other frame. `Some(Err(file_id))` when Buddy's members are not all there: the
/// download is answered as failed (an empty `T` frame).
pub(crate) fn request(frame: &Value) -> Option<Result<Request, Option<u32>>> {
    if frame.get("transfer").and_then(Value::as_str) != Some("inline") {
        return None;
    }
    let file_id = frame
        .get("file_id")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok());
    let number = |key: &str| frame.get(key).and_then(Value::as_u64);
    let parsed = (|| {
        let first = match (frame.get("hash"), frame.get("team_id")) {
            (None, None) => None,
            (Some(hash), Some(team)) => Some((hash.as_str()?.to_owned(), team.as_i64()?)),
            _ => return None,
        };
        Some(Request {
            file_id: file_id?,
            start: number("start")?,
            end: number("end")?,
            chunk: usize::try_from(number("chunk")?).ok().filter(|&c| c > 0)?,
            first,
        })
    })();
    Some(parsed.ok_or(file_id))
}

/// The bytes of a segment as `T` frames of `chunk` bytes (`connect.cpp:515-516`).
pub(crate) fn frames(file_id: u32, bytes: &[u8], chunk: usize) -> Vec<Vec<u8>> {
    bytes
        .chunks(chunk.max(1))
        .map(|piece| {
            let mut frame = header(file_id);
            frame.extend_from_slice(piece);
            frame
        })
        .collect()
}

/// A `T` frame without bytes: the server failed (`download.cpp:585`).
pub(crate) fn failed(file_id: u32) -> Vec<u8> {
    header(file_id)
}

/// `T` and the id as 8 hex digits (`connect.cpp:364-365`, `:456`).
fn header(file_id: u32) -> Vec<u8> {
    format!("T{file_id:08x}").into_bytes()
}

/// How many bytes of a file of `size` bytes a segment `start..=end` has; `None` when it is not
/// all in the file (Buddy fails a download whose bytes do not reach `end`, or go past it,
/// `download.cpp:584-586`, `:536-537`).
pub(crate) fn segment_len(start: u64, end: u64, size: u64) -> Option<u64> {
    (start <= end && end < size).then(|| end - start + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The first request of a download carries the file; the next ones only their segment.
    #[test]
    fn a_request_as_buddy_writes_it() {
        let first = json!({"transfer": "inline", "hash": "0123456789abcdef", "team_id": 7,
            "transfer_id": 3, "chunk": 4096, "file_id": 4000000000u32, "start": 0, "end": 262143});
        assert_eq!(
            request(&first),
            Some(Ok(Request {
                file_id: 4000000000,
                start: 0,
                end: 262143,
                chunk: 4096,
                first: Some(("0123456789abcdef".into(), 7)),
            }))
        );
        let next = json!({"transfer": "inline", "chunk": 4096, "file_id": 5, "start": 262144,
            "end": 300000});
        assert_eq!(request(&next).unwrap().unwrap().first, None);
        assert_eq!(request(&json!({"state": "IDLE"})), None, "telemetry");
        assert_eq!(
            request(&json!({"transfer": "inline", "file_id": 5})),
            Some(Err(Some(5)))
        );
    }

    /// `T`, the id in 8 hex digits, then at most `chunk` bytes; an empty one says it failed.
    #[test]
    fn t_frames_as_buddy_reads_them() {
        let frames = frames(0xab, &[1, 2, 3, 4, 5], 2);
        assert_eq!(frames.len(), 3);
        assert_eq!(&frames[0][..9], b"T000000ab");
        assert_eq!(&frames[0][9..], [1, 2]);
        assert_eq!(&frames[2][9..], [5]);
        assert_eq!(failed(0xab), b"T000000ab");
    }

    #[test]
    fn a_segment_is_all_in_the_file() {
        assert_eq!(segment_len(0, 9, 10), Some(10));
        assert_eq!(segment_len(5, 5, 10), Some(1));
        assert_eq!(segment_len(0, 10, 10), None);
        assert_eq!(segment_len(6, 5, 10), None);
    }
}
