package main

import (
	"bytes"
	"testing"
)

func TestFrameRoundtrip(t *testing.T) {
	var buf bytes.Buffer
	payloads := [][]byte{
		[]byte("hello"),
		make([]byte, 1232),
	}
	types := []byte{fToPeerTCP, fToPeerUDP}
	for i, p := range payloads {
		if err := writeFrame(&buf, types[i], p); err != nil {
			t.Fatalf("write: %v", err)
		}
	}
	fr := &frameReader{r: &buf}
	for i, p := range payloads {
		typ, body, err := fr.next()
		if err != nil {
			t.Fatalf("read: %v", err)
		}
		if typ != types[i] || !bytes.Equal(body, p) {
			t.Fatalf("mismatch %d: typ=%#x len=%d", i, typ, len(body))
		}
	}
}

func TestFrameRejectsOversize(t *testing.T) {
	if err := writeFrame(&bytes.Buffer{}, fToPeerUDP, make([]byte, maxFrame)); err == nil {
		t.Fatal("oversize aceito")
	}
	var buf bytes.Buffer
	if err := writeFrame(&buf, 0x99, []byte("x")); err != nil {
		t.Fatalf("tipo opaco deveria passar no framing: %v", err)
	}
	typ, body, err := (&frameReader{r: &buf}).next()
	if err != nil || typ != 0x99 || string(body) != "x" {
		t.Fatalf("frame opaco: %v %#x %q", err, typ, body)
	}
}

func TestFrameRejectsTruncated(t *testing.T) {
	fr := &frameReader{r: bytes.NewReader([]byte{0, 0, 0, 10, fToPeerTCP, 1, 2})}
	if _, _, err := fr.next(); err == nil {
		t.Fatal("truncado aceito")
	}
}
