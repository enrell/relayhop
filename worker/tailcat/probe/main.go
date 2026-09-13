// Command probe exercises a pinned Tailcat (WireGuard + magicsock + DERP)
// build between two endpoints and reports honest, machine-readable numbers.
//
//	probe serve --udp-port 4007 --derpmap-url URL --addr-file PATH
//	  Runs an echo server for one UDP port and publishes its tailcat address.
//
//	probe run --addr ADDR --derpmap-url URL --udp-port 4007 [tuning flags]
//	  Dials the server, measures handshake time, disco path (direct vs DERP
//	  relay), UDP RTT/loss and a bounded throughput blast, then prints one
//	  JSON report line to stdout.
//
// The relay path is reported as data, never as failure: falling back to DERP
// is the behavior under test. Only a failed handshake/dial exits non-zero.
package main

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net"
	"os"
	"os/signal"
	"runtime"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/tailscale/tailcat"
)

const maxPayload = tailcat.MaxUDPPayload // 1232

func logger(verbose bool) func(string, ...any) {
	if verbose {
		return log.Printf
	}
	return func(string, ...any) {}
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "uso: probe <serve|run> [flags]")
		os.Exit(2)
	}
	var err error
	switch os.Args[1] {
	case "serve":
		err = serve(os.Args[2:])
	case "run":
		err = run(os.Args[2:])
	default:
		fmt.Fprintf(os.Stderr, "subcomando desconhecido: %s\n", os.Args[1])
		os.Exit(2)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "probe: %v\n", err)
		os.Exit(1)
	}
}

// --- serve ---

func serve(args []string) error {
	fs := flag.NewFlagSet("serve", flag.ContinueOnError)
	udpPort := fs.Int("udp-port", 4007, "UDP port to echo")
	derpMapURL := fs.String("derpmap-url", "", "alternate DERP map URL (empty = public default)")
	addrFile := fs.String("addr-file", "", "file to write the tailcat address to")
	verbose := fs.Bool("verbose", false, "tailcat debug logs")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if *udpPort <= 0 || *udpPort > 65535 {
		return fmt.Errorf("porta UDP inválida: %d", *udpPort)
	}

	var datagrams atomic.Uint64
	var bytes atomic.Uint64
	s := &tailcat.Server{
		DERPMapURL: *derpMapURL,
		Logf:       logger(*verbose),
		OnUDP: func(port uint16) func(tailcat.ConnPacketConn) {
			if int(port) != *udpPort {
				return nil
			}
			return func(c tailcat.ConnPacketConn) {
				defer c.Close()
				buf := make([]byte, maxPayload)
				for {
					n, err := c.Read(buf)
					if err != nil {
						return
					}
					if _, err := c.Write(buf[:n]); err != nil {
						return
					}
					datagrams.Add(1)
					bytes.Add(uint64(n))
				}
			}
		},
	}
	if err := s.Start(); err != nil {
		return fmt.Errorf("server start: %w", err)
	}
	addr := string(s.TailcatAddr())
	fmt.Println(addr)
	if *addrFile != "" {
		if err := os.WriteFile(*addrFile, []byte(addr+"\n"), 0600); err != nil {
			return fmt.Errorf("addr-file: %w", err)
		}
	}
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM)
	<-sig
	fmt.Fprintf(os.Stderr, "probe serve: %d datagramas, %d bytes ecoados\n",
		datagrams.Load(), bytes.Load())
	return nil
}

// --- run ---

type pathSample struct {
	Direct   bool    `json:"direct"`
	Endpoint string  `json:"endpoint,omitempty"`
	RegionID int     `json:"region_id,omitempty"`
	RTTMs    float64 `json:"rtt_ms"`
}

type report struct {
	OK             bool       `json:"ok"`
	HandshakeMs    float64    `json:"handshake_ms"`
	DiscoCold      pathSample `json:"disco_cold"`
	DiscoWarm      pathSample `json:"disco_warm"`
	DiscoPostBlast pathSample `json:"disco_post_blast"`
	RTTMinMs       float64    `json:"rtt_min_ms"`
	RTTAvgMs       float64    `json:"rtt_avg_ms"`
	RTTMaxMs       float64    `json:"rtt_max_ms"`
	Sent           int        `json:"datagrams_sent"`
	Received       int        `json:"datagrams_recv"`
	LossPct        float64    `json:"loss_pct"`
	BlastMbps      float64    `json:"blast_mbps_measured"`
	BlastSent      int        `json:"blast_sent"`
	BlastRecv      int        `json:"blast_recv"`
	RSSKb          int64      `json:"rss_kb"`
	Error          string     `json:"error,omitempty"`
}

func discoSample(ctx context.Context, cl *tailcat.Client) (pathSample, error) {
	t0 := time.Now()
	res, err := cl.DiscoPing(ctx)
	if err != nil {
		return pathSample{}, err
	}
	rtt := time.Since(t0).Seconds() * 1000
	out := pathSample{RTTMs: rtt}
	if res.Endpoint != "" {
		out.Direct = true
		out.Endpoint = res.Endpoint
	} else {
		out.RegionID = int(res.DERPRegionID)
	}
	return out, nil
}

