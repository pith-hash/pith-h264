// SPDX-License-Identifier: MIT
// Copyright (c) 2026 pith-hash

// Package pithh264 provides Go bindings for the pith-h264 Rust cdylib:
// H.264/AVC (baseline-subset) decoding into the canonical vector stream.
//
// The single Rust core (built by `cargo build --release`) is loaded at
// runtime; the package carries zero module dependencies. On unix the
// cdylib is opened with dlopen through cgo, on Windows with
// LoadLibrary through the standard syscall package — both resolve the
// library through the same discovery chain, so `go build ./... &&
// go test ./...` works unchanged on every OS the CD matrix builds.
//
// Discovery order (the suite's cdylib convention):
//
//  1. PITH_CDYLIB — an explicit cdylib file path;
//  2. PITH_CDYLIB_DIR — a directory scanned for the cdylib names (the
//     CD pipeline points this at target/release);
//  3. <repo root>/target/release — the repository working-tree layout,
//     anchored at this package's source directory, so a source
//     checkout runs against a local cargo build unconfigured.
//
// The FFI surface is one decode operation plus one free:
// pith_h264_decode_planes decodes a whole Annex-B byte stream into the
// canonical byte stream the reference.json vectors are defined over,
// and pith_h264_free releases the handed-out buffer.
package pithh264

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"unsafe"
)

// Status codes returned by the cdylib's C ABI.
const (
	// StatusOK: success.
	StatusOK int32 = 0
	// StatusInvalid: a caller argument is invalid (a null pointer).
	StatusInvalid int32 = -1
	// StatusRejected: the core decoder refused the input (malformed
	// Annex-B stream: bad magic, unsupported profile or feature,
	// truncated NAL).
	StatusRejected int32 = -2
)

// cdylibNames are the file names cargo may drop into the build
// directory, per platform (windows / linux / macOS).
var cdylibNames = []string{"pith_h264.dll", "libpith_h264.so", "libpith_h264.dylib"}

// FfiError reports a non-zero status code from the cdylib.
type FfiError struct {
	// Op is the FFI operation name.
	Op string
	// Status is the raw status code the FFI returned.
	Status int32
}

func (e *FfiError) Error() string {
	kind := "unknown failure"
	switch e.Status {
	case StatusInvalid:
		kind = "invalid argument"
	case StatusRejected:
		kind = "input rejected"
	}
	return fmt.Sprintf("%s failed: %s (status %d)", e.Op, kind, e.Status)
}

// FindCdylib locates the cdylib through the suite's discovery chain.
func FindCdylib() (string, error) {
	if p := os.Getenv("PITH_CDYLIB"); p != "" {
		if st, err := os.Stat(p); err == nil && st.Mode().IsRegular() {
			return filepath.Abs(p)
		}
	}
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		return "", fmt.Errorf("pithh264: cannot locate the package source directory")
	}
	pkgDir := filepath.Dir(thisFile)
	repoRoot := filepath.Dir(filepath.Dir(pkgDir)) // sdk/go -> sdk -> repo root

	var dirs []string
	if env := os.Getenv("PITH_CDYLIB_DIR"); env != "" {
		dirs = append(dirs, env)
		if !filepath.IsAbs(env) {
			dirs = append(dirs, filepath.Join(repoRoot, env))
		}
	}
	dirs = append(dirs, filepath.Join(repoRoot, "target", "release"))
	for _, dir := range dirs {
		for _, name := range cdylibNames {
			p := filepath.Join(dir, name)
			if st, err := os.Stat(p); err == nil && st.Mode().IsRegular() {
				return p, nil
			}
		}
	}
	return "", fmt.Errorf(
		"pithh264: no cdylib found (searched PITH_CDYLIB, PITH_CDYLIB_DIR and <repo>/target/release); run `cargo build --release` first",
	)
}

// locate resolves the cdylib path once per process.
var locate = sync.OnceValues(FindCdylib)

// Canonical is the decoded stream, re-expressed from the canonical
// byte stream. The wire format is a 12-byte header, all fields
// big-endian: frames u32, width u32, height u32, then the planes tail.
type Canonical struct {
	// Frames is the number of decoded pictures in presentation order.
	Frames uint32
	// Width is the displayed width in luma samples.
	Width uint32
	// Height is the displayed height in luma samples.
	Height uint32
	// Planes is the decoded pictures' y || cb || cr samples
	// concatenated in presentation order (width*height luma and
	// (width/2)*(height/2) chroma bytes per frame) — exactly the bytes
	// the planes_sha256 digest in reference.json covers (the digest
	// covers the tail only, never the header).
	Planes []byte
}

// DecodePlanes decodes a complete Annex-B H.264 byte stream into the
// canonical byte stream the reference.json vectors are defined over.
// The returned slice is a Go copy; the handed-out cdylib buffer is
// released before returning.
func DecodePlanes(data []byte) ([]byte, error) {
	libPath, err := locate()
	if err != nil {
		return nil, err
	}
	// An empty input is a decode refusal (the recorded empty-input
	// error vector), not a caller bug — hand the cdylib a valid
	// non-null address so the zero length reaches the decoder instead
	// of tripping the null-pointer argument guard.
	var dataPtr *byte
	if len(data) > 0 {
		dataPtr = &data[0]
	} else {
		dataPtr = new(byte)
	}
	var out *byte
	var outLen uintptr
	status, err := ffiDecodePlanes(libPath, dataPtr, len(data), &out, &outLen)
	if err != nil {
		return nil, err
	}
	if status != StatusOK {
		return nil, &FfiError{Op: "pith_h264_decode_planes", Status: status}
	}
	buf := make([]byte, outLen)
	copy(buf, unsafe.Slice(out, outLen))
	ffiFree(libPath, out, outLen)
	return buf, nil
}

// ParseCanonical re-expresses the canonical byte stream as a Canonical.
func ParseCanonical(raw []byte) (Canonical, error) {
	if len(raw) < 12 {
		return Canonical{}, fmt.Errorf("pithh264: canonical stream is shorter than the 12-byte header")
	}
	return Canonical{
		Frames: uint32(raw[0])<<24 | uint32(raw[1])<<16 | uint32(raw[2])<<8 | uint32(raw[3]),
		Width:  uint32(raw[4])<<24 | uint32(raw[5])<<16 | uint32(raw[6])<<8 | uint32(raw[7]),
		Height: uint32(raw[8])<<24 | uint32(raw[9])<<16 | uint32(raw[10])<<8 | uint32(raw[11]),
		Planes: raw[12:],
	}, nil
}
