# Makes req1..req18.txt: go2rtc v1.9.15 (7c05066) pkg/onvif client run against a local server, its nonce and
# time fixed (envelope.go patched: nonce abcdefghij012345, created 2026-11-21T10:20:30.5Z). Usage: make-fixtures.sh SCRATCHDIR
# then copy SCRATCHDIR/goref/out/req*.txt here.
set -e
S=${1:?scratch dir}/goref
G=${GO2RTC:-$(cd "$(dirname "$0")/../../../../.." && pwd)/reference/go2rtc}
rm -rf $S
mkdir -p $S/pkg/onvif $S/pkg/core $S/cmd $S/out
printf 'module github.com/AlexxIT/go2rtc\n\ngo 1.22\n' > $S/go.mod
cp $G/pkg/onvif/client.go $G/pkg/onvif/helpers.go $S/pkg/onvif/
sed -e 's/core.RandString(16, 36)/"abcdefghij012345"/' -e 's/time.Now().UTC().Format(time.RFC3339Nano)/"2026-11-21T10:20:30.5Z"/' -e '/pkg\/core"/d' -e '/"time"/d' $G/pkg/onvif/envelope.go > $S/pkg/onvif/envelope.go
cat > $S/pkg/onvif/consts.go <<'EOF'
package onvif

const (
	DeviceGetServices          = "GetServices"
	DeviceGetCapabilities      = "GetCapabilities"
	DeviceGetDeviceInformation = "GetDeviceInformation"
	MediaGetProfiles           = "GetProfiles"
)
EOF
cat > $S/pkg/core/core.go <<'EOF'
package core

import "strings"

func RandString(size, base byte) string { return "00000000000000000000000000000000"[:size] }
func Between(s, sub1, sub2 string) string {
	i := strings.Index(s, sub1)
	if i < 0 {
		return ""
	}
	s = s[i+len(sub1):]
	if j := strings.Index(s, sub2); j >= 0 {
		return s[:j]
	}
	return s
}
EOF
cat > $S/cmd/main.go <<'EOF'
package main

import (
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"

	"github.com/AlexxIT/go2rtc/pkg/onvif"
)

func main() {
	dir := os.Args[1]
	n := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		n++
		os.WriteFile(fmt.Sprintf("%s/req%d.txt", dir, n), []byte(r.Method+" "+r.URL.Path+"\n"+r.Header.Get("Content-Type")+"\n"+r.Header.Get("User-Agent")+"\n"+string(b)), 0644)
		s := string(b)
		switch {
		case strings.Contains(s, "GetCapabilities"):
			fmt.Fprint(w, `<tds:GetCapabilitiesResponse><tt:Media><tt:XAddr>http://x/onvif/media_service</tt:XAddr></tt:Media><tt:Imaging><tt:XAddr>http://x/onvif/imaging_service</tt:XAddr></tt:Imaging></tds:GetCapabilitiesResponse>`)
		case strings.Contains(s, "GetProfiles"):
			fmt.Fprint(w, `<trt:GetProfilesResponse><trt:Profiles token="prof_a" fixed="true"><tt:Name>a</tt:Name></trt:Profiles><trt:Profiles token="prof_b"></trt:Profiles></trt:GetProfilesResponse>`)
		case strings.Contains(s, "GetStreamUri"):
			fmt.Fprint(w, `<trt:GetStreamUriResponse><trt:MediaUri><tt:Uri>rtsp://192.168.1.123:554/cam?channel=1&amp;subtype=1</tt:Uri></trt:MediaUri></trt:GetStreamUriResponse>`)
		}
	}))
	defer srv.Close()
	host := strings.TrimPrefix(srv.URL, "http://")
	for _, raw := range []string{"onvif://admin:s3cret@" + host + "?subtype=1", "onvif://" + host} {
		c, err := onvif.NewClient(raw)
		if err != nil {
			panic(err)
		}
		u, err := c.GetURI()
		fmt.Println(raw, "=>", u, err)
		c.GetSnapshotUri("t1")
		c.GetVideoSourceConfiguration("t2")
		c.GetProfile("t3")
		c.GetServiceCapabilities()
		c.GetName()
		c.HasSnapshots()
	}
}
EOF
cd $S
go run ./cmd $S/out
ls out
