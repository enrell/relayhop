// Command relay is the RelayHop Tailcat worker: a supervised child process
// that shuttles exactly one control stream (TCP, reliable) and media
// datagrams (UDP, bounded) between the Rust supervisor (over stdio frames)
// and one Tailcat peer. No shell, files, SSH, exit-node or SOCKS services.
//
// Wire protocol on stdout/stdin (parent <-> worker), all length-prefixed:
// orker.
//
//	[C u32BE len][0x01][control bytes]   parent -> worker -> peer TCP stream
//	[C u32BE len][0x02][media datagram]   parent -> worker -> peer UDP datagram
//	[C u32BE len][0x81][control bytes]   peer TCP stream -> worker -> parent
//	[C u32BE len][0x82][media datagram]  peer UDP datagram -> worker -> parent
//	[C u32BE len][0xFF][close reason]    either direction: orderly shutdown
//
// Human/machine events go to stderr as one JSON object per line:
//
//	{"t":"ready","addr":"tc..."}          serve mode: address to share
//	{"t":"peer","state":"connected"}      TCP control stream established
//	{"t":"path","kind":"direct|relay|unknown"}
//	{"t":"closed","reason":"..."}         terminal event, worker will exit
//	{"t":"error","error":"..."}           fatal error, worker will exit
//
// EOF on stdin terminates the worker. serve mode usage:
//
//	relay serve --tcp-port 4106 --udp-port 4107 --derpmap-url URL
//
// join mode usage:
//
//	relay join --addr tc... --tcp-port 4106 --udp-port 4107 --derpmap-url URL
package main

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"sync"
	"time"

	"github.com/tailscale/tailcat"
)

const (
	// Frame types parent -> worker.
	fToPeerTCP = 0x01
	fToPeerUDP = 0x02
	fClose     = 0xFF
	// Frame types worker -> parent.
	fFromPeerTCP = 0x81
	fFromPeerUDP = 0x82

	maxFrame = 1 << 20 // 1 MiB control bound (matches Rust MAX_CONTROL_BYTES+slack)
	maxUDP   = tailcat.MaxUDPPayload
)

func emit(event any) {
	line, _ := json.Marshal(event)
	fmt.Fprintf(os.Stderr, "%s\n", line)
}

func emitErr(err error) {
	emit(map[string]string{"t": "error", "error": err.Error()})
}

func emitClosed(reason string) {
	emit(map[string]string{"t": "closed", "reason": reason})
}

// frameReader reads length-prefixed frames with bounded allocation.
type frameReader struct {
	r   io.Reader
	buf []byte
}

func (f *frameReader) next() (byte, []byte, error) {
	var hdr [5]byte
	if _, err := io.ReadFull(f.r, hdr[:]); err != nil {
		return 0, nil, err
	}
	n := binary.BigEndian.Uint32(hdr[:4])
	typ := hdr[4]
	if n < 1 || n > maxFrame {
		return 0, nil, fmt.Errorf("frame inválido: %d bytes", n)
	}
	body := make([]byte, n-1)
	if _, err := io.ReadFull(f.r, body); err != nil {
		return 0, nil, err
	}
	return typ, body, nil
}

var writeMu sync.Mutex

func writeFrame(w io.Writer, typ byte, body []byte) error {
	if len(body)+1 > maxFrame {
		return fmt.Errorf("frame grande demais: %d", len(body))
	}
	var hdr [5]byte
	binary.BigEndian.PutUint32(hdr[:4], uint32(len(body)+1))
	hdr[4] = typ
	writeMu.Lock()
	defer writeMu.Unlock()
	if _, err := w.Write(hdr[:]); err != nil {
		return err
	}
	_, err := w.Write(body)
	return err
}

func logger(verbose bool) func(string, ...any) {
	if verbose {
		return log.Printf
	}
	return func(string, ...any) {}
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "uso: relay <serve|join> [flags]")
		os.Exit(2)
	}
	var err error
	switch os.Args[1] {
	case "serve":
		err = serve(os.Args[2:])
	case "join":
		err = join(os.Args[2:])
	default:
		fmt.Fprintf(os.Stderr, "subcomando desconhecido: %s\n", os.Args[1])
		os.Exit(2)
	}
	if err != nil {
		emitErr(err)
		emitClosed(fmt.Sprint(err))
		os.Exit(1)
	}
	emitClosed("ok")
}

func commonFlags(fs *flag.FlagSet) (tcpPort, udpPort *int, derpMapURL *string, verbose *bool) {
	tcpPort = fs.Int("tcp-port", 4106, "TCP control port on the Tailcat node")
	udpPort = fs.Int("udp-port", 4107, "UDP media port on the Tailcat node")
	derpMapURL = fs.String("derpmap-url", "", "alternate DERP map URL (empty = public default)")
	verbose = fs.Bool("verbose", false, "tailcat debug logs to stderr")
	return tcpPort, udpPort, derpMapURL, verbose
}

