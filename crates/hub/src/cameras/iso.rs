//! ISO BMFF boxes for fragmented MP4, ported from go2rtc `pkg/iso/iso.go`, `pkg/iso/atoms.go` and
//! the H.264 case of `pkg/iso/codecs.go` `WriteVideo` (AlexxIT/go2rtc v1.9.15, `7c05066`,
//! Copyright (c) 2022 Alexey Khit, MIT), function by function. Not ported: audio tracks and
//! H.265 (the microphone is never opened; H.264 only for now).

/// go2rtc's `Movie`: a byte buffer with the starts of the open atoms.
pub struct Movie {
    b: Vec<u8>,
    start: Vec<usize>,
}

pub const MOOV: &str = "moov";
pub const MOOV_MVEX: &str = "mvex";

const SAMPLE_IS_NON_SYNC: u32 = 0x10000;
const SAMPLE_DEPENDS_ON1: u32 = 0x1000000;
const SAMPLE_DEPENDS_ON2: u32 = 0x2000000;

pub const SAMPLE_VIDEO_I_FRAME: u32 = SAMPLE_DEPENDS_ON2;
pub const SAMPLE_VIDEO_NON_I_FRAME: u32 = SAMPLE_DEPENDS_ON1 | SAMPLE_IS_NON_SYNC;

const TFHD_DEFAULT_SAMPLE_DURATION: u32 = 0x000008;
const TFHD_DEFAULT_SAMPLE_SIZE: u32 = 0x000010;
const TFHD_DEFAULT_SAMPLE_FLAGS: u32 = 0x000020;
const TFHD_DEFAULT_BASE_IS_MOOF: u32 = 0x020000;

const TRUN_DATA_OFFSET: u32 = 0x000001;
const TRUN_SAMPLE_CTS: u32 = 0x0000800;

