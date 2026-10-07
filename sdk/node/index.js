// SPDX-License-Identifier: MIT
// Copyright (c) 2026 pith-hash
"use strict";

/**
 * pith-h264 SDK: H.264/AVC (baseline-subset) decoding through koffi.
 *
 * The single Rust core (the `pith-h264` cdylib built by
 * `cargo build --release`) is loaded at runtime; koffi is the only
 * runtime dependency.
 *
 * Discovery order (the suite's cdylib convention):
 *
 *  1. `PITH_CDYLIB` — an explicit cdylib *file* path;
 *  2. `PITH_CDYLIB_DIR` — a *directory* scanned for the cdylib names
 *     (the CD pipeline points this at `target/release`);
 *  3. `prebuilds/` — the packaged npm layout the CD publish job
 *     assembles, flat and per `<os-arch>` (e.g. `linux-x64`);
 *  4. `<repo root>/target/release` — the repository working-tree
 *     layout, so a source checkout runs against a local cargo build
 *     with no configuration.
 *
 * The FFI surface is one decode operation plus one free:
 * `pith_h264_decode_planes` decodes a whole Annex-B byte stream into
 * the canonical byte stream the `reference.json` vectors are defined
 * over, and `pith_h264_free` releases the handed-out buffer.
 */

const koffi = require("koffi");
const fs = require("node:fs");
const path = require("node:path");

const STATUS_OK = 0;
const STATUS_INVALID = -1;
const STATUS_REJECTED = -2;

/** Every cdylib file name cargo may drop into the build directory, per platform. */
const CDYLIB_NAMES = ["pith_h264.dll", "libpith_h264.so", "libpith_h264.dylib"];

const PKG_ROOT = path.join(__dirname);
const REPO_ROOT = path.resolve(__dirname, "..", "..");

/** FfiError: a non-zero status code came back from the cdylib. */
class FfiError extends Error {
  /**
   * @param {string} op the FFI operation name
   * @param {number} status the raw status code
   */
  constructor(op, status) {
    const kind = { [STATUS_INVALID]: "invalid argument", [STATUS_REJECTED]: "input rejected" }[status] ?? "unknown failure";
    super(`${op} failed: ${kind} (status ${status})`);
    this.name = "FfiError";
    /** The raw status code the FFI returned. */
    this.status = status;
  }
}

/**
 * Locates the cdylib through the suite's discovery chain.
 * @returns {string} an absolute path to the cdylib file
 * @throws {Error} when nothing is found
 */
function findCdylib() {
  const explicit = process.env.PITH_CDYLIB;
  if (explicit && fs.statSync(explicit, { throwIfNoEntry: false })?.isFile()) {
    return path.resolve(explicit);
  }
  /** @type {string[]} */
  const dirs = [];
  const envDir = process.env.PITH_CDYLIB_DIR;
  if (envDir) {
    dirs.push(envDir);
    if (!path.isAbsolute(envDir)) {
      dirs.push(path.join(REPO_ROOT, envDir));
    }
  }
  const osArch = `${process.platform}-${process.arch}`;
  dirs.push(path.join(PKG_ROOT, "prebuilds", osArch));
  dirs.push(path.join(PKG_ROOT, "prebuilds"));
  dirs.push(path.join(REPO_ROOT, "target", "release"));
  for (const dir of dirs) {
    for (const name of CDYLIB_NAMES) {
      const p = path.join(dir, name);
      if (fs.statSync(p, { throwIfNoEntry: false })?.isFile()) return p;
    }
  }
  throw new Error(
    "no pith-h264 cdylib found (searched PITH_CDYLIB, PITH_CDYLIB_DIR, prebuilds/ and <repo>/target/release); " +
      "run `cargo build --release` first",
  );
}

let cached = undefined;

/**
 * Loads the cdylib and binds the exported symbols (lazily, once).
 * @returns {{decode: Function, free: Function}}
 */
function loadLibrary() {
  if (cached) return cached;
  const lib = koffi.load(findCdylib());
  const decode = lib.func("pith_h264_decode_planes", "int32_t", [
    "const uint8_t *",
    "size_t",
    koffi.out(koffi.pointer("void *")),
    koffi.out(koffi.pointer("size_t")),
  ]);
  const free = lib.func("void pith_h264_free(void *ptr, size_t len)");
  cached = { decode, free };
  return cached;
}

/**
 * Decodes a complete Annex-B H.264 byte stream into the canonical byte
 * stream the `reference.json` vectors are defined over. The handed-out
 * cdylib buffer is copied into a JS Buffer and released before
 * returning.
 *
 * @param {Buffer} data the complete Annex-B stream bytes
 * @returns {Buffer} the canonical stream (12-byte header + planes)
 * @throws {FfiError} with `status === -2` for any malformed input —
 *   bad magic, an unsupported profile or feature, a truncated NAL
 */
function decodePlanes(data) {
  if (!Buffer.isBuffer(data)) {
    throw new TypeError("data must be a Buffer");
  }
  const { decode, free } = loadLibrary();
  // An empty input is a decode refusal (the recorded empty-input error
  // vector), not a caller bug — koffi maps a zero-length Buffer to a
  // null pointer, which would trip the cdylib's null-argument guard
  // (-1) instead of reaching the decoder (-2). Hand over a valid
  // non-null address with the zero length instead.
  const buf = data.length > 0 ? data : Buffer.alloc(1);
  const out = [null];
  const outLen = [0];
  const status = decode(buf, data.length, out, outLen);
  if (status !== STATUS_OK) {
    throw new FfiError("pith_h264_decode_planes", status);
  }
  try {
    // koffi.decode hands back a Uint8Array view over the external
    // buffer; copy it into a Buffer before the cdylib buffer is freed.
    return Buffer.from(koffi.decode(out[0], "uint8_t", Number(outLen[0])));
  } finally {
    free(out[0], Number(outLen[0]));
  }
}

/**
 * Re-expresses the canonical byte stream as a plain object.
 *
 * The wire format is a 12-byte header, all fields big-endian: frames
 * u32, width u32, height u32, then the planes tail. `planes` is the
 * decoded pictures' `y || cb || cr` samples concatenated in
 * presentation order — exactly the bytes the `planes_sha256` digest in
 * reference.json covers (the digest covers the tail only, never the
 * header).
 *
 * @param {Buffer} raw the canonical stream
 * @returns {{frames: number, width: number, height: number, planes: Buffer, raw: Buffer}}
 */
function parseCanonical(raw) {
  if (!Buffer.isBuffer(raw) || raw.length < 12) {
    throw new TypeError("canonical stream is shorter than the 12-byte header");
  }
  return {
    frames: raw.readUInt32BE(0),
    width: raw.readUInt32BE(4),
    height: raw.readUInt32BE(8),
    planes: raw.subarray(12),
    raw,
  };
}

module.exports = {
  STATUS_OK,
  STATUS_INVALID,
  STATUS_REJECTED,
  CDYLIB_NAMES,
  FfiError,
  findCdylib,
  decodePlanes,
  parseCanonical,
};