// heartbeatPayload is the transport heartbeat: it establishes the
// server-side UDP flow (servers can only reply to flows they have seen)
// and refreshes NAT bindings. The peer worker drops it; it is never
// delivered to the parent. (Zero-length writes are unreliable across the
// netstack path, so the heartbeat is one marker byte.)
var heartbeatPayload = []byte{0x00}

func checkPort(name string, p int) error {
	if p <= 0 || p > 65535 {
		return fmt.Errorf("porta %s inválida: %d", name, p)
	}
	return nil
}

// pumpTCP shuttles one accepted TCP stream to the parent as 0x81 frames and
// accepts 0x01 payloads for it. Only the first connection is served;
// further connections are refused (single viewer per room).
type tcpBridge struct {
	mu   sync.Mutex
	conn net.Conn
	got  chan struct{}
	once sync.Once
}

func (b *tcpBridge) set(c net.Conn) bool {
	b.mu.Lock()
	defer b.mu.Unlock()
	select {
	case <-b.got:
		return false
	default:
	}
	b.conn = c
	close(b.got)
	return true
}

func (b *tcpBridge) send(body []byte) error {
	b.mu.Lock()
	c := b.conn
	b.mu.Unlock()
	if c == nil {
		return fmt.Errorf("sem stream de controle")
	}
	_ = c.SetWriteDeadline(time.Now().Add(10 * time.Second))
	_, err := c.Write(body)
	return err
}

func serve(args []string) error {
	fs := flag.NewFlagSet("serve", flag.ContinueOnError)
	tcpPort, udpPort, derpMapURL, verbose := commonFlags(fs)
	if err := fs.Parse(args); err != nil {
		return err
	}
	if err := checkPort("tcp", *tcpPort); err != nil {
		return err
	}
	if err := checkPort("udp", *udpPort); err != nil {
		return err
	}

	bridge := &tcpBridge{got: make(chan struct{})}
	var udpDst atomicUDPAddr

	s := &tailcat.Server{
		DERPMapURL: *derpMapURL,
		Logf:       logger(*verbose),
		OnTCP: func(port uint16) func(net.Conn) {
			if int(port) != *tcpPort {
				return nil
			}
			return func(c net.Conn) {
				if !bridge.set(c) {
					c.Close()
					return
				}
				emit(map[string]string{"t": "peer", "state": "connected"})
				go func() {
					defer c.Close()
					buf := make([]byte, 65536)
					for {
						n, err := c.Read(buf)
						if err != nil {
							return
						}
						if err := writeFrame(os.Stdout, fFromPeerTCP, buf[:n]); err != nil {
							return
						}
					}
				}()
			}
		},
		OnUDP: func(port uint16) func(tailcat.ConnPacketConn) {
			if int(port) != *udpPort {
				return nil
			}
			return func(c tailcat.ConnPacketConn) {
				defer c.Close()
				udpDst.set(c)
				buf := make([]byte, maxUDP)
				for {
					n, err := c.Read(buf)
					if err != nil {
						return
					}
					// Transport heartbeat (see heartbeatPayload): dropped here,
					// never delivered to the parent.
					if n == len(heartbeatPayload) && buf[0] == heartbeatPayload[0] {
						continue
					}
					if err := writeFrame(os.Stdout, fFromPeerUDP, buf[:n]); err != nil {
						return
					}
				}
			}
		},
	}
	if err := s.Start(); err != nil {
		return fmt.Errorf("server start: %w", err)
	}
	defer s.Close()
	// NOTE: stdout carries ONLY framed binary (see protocol header).
	// The address goes to stderr via the ready event.
	emit(map[string]string{"t": "ready", "addr": string(s.TailcatAddr())})
	return pumpParent(&parentDeps{tcp: bridge, udp: &udpDst, isServer: true})
}

// atomicUDPAddr remembers the latest UDP flow for replies.
type atomicUDPAddr struct {
	mu sync.Mutex
	c  tailcat.ConnPacketConn
}

func (a *atomicUDPAddr) set(c tailcat.ConnPacketConn) {
	a.mu.Lock()
	a.c = c
	a.mu.Unlock()
}

func (a *atomicUDPAddr) write(b []byte) error {
	a.mu.Lock()
	c := a.c
	a.mu.Unlock()
	if c == nil {
		return fmt.Errorf("sem fluxo UDP")
	}
	_ = c.SetWriteDeadline(time.Now().Add(2 * time.Second))
	_, err := c.Write(b)
	return err
}

type parentDeps struct {
	tcp      *tcpBridge
	udp      *atomicUDPAddr
	isServer bool
}