impl Movie {
    pub fn new(size: usize) -> Movie {
        Movie {
            b: Vec::with_capacity(size),
            start: Vec::new(),
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.b
    }

    pub fn start_atom(&mut self, name: &str) {
        self.start.push(self.b.len());
        self.b.extend_from_slice(&[0, 0, 0, 0]);
        self.b.extend_from_slice(name.as_bytes());
    }

    pub fn end_atom(&mut self) {
        let i = self.start.pop().expect("an atom to end");
        let size = (self.b.len() - i) as u32;
        self.b[i..i + 4].copy_from_slice(&size.to_be_bytes());
    }

    pub fn write(&mut self, b: &[u8]) {
        self.b.extend_from_slice(b);
    }

    fn write_bytes(&mut self, b: &[u8]) {
        self.b.extend_from_slice(b);
    }

    fn write_string(&mut self, s: &str) {
        self.b.extend_from_slice(s.as_bytes());
    }

    fn skip(&mut self, n: usize) {
        self.b.resize(self.b.len() + n, 0);
    }

    fn write_uint16(&mut self, v: u16) {
        self.b.extend_from_slice(&v.to_be_bytes());
    }

    fn write_uint24(&mut self, v: u32) {
        self.b
            .extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
    }

    fn write_uint32(&mut self, v: u32) {
        self.b.extend_from_slice(&v.to_be_bytes());
    }

    fn write_uint64(&mut self, v: u64) {
        self.b.extend_from_slice(&v.to_be_bytes());
    }

    /// go2rtc's `WriteFloat16`: the integer part, then the fraction times 256.
    fn write_float16(&mut self, f: f64) {
        let (i, f) = (f.trunc(), f.fract() * 256.0);
        self.b.extend_from_slice(&[i as u8, f as u8]);
    }

    /// go2rtc's `WriteFloat32`: the integer part, then the fraction times 65536.
    fn write_float32(&mut self, f: f64) {
        let (i, f) = (f.trunc(), f.fract() * 65536.0);
        let (i16_, f16) = (i as u16, f as u16);
        self.b
            .extend_from_slice(&[(i16_ >> 8) as u8, i16_ as u8, (f16 >> 8) as u8, f16 as u8]);
    }

    fn write_matrix(&mut self) {
        self.write_uint32(0x00010000);
        self.skip(4);
        self.skip(4);
        self.skip(4);
        self.write_uint32(0x00010000);
        self.skip(4);
        self.skip(4);
        self.skip(4);
        self.write_uint32(0x40000000);
    }

    pub fn write_file_type(&mut self) {
        self.start_atom("ftyp");
        self.write_string("iso5");
        self.write_uint32(512);
        self.write_string("iso5");
        self.write_string("iso6");
        self.write_string("mp41");
        self.end_atom();
    }

    pub fn write_movie_header(&mut self) {
        self.start_atom("mvhd");
        self.skip(1); // version
        self.skip(3); // flags
        self.skip(4); // create time
        self.skip(4); // modify time
        self.write_uint32(1000); // time scale
        self.skip(4); // duration
        self.write_float32(1.0); // preferred rate
        self.write_float16(1.0); // preferred volume
        self.skip(10); // reserved
        self.write_matrix();
        self.skip(6 * 4); // predefined?
        self.write_uint32(0xFFFFFFFF); // next track ID
        self.end_atom();
    }

    fn write_track_header(&mut self, id: u32, width: u16, height: u16) {
        const TKHD_TRACK_ENABLED: u32 = 0x0001;
        const TKHD_TRACK_IN_MOVIE: u32 = 0x0002;
        self.start_atom("tkhd");
        self.skip(1); // version
        self.write_uint24(TKHD_TRACK_ENABLED | TKHD_TRACK_IN_MOVIE);
        self.skip(4); // create time
        self.skip(4); // modify time
        self.write_uint32(id); // trackID
        self.skip(4); // reserved
        self.skip(4); // duration
        self.skip(8); // reserved
        self.skip(2); // layer
        if width > 0 {
            self.skip(2);
            self.skip(2);
        } else {
            self.write_uint16(1); // alternate group
            self.write_float16(1.0); // volume
        }
        self.skip(2); // reserved
        self.write_matrix();
        if width > 0 {
            self.write_float32(f64::from(width));
            self.write_float32(f64::from(height));
        } else {
            self.skip(4);
            self.skip(4);
        }
        self.end_atom();
    }

    fn write_media_header(&mut self, timescale: u32) {
        self.start_atom("mdhd");
        self.skip(1); // version
        self.skip(3); // flags
        self.skip(4); // creation time
        self.skip(4); // modification time
        self.write_uint32(timescale); // timescale
        self.skip(4); // duration
        self.write_uint16(0x55C4); // language (Unspecified)
        self.skip(2); // quality
        self.end_atom();
    }

    fn write_media_handler(&mut self, s: &str, name: &str) {
        self.start_atom("hdlr");
        self.skip(1); // version
        self.skip(3); // flags
        self.skip(4);
        self.write_string(s); // handler type (4 byte!)
        self.skip(3 * 4); // reserved
        self.write_string(name); // handler name (any len)
        self.skip(1); // end string
        self.end_atom();
    }

    fn write_video_media_info(&mut self) {
        self.start_atom("vmhd");
        self.skip(1); // version
        self.write_uint24(1); // flags (You should always set this flag to 1)
        self.skip(2); // graphics mode
        self.skip(3 * 2); // op color
        self.end_atom();
    }

    fn write_data_info(&mut self) {
        self.start_atom("dinf");
        self.start_atom("dref");
        self.skip(1); // version
        self.skip(3); // flags
        self.write_uint32(1); // childrens

        self.start_atom("url ");
        self.skip(1); // version
        self.write_uint24(1); // flags (self reference)
        self.end_atom();

        self.end_atom(); // DREF
        self.end_atom(); // DINF
    }

    fn write_sample_table(&mut self, write_sample_desc: impl FnOnce(&mut Movie)) {
        self.start_atom("stbl");

        self.start_atom("stsd");
        self.skip(1); // version
        self.skip(3); // flags
        self.write_uint32(1); // entry count
        write_sample_desc(self);
        self.end_atom();

        self.start_atom("stts");
        self.skip(1); // version
        self.skip(3); // flags
        self.skip(4); // entry count
        self.end_atom();

        self.start_atom("stsc");
        self.skip(1); // version
        self.skip(3); // flags
        self.skip(4); // entry count
        self.end_atom();

        self.start_atom("stsz");
        self.skip(1); // version
        self.skip(3); // flags
        self.skip(4); // sample size
        self.skip(4); // entry count
        self.end_atom();

        self.start_atom("stco");
        self.skip(1); // version
        self.skip(3); // flags
        self.skip(4); // entry count
        self.end_atom();

        self.end_atom();
    }

    pub fn write_track_extend(&mut self, id: u32) {
        self.start_atom("trex");
        self.skip(1); // version
        self.skip(3); // flags
        self.write_uint32(id); // trackID
        self.write_uint32(1); // default sample description index
        self.skip(4); // default sample duration
        self.skip(4); // default sample size
        self.skip(4); // default sample flags
        self.end_atom();
    }

    /// go2rtc's `WriteVideoTrack`, H.264 only.
    pub fn write_video_track(
        &mut self,
        id: u32,
        timescale: u32,
        width: u16,
        height: u16,
        conf: &[u8],
    ) {
        self.start_atom("trak");
        self.write_track_header(id, width, height);

        self.start_atom("mdia");
        self.write_media_header(timescale);
        self.write_media_handler("vide", "VideoHandler");

        self.start_atom("minf");
        self.write_video_media_info();
        self.write_data_info();
        self.write_sample_table(|m| m.write_video_h264(width, height, conf));
        self.end_atom(); // MINF

        self.end_atom(); // MDIA
        self.end_atom(); // TRAK
    }

    /// `codecs.go` `WriteVideo`, its H.264 case.
    fn write_video_h264(&mut self, width: u16, height: u16, conf: &[u8]) {
        self.start_atom("avc1");
        self.skip(6);
        self.write_uint16(1); // data_reference_index
        self.skip(2); // version
        self.skip(2); // revision
        self.skip(4); // vendor
        self.skip(4); // temporal quality
        self.skip(4); // spatial quality
        self.write_uint16(width); // width
        self.write_uint16(height); // height
        self.write_float32(72.0); // horizontal resolution
        self.write_float32(72.0); // vertical resolution
        self.skip(4); // reserved
        self.write_uint16(1); // frame count
        self.skip(32); // compressor name
        self.write_uint16(24); // depth
        self.write_uint16(0xFFFF); // color table id (-1)

        self.start_atom("avcC");
        self.write(conf);
        self.end_atom(); // AVCC

        self.start_atom("pasp"); // Pixel Aspect Ratio
        self.write_uint32(1); // hSpacing
        self.write_uint32(1); // vSpacing
        self.end_atom();

        self.end_atom(); // AVC1
    }

    #[allow(clippy::too_many_arguments)]
    pub fn write_movie_fragment(
        &mut self,
        seq: u32,
        tid: u32,
        duration: u32,
        size: u32,
        flags: u32,
        dts: u64,
        cts: u32,
    ) {
        self.start_atom("moof");

        self.start_atom("mfhd");
        self.skip(1); // version
        self.skip(3); // flags
        self.write_uint32(seq); // sequence number
        self.end_atom();

        self.start_atom("traf");

        self.start_atom("tfhd");
        self.skip(1); // version
        self.write_uint24(
            TFHD_DEFAULT_SAMPLE_DURATION
                | TFHD_DEFAULT_SAMPLE_SIZE
                | TFHD_DEFAULT_SAMPLE_FLAGS
                | TFHD_DEFAULT_BASE_IS_MOOF,
        );
        self.write_uint32(tid); // track id
        self.write_uint32(duration); // default sample duration
        self.write_uint32(size); // default sample size
        self.write_uint32(flags); // default sample flags
        self.end_atom();

        self.start_atom("tfdt");
        self.write_bytes(&[1]); // version
        self.skip(3); // flags
        self.write_uint64(dts); // base media decode time
        self.end_atom();

        self.start_atom("trun");
        self.skip(1); // version

        if cts == 0 {
            self.write_uint24(TRUN_DATA_OFFSET); // flags
            self.write_uint32(1); // sample count

            // data offset: current pos + uint32 len + MDAT header len
            let offset = self.b.len() as u32 + 4 + 8;
            self.write_uint32(offset);
        } else {
            self.write_uint24(TRUN_DATA_OFFSET | TRUN_SAMPLE_CTS);
            self.write_uint32(1);

            // data offset: current pos + uint32 len + CTS + MDAT header len
            let offset = self.b.len() as u32 + 4 + 4 + 8;
            self.write_uint32(offset);
            self.write_uint32(cts);
        }

        self.end_atom(); // TRUN

        self.end_atom(); // TRAF

        self.end_atom(); // MOOF
    }

    pub fn write_data(&mut self, b: &[u8]) {
        self.start_atom("mdat");
        self.write(b);
        self.end_atom();
    }
}
