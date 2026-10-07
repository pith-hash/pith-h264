# SPDX-License-Identifier: MIT
# Copyright (c) 2026 pith-hash
"""pith-h264 SDK: H.264/AVC (baseline-subset) decoding through ctypes.

The single Rust core (the ``pith-h264`` cdylib built by
``cargo build --release``) is loaded at runtime; this package carries
no third-party dependency — ``ctypes`` is the standard library.

Discovery order (the suite's cdylib convention):

1. ``PITH_CDYLIB`` — an explicit cdylib *file* path;
2. ``PITH_CDYLIB_DIR`` — a *directory* scanned for the cdylib names
   (the CD pipeline points this at ``target/release``);
3. the package directory itself (the built wheel ships the cdylib as
   package data);
4. ``<repo root>/target/release`` — the repository working-tree layout,
   so a source checkout runs against a local cargo build with no
   configuration.

The FFI surface is one decode operation plus one free:
``pith_h264_decode_planes`` decodes a whole Annex-B byte stream into
the canonical byte stream the ``reference.json`` vectors are defined
over, and ``pith_h264_free`` releases the handed-out buffer.
"""

from __future__ import annotations

import ctypes
import os
from dataclasses import dataclass
from pathlib import Path

__all__ = [
    "CDYLIB_NAMES",
    "Canonical",
    "FfiError",
    "LibraryNotFoundError",
    "STATUS_INVALID",
    "STATUS_OK",
    "STATUS_REJECTED",
    "decode_planes",
    "find_cdylib",
    "parse_canonical",
]

#: Status: success.
STATUS_OK = 0
#: Status: a caller argument is invalid (a null pointer).
STATUS_INVALID = -1
#: Status: the core decoder refused the input (malformed stream).
STATUS_REJECTED = -2

#: Every cdylib file name cargo may drop into the build directory, per
#: platform (windows / linux / macOS).
CDYLIB_NAMES = ("pith_h264.dll", "libpith_h264.so", "libpith_h264.dylib")


@dataclass(frozen=True)
class Canonical:
    """The decoded stream, re-expressed from the canonical byte stream.

    The wire format is a 12-byte header, all fields big-endian:
    ``frames`` as ``u32``, ``width`` as ``u32``, ``height`` as
    ``u32``, then the planes tail. ``planes`` is the decoded pictures'
    ``y || cb || cr`` samples concatenated in presentation order
    (``width*height`` luma and ``(width/2)*(height/2)`` chroma bytes
    per frame) — exactly the bytes the ``planes_sha256`` digest in
    ``reference.json`` covers (the digest covers the tail only, never
    the header).
    """

    #: Number of decoded pictures in presentation order.
    frames: int
    #: Displayed width in luma samples.
    width: int
    #: Displayed height in luma samples.
    height: int
    #: The canonical byte stream the header was parsed from.
    raw: bytes

    @property
    def planes(self) -> bytes:
        """The decoded planes (everything after the 12-byte header)."""
        return self.raw[12:]


class LibraryNotFoundError(OSError):
    """No cdylib was found through the discovery chain."""


class FfiError(Exception):
    """A non-zero status code came back from the cdylib."""

    def __init__(self, op: str, status: int) -> None:
        kind = {
            STATUS_INVALID: "invalid argument",
            STATUS_REJECTED: "input rejected",
        }.get(status, "unknown failure")
        super().__init__(f"{op} failed: {kind} (status {status})")
        #: The raw status code the FFI returned.
        self.status = status


def find_cdylib() -> Path:
    """Locates the cdylib through the suite's discovery chain."""
    explicit = os.environ.get("PITH_CDYLIB")
    if explicit:
        p = Path(explicit)
        if p.is_file():
            return p
    env_dir = os.environ.get("PITH_CDYLIB_DIR")
    candidates: list[Path] = []
    if env_dir:
        env_dir_path = Path(env_dir)
        candidates.append(env_dir_path)
        if not env_dir_path.is_absolute():
            # CD and local runs invoke tools from the repository root or
            # from sdk/<lang>; resolve the env value against both.
            candidates.append(Path.cwd() / env_dir_path)
            candidates.append(Path(__file__).resolve().parents[3] / env_dir_path)
    candidates.append(Path(__file__).resolve().parent)  # packaged wheel
    candidates.append(Path(__file__).resolve().parents[3] / "target" / "release")
    for directory in candidates:
        for name in CDYLIB_NAMES:
            p = directory / name
            if p.is_file():
                return p
    raise LibraryNotFoundError(
        "no pith-h264 cdylib found (searched PITH_CDYLIB, PITH_CDYLIB_DIR, "
        "the package directory and <repo>/target/release); "
        "run `cargo build --release` first"
    )


_lib: ctypes.CDLL | None = None


def _load() -> ctypes.CDLL:
    global _lib
    if _lib is None:
        lib = ctypes.CDLL(str(find_cdylib()))
        lib.pith_h264_decode_planes.argtypes = [
            ctypes.c_void_p,  # data
            ctypes.c_size_t,  # len
            ctypes.POINTER(ctypes.c_void_p),  # out buffer
            ctypes.POINTER(ctypes.c_size_t),  # out length
        ]
        lib.pith_h264_decode_planes.restype = ctypes.c_int32
        lib.pith_h264_free.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        lib.pith_h264_free.restype = None
        _lib = lib
    return _lib


def decode_planes(data: bytes) -> bytes:
    """Decodes a complete Annex-B H.264 byte stream into the canonical
    byte stream the ``reference.json`` vectors are defined over.

    Raises :class:`FfiError` with ``status == STATUS_REJECTED`` for any
    malformed input — bad magic, an unsupported profile or feature, a
    truncated NAL; the decoder never panics through this boundary.
    """
    out = ctypes.c_void_p()
    out_len = ctypes.c_size_t()
    status = _load().pith_h264_decode_planes(data, len(data), ctypes.byref(out), ctypes.byref(out_len))
    if status != STATUS_OK:
        raise FfiError("pith_h264_decode_planes", status)
    try:
        return ctypes.string_at(out, out_len.value)
    finally:
        _load().pith_h264_free(out, out_len.value)


def parse_canonical(raw: bytes) -> Canonical:
    """Re-expresses the canonical byte stream as a :class:`Canonical`."""
    if len(raw) < 12:
        raise ValueError("canonical stream is shorter than the 12-byte header")
    return Canonical(
        frames=int.from_bytes(raw[0:4], "big"),
        width=int.from_bytes(raw[4:8], "big"),
        height=int.from_bytes(raw[8:12], "big"),
        raw=raw,
    )