// pumpParent routes parent frames to the peer until stdin EOF or 0xFF.
// NOTE: stdin has exactly ONE reader (frameReader below). A second
// concurrent reader (e.g. an EOF watcher) would steal stream bytes and
// desync framing — EOF is reported by ReadFull itself.
func pumpParent(deps *parentDeps) error {
	fr := &frameReader{r: os.Stdin}
	for {
		typ, body, err := fr.next()
		if err != nil {
			if err == io.EOF || err == io.ErrUnexpectedEOF {
				return nil
			}
			return err
		}
		switch typ {
		case fToPeerTCP:
			if err := deps.tcp.send(body); err != nil {
				return err
			}
		case fToPeerUDP:
			if len(body) > maxUDP {
				return fmt.Errorf("datagrama excede %d bytes", maxUDP)
			}
			if len(body) == 0 {
				return fmt.Errorf("datagrama vazio (heartbeat é interno do worker)")
			}
			// NOTE: single byte 0x00 is the transport heartbeat and is
			// dropped by the peer; app datagrams are >= 20 bytes.
			if err := deps.udp.write(body); err != nil {
				return err
			}
		case fClose:
			return nil
		default:
			return fmt.Errorf("tipo de frame desconhecido: %#x", typ)
		}
	}
}

func join(args []string) error {
	fs := flag.NewFlagSet("join", flag.ContinueOnError)
	tcpPort, udpPort, derpMapURL, verbose := commonFlags(fs)
	addr := fs.String("addr", "", "server tailcat address (required)")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if *addr == "" {
		return fmt.Errorf("--addr é obrigatório")
	}
	if err := checkPort("tcp", *tcpPort); err != nil {
		return err
	}
	if err := checkPort("udp", *udpPort); err != nil {
		return err
	}

	cl := tailcat.NewClient(tailcat.Addr(*addr))
	cl.DERPMapURL = *derpMapURL
	cl.Logf = logger(*verbose)

	// Rendezvous + authentication before reporting anything.
	if _, err := cl.Ping(context.Background()); err != nil {
		cl.Close()
		return fmt.Errorf("handshake: %w", err)
	}
	tcp, err := cl.DialTCPPort(context.Background(), uint16(*tcpPort))
	if err != nil {
		cl.Close()
		return fmt.Errorf("dial tcp: %w", err)
	}
	udp, err := cl.DialUDPPort(context.Background(), uint16(*udpPort))
	if err != nil {
		tcp.Close()
		cl.Close()
		return fmt.Errorf("dial udp: %w", err)
	}
	// Transport heartbeat: establishes the server-side UDP flow (servers
	// can only reply to flows they have seen) and refreshes NAT bindings.
	// The peer worker drops it; it is never delivered to the parent.
	if _, err := udp.Write(heartbeatPayload); err != nil {
		tcp.Close()
		cl.Close()
		return fmt.Errorf("heartbeat udp: %w", err)
	}
	heartbeatDone := make(chan struct{})
	defer close(heartbeatDone)
	go func() {
		t := time.NewTicker(15 * time.Second)
		defer t.Stop()
		for {
			select {
			case <-heartbeatDone:
				return
			case <-t.C:
				_ = udp.SetWriteDeadline(time.Now().Add(2 * time.Second))
				if _, err := udp.Write(heartbeatPayload); err != nil {
					return
				}
			}
		}
	}()
	emit(map[string]string{"t": "peer", "state": "connected"})
	// Path report: direct when hole-punching succeeded, else relay.
	go reportPath(cl)

	bridge := &tcpBridge{got: make(chan struct{})}
	bridge.set(tcp)
	udpDst := &atomicUDPAddr{}
	udpDst.set(udp)
	go func() {
		defer tcp.Close()
		buf := make([]byte, 65536)
		for {
			n, err := tcp.Read(buf)
			if err != nil {
				return
			}
			if err := writeFrame(os.Stdout, fFromPeerTCP, buf[:n]); err != nil {
				return
			}
		}
	}()
	go func() {
		buf := make([]byte, maxUDP)
		for {
			n, err := udp.Read(buf)
			if err != nil {
				return
			}
			if err := writeFrame(os.Stdout, fFromPeerUDP, buf[:n]); err != nil {
				return
			}
		}
	}()
	defer cl.Close()
	return pumpParent(&parentDeps{tcp: bridge, udp: udpDst})
}

func reportPath(cl *tailcat.Client) {
	// Wait for NAT traversal to settle, then sample once.
	time.Sleep(4 * time.Second)
	res, err := cl.DiscoPing(context.Background())
	if err != nil {
		emit(map[string]string{"t": "path", "kind": "unknown"})
		return
	}
	if res.Endpoint != "" {
		emit(map[string]string{"t": "path", "kind": "direct", "endpoint": res.Endpoint})
	} else {
		emit(map[string]string{"t": "path", "kind": "relay"})
	}
}
