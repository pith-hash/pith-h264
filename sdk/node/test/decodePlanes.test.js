// SPDX-License-Identifier: MIT
// Copyright (c) 2026 pith-hash
"use strict";

// Hex-exact conformance: the committed reference vectors through koffi.
// Every vector in the repository-root reference.json is replayed through
// the cdylib and compared byte-exact — the canonical stream's planes
// tail (raw[12:]) hashed against planes_sha256, plus every recorded
// header fact (frames/width/height, big-endian). The same vectors the
// Rust gen-reference verify gate and the Python/Go SDKs check.

const test = require("node:test");
const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const { FfiError, decodePlanes, findCdylib, parseCanonical } = require("../index.js");

const REPO_ROOT = path.resolve(__dirname, "..", "..", "..");

const REFERENCE = JSON.parse(fs.readFileSync(path.join(REPO_ROOT, "reference.json"), "utf8"));

test("cdylib is discoverable", () => {
  assert.ok(fs.statSync(findCdylib()).isFile());
});

for (const vector of REFERENCE.vectors) {
  test(`reference vector ${vector.name} is reproduced hex-exact`, () => {
    const data = fs.readFileSync(path.join(REPO_ROOT, vector.input_path));

    const raw = decodePlanes(data);
    // The digest contract: planes_sha256 covers the planes tail only
    // (bytes 12..), never the 12-byte header.
    assert.equal(crypto.createHash("sha256").update(raw.subarray(12)).digest("hex"), vector.planes_sha256, vector.name);

    const canonical = parseCanonical(raw);
    assert.equal(canonical.frames, vector.frames, vector.name);
    assert.equal(canonical.width, vector.width, vector.name);
    assert.equal(canonical.height, vector.height, vector.name);
  });
}

for (const error of REFERENCE.errors) {
  // Every decode() failure the contract records surfaces as
  // STATUS_REJECTED (-2) through the FFI, never a crash.
  test(`recorded error input ${error.name} is refused`, () => {
    assert.throws(() => decodePlanes(Buffer.from(error.input_hex, "hex")), (err) => {
      assert.ok(err instanceof FfiError);
      assert.equal(err.status, -2, error.name);
      return true;
    });
  });
}

test("malformed input is refused, not crashing", () => {
  assert.throws(() => decodePlanes(Buffer.from("not an h264 stream at all")), (err) => {
    assert.ok(err instanceof FfiError);
    assert.equal(err.status, -2);
    return true;
  });
});

test("empty input is refused", () => {
  assert.throws(() => decodePlanes(Buffer.alloc(0)), FfiError);
});

test("planes match a rust-pinned value", () => {
  // flat16's digest, pinned in the committed reference.json and
  // re-derived by the Rust unit tests; this test fails loudly even if
  // reference.json were regenerated wrongly. The 12 header bytes are
  // the recorded geometry: 1 frame, 16x16, big-endian.
  const data = fs.readFileSync(path.join(REPO_ROOT, "tests", "fixtures", "flat16.h264"));
  const raw = decodePlanes(data);
  assert.equal(
    crypto.createHash("sha256").update(raw.subarray(12)).digest("hex"),
    "524961aab71df0b48291ed8507c78051cde1dad5ec6e305fdd083e7667726519",
  );
  assert.deepEqual([...raw.subarray(0, 12)], [0, 0, 0, 1, 0, 0, 0, 16, 0, 0, 0, 16]);

  const canonical = parseCanonical(raw);
  assert.deepEqual(
    { frames: canonical.frames, width: canonical.width, height: canonical.height },
    { frames: 1, width: 16, height: 16 },
  );
  // 16x16 luma + 2 * 8x8 chroma bytes for the single frame.
  assert.equal(canonical.planes.length, 16 * 16 + 2 * 8 * 8);
});