func run(args []string) error {
	fs := flag.NewFlagSet("run", flag.ContinueOnError)
	addr := fs.String("addr", "", "server tailcat address (required)")
	derpMapURL := fs.String("derpmap-url", "", "alternate DERP map URL (empty = public default)")
	udpPort := fs.Int("udp-port", 4007, "server UDP port")
	rttCount := fs.Int("rtt-count", 50, "RTT datagrams")
	rttSize := fs.Int("rtt-size", 1000, "RTT datagram payload bytes")
	blastMbps := fs.Float64("blast-mbps", 5, "throughput blast rate")
	blastSecs := fs.Int("blast-secs", 3, "throughput blast duration seconds")
	verbose := fs.Bool("verbose", false, "tailcat debug logs")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if *addr == "" {
		return fmt.Errorf("--addr é obrigatório")
	}
	if *rttSize < 16 || *rttSize > maxPayload {
		return fmt.Errorf("--rtt-size deve estar em 16..%d", maxPayload)
	}

	rep := report{OK: false}
	fail := func(err error) error {
		rep.Error = err.Error()
		rep.RSSKb = rssKb()
		out, _ := json.Marshal(rep)
		fmt.Println(string(out))
		return err
	}

	cl := tailcat.NewClient(tailcat.Addr(*addr))
	cl.DERPMapURL = *derpMapURL
	cl.Logf = logger(*verbose)
	ctx := context.Background()

	t0 := time.Now()
	if _, err := cl.Ping(ctx); err != nil {
		return fail(fmt.Errorf("handshake ping: %w", err))
	}
	rep.HandshakeMs = time.Since(t0).Seconds() * 1000

	cold, err := discoSample(ctx, cl)
	if err != nil {
		return fail(fmt.Errorf("disco cold: %w", err))
	}
	rep.DiscoCold = cold

	pc, err := cl.DialUDPPort(ctx, uint16(*udpPort))
	if err != nil {
		return fail(fmt.Errorf("dial udp: %w", err))
	}
	defer pc.Close()

	// RTT series: seq(4B) | sendUnixNano(8B) | padding.
	payload := make([]byte, *rttSize)
	var rtts []float64
	recvBuf := make([]byte, maxPayload)
	for i := 0; i < *rttCount; i++ {
		binary.BigEndian.PutUint32(payload[0:4], uint32(i))
		binary.BigEndian.PutUint64(payload[4:12], uint64(time.Now().UnixNano()))
		start := time.Now()
		if _, err := pc.Write(payload); err != nil {
			return fail(fmt.Errorf("udp write: %w", err))
		}
		if err := pc.SetReadDeadline(time.Now().Add(2 * time.Second)); err != nil {
			return fail(err)
		}
		n, err := pc.Read(recvBuf)
		if err != nil {
			if ne, ok := err.(net.Error); ok && ne.Timeout() {
				continue // lost datagram; counted below
			}
			return fail(fmt.Errorf("udp read: %w", err))
		}
		if n < 12 || binary.BigEndian.Uint32(recvBuf[0:4]) != uint32(i) {
			continue // corrupted echo; counted as loss
		}
		rtts = append(rtts, time.Since(start).Seconds()*1000)
	}
	rep.Sent = *rttCount
	rep.Received = len(rtts)
	if len(rtts) > 0 {
		min, max, sum := rtts[0], rtts[0], 0.0
		for _, r := range rtts {
			if r < min {
				min = r
			}
			if r > max {
				max = r
			}
			sum += r
		}
		rep.RTTMinMs, rep.RTTAvgMs, rep.RTTMaxMs = min, sum/float64(len(rtts)), max
	}
	rep.LossPct = 100 * float64(*rttCount-len(rtts)) / float64(*rttCount)

	warm, err := discoSample(ctx, cl)
	if err != nil {
		return fail(fmt.Errorf("disco warm: %w", err))
	}
	rep.DiscoWarm = warm

	// Bounded throughput blast with 1000-byte datagrams.
	if *blastMbps <= 0 {
		*blastMbps = 1
	}
	blastPayload := make([]byte, 1000)
	perDatagram := time.Second * time.Duration(1000*8) / time.Duration(*blastMbps*1e6)
	deadline := time.Now().Add(time.Duration(*blastSecs) * time.Second)
	next := time.Now()
	// Clear the RTT phase's read deadline before the blast drain.
	if err := pc.SetReadDeadline(time.Time{}); err != nil {
		return fail(err)
	}
	var blastRecv atomic.Int64
	readDone := make(chan struct{})
	go func() {
		defer close(readDone)
		for {
			n, err := pc.Read(recvBuf)
			if err != nil {
				return
			}
			if n >= 4 {
				blastRecv.Add(1)
			}
		}
	}()
	for seq := 0; time.Now().Before(deadline); seq++ {
		binary.BigEndian.PutUint32(blastPayload[0:4], uint32(seq))
		if _, err := pc.Write(blastPayload); err != nil {
			break
		}
		rep.BlastSent++
		next = next.Add(perDatagram)
		if wait := time.Until(next); wait > 0 {
			time.Sleep(wait)
		}
	}
	// Short drain for in-flight echoes, then stop the reader.
	time.Sleep(500 * time.Millisecond)
	pc.Close()
	<-readDone
	rep.BlastRecv = int(blastRecv.Load())
	elapsed := float64(*blastSecs) + 0.5
	rep.BlastMbps = float64(rep.BlastRecv*1000*8) / elapsed / 1e6

	post, err := discoSample(ctx, cl)
	if err != nil {
		return fail(fmt.Errorf("disco post-blast: %w", err))
	}
	rep.DiscoPostBlast = post

	rep.OK = true
	rep.RSSKb = rssKb()
	out, _ := json.Marshal(rep)
	fmt.Println(string(out))
	return nil
}

// rssKb reports the process resident set size in KiB (-1 when unavailable).
func rssKb() int64 {
	if runtime.GOOS != "linux" {
		return -1
	}
	data, err := os.ReadFile("/proc/self/statm")
	if err != nil {
		return -1
	}
	parts := strings.Fields(string(data))
	if len(parts) < 2 {
		return -1
	}
	pages, err := strconv.ParseInt(parts[1], 10, 64)
	if err != nil {
		return -1
	}
	return pages * int64(os.Getpagesize()) / 1024
}
