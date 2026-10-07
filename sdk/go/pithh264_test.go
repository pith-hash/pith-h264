// SPDX-License-Identifier: MIT
// Copyright (c) 2026 pith-hash

package pithh264

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

// repoRoot resolves the repository root relative to this package
// (sdk/go -> sdk -> repo root), the anchor for reference.json and the
// committed fixtures.
func repoRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.Abs(filepath.Join("..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	if st, err := os.Stat(filepath.Join(root, "reference.json")); err != nil || st.IsDir() {
		t.Fatalf("reference.json not found at %s", root)
	}
	return root
}

// committedReference parses the committed reference.json: one record
// per conformance fixture plus the recorded decode refusals.
func committedReference(t *testing.T) (vectors []struct {
	Name      string `json:"name"`
	InputPath string `json:"input_path"`
	Frames    uint32 `json:"frames"`
	Width     uint32 `json:"width"`
	Height    uint32 `json:"height"`
	PlanesSha string `json:"planes_sha256"`
},
	errors []struct {
		Name      string `json:"name"`
		InputHex  string `json:"input_hex"`
		ErrorKind string `json:"error_kind"`
	}) {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join(repoRoot(t), "reference.json"))
	if err != nil {
		t.Fatal(err)
	}
	var parsed struct {
		Vectors []struct {
			Name      string `json:"name"`
			InputPath string `json:"input_path"`
			Frames    uint32 `json:"frames"`
			Width     uint32 `json:"width"`
			Height    uint32 `json:"height"`
			PlanesSha string `json:"planes_sha256"`
		} `json:"vectors"`
		Errors []struct {
			Name      string `json:"name"`
			InputHex  string `json:"input_hex"`
			ErrorKind string `json:"error_kind"`
		} `json:"errors"`
	}
	if err := json.Unmarshal(raw, &parsed); err != nil {
		t.Fatal(err)
	}
	return parsed.Vectors, parsed.Errors
}

// TestReferenceVectorsHexExact replays every committed reference.json
// vector through the cdylib and compares byte-exact: the canonical
// stream's planes tail (raw[12:]) hashed against planes_sha256, plus
// every recorded header fact (frames/width/height, big-endian) — the
// same vectors the Rust gen-reference verify gate and the Python/Node
// SDKs check.
func TestReferenceVectorsHexExact(t *testing.T) {
	vectors, _ := committedReference(t)
	if len(vectors) == 0 {
		t.Fatal("reference.json carries no vectors")
	}
	for _, want := range vectors {
		t.Run(want.Name, func(t *testing.T) {
			data, err := os.ReadFile(filepath.Join(repoRoot(t), filepath.FromSlash(want.InputPath)))
			if err != nil {
				t.Fatal(err)
			}
			raw, err := DecodePlanes(data)
			if err != nil {
				t.Fatalf("DecodePlanes(%s): %v", want.Name, err)
			}
			// The digest contract: planes_sha256 covers the planes
			// tail only (bytes 12..), never the 12-byte header.
			digest := sha256.Sum256(raw[12:])
			if got := hex.EncodeToString(digest[:]); got != want.PlanesSha {
				t.Errorf("%s: planes digest %s, want %s", want.Name, got, want.PlanesSha)
			}
			canonical, err := ParseCanonical(raw)
			if err != nil {
				t.Fatal(err)
			}
			if canonical.Frames != want.Frames || canonical.Width != want.Width || canonical.Height != want.Height {
				t.Errorf("%s: header %d frames %dx%d, want %d frames %dx%d",
					want.Name, canonical.Frames, canonical.Width, canonical.Height,
					want.Frames, want.Width, want.Height)
			}
		})
	}
}

// TestFlat16PinnedDigest pins one digest the Rust unit tests re-derive,
// so the binding fails loudly even if reference.json were regenerated
// wrongly. The 12 header bytes are the recorded geometry: 1 frame,
// 16x16, big-endian.
func TestFlat16PinnedDigest(t *testing.T) {
	data, err := os.ReadFile(filepath.Join(repoRoot(t), "tests", "fixtures", "flat16.h264"))
	if err != nil {
		t.Fatal(err)
	}
	raw, err := DecodePlanes(data)
	if err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256(raw[12:])
	const want = "524961aab71df0b48291ed8507c78051cde1dad5ec6e305fdd083e7667726519"
	if got := hex.EncodeToString(digest[:]); got != want {
		t.Errorf("flat16: planes digest %s, want %s", got, want)
	}
	wantHeader := []byte{0, 0, 0, 1, 0, 0, 0, 16, 0, 0, 0, 16}
	for i, b := range wantHeader {
		if raw[i] != b {
			t.Fatalf("flat16: header byte %d = %d, want %d", i, raw[i], b)
		}
	}
}

// TestRecordedErrorInputsAreRefused checks every refusal the contract
// records: a status code, never a crash.
func TestRecordedErrorInputsAreRefused(t *testing.T) {
	_, errors := committedReference(t)
	if len(errors) == 0 {
		t.Fatal("reference.json carries no error vectors")
	}
	for _, want := range errors {
		t.Run(want.Name, func(t *testing.T) {
			input, err := hex.DecodeString(want.InputHex)
			if err != nil {
				t.Fatal(err)
			}
			_, err = DecodePlanes(input)
			var ffi *FfiError
			if e, ok := err.(*FfiError); ok {
				ffi = e
			} else {
				t.Fatalf("%s: want FfiError, got %v", want.Name, err)
			}
			if ffi.Status != StatusRejected {
				t.Errorf("%s: want StatusRejected, got %d", want.Name, ffi.Status)
			}
		})
	}
}

// TestMalformedInputIsRefused checks the decoder's refusal path: a
// status code, never a crash.
func TestMalformedInputIsRefused(t *testing.T) {
	_, err := DecodePlanes([]byte("not an h264 stream at all"))
	var ffi *FfiError
	if e, ok := err.(*FfiError); ok {
		ffi = e
	} else {
		t.Fatalf("want FfiError, got %v", err)
	}
	if ffi.Status != StatusRejected {
		t.Errorf("want StatusRejected, got %d", ffi.Status)
	}
}
