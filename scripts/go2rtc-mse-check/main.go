// go2rtc's own muxer on recorded frames, for the MSE audit (docs/audits/go2rtc-mse.md).
// Reads frames.bin (written by crates/hub/examples/mse_record.rs), writes go2rtc.bin in the
// same form as ours.bin: the content type, the init segment, one fragment per frame; built
// against reference/go2rtc (v1.9.15, 7c05066) through the replace in go.mod.
//
//	go run . <dir>
package main

import (
	"encoding/base64"
	"encoding/binary"
	"os"

	"github.com/AlexxIT/go2rtc/pkg/core"
	"github.com/AlexxIT/go2rtc/pkg/h264"
	"github.com/AlexxIT/go2rtc/pkg/mp4"
	"github.com/pion/rtp"
)

func records(b []byte) (out [][]byte) {
	for len(b) >= 4 {
		n := binary.BigEndian.Uint32(b)
		out = append(out, b[4:4+n])
		b = b[4+n:]
	}
	return
}

func put(out []byte, b []byte) []byte {
	out = binary.BigEndian.AppendUint32(out, uint32(len(b)))
	return append(out, b...)
}

func main() {
	dir := os.Args[1]
	in, err := os.ReadFile(dir + "/frames.bin")
	if err != nil {
		panic(err)
	}
	recs := records(in)
	_, sps, pps := h264.DecodeConfig(recs[0])
	// The camera's codec as go2rtc's RTSP client gives it: from the SDP's fmtp.
	codec := &core.Codec{
		Name:      core.CodecH264,
		ClockRate: 90000,
		FmtpLine: "packetization-mode=1;sprop-parameter-sets=" +
			base64.StdEncoding.EncodeToString(sps) + "," + base64.StdEncoding.EncodeToString(pps),
	}
	m := &mp4.Muxer{}
	m.AddTrack(codec)
	var out []byte
	out = put(out, []byte(mp4.ContentType([]*core.Codec{codec})))
	init, err := m.GetInit()
	if err != nil {
		panic(err)
	}
	out = put(out, init)
	for _, r := range recs[2:] {
		packet := &rtp.Packet{Header: rtp.Header{Timestamp: binary.BigEndian.Uint32(r)}, Payload: r[4:]}
		out = put(out, m.GetPayload(0, packet))
	}
	if err = os.WriteFile(dir+"/go2rtc.bin", out, 0o644); err != nil {
		panic(err)
	}
}
