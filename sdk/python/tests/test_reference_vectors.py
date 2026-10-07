# SPDX-License-Identifier: MIT
# Copyright (c) 2026 pith-hash
"""Hex-exact conformance: the committed reference vectors through ctypes.

Every vector in the repository-root ``reference.json`` is replayed
through the cdylib and compared byte-exact — the canonical stream's
planes tail (``raw[12:]``) hashed against ``planes_sha256``, plus every
recorded header fact (frames/width/height, big-endian). The same
vectors the Rust ``gen-reference verify`` gate and the Node/Go SDKs
check.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from pith_h264 import Canonical, FfiError, decode_planes, find_cdylib, parse_canonical

REPO_ROOT = Path(__file__).resolve().parents[3]


def test_cdylib_is_discoverable() -> None:
    path = find_cdylib()
    assert path.is_file(), path


@pytest.mark.parametrize(
    "vector",
    sorted(json.loads((REPO_ROOT / "reference.json").read_text(encoding="utf-8"))["vectors"], key=lambda v: v["name"]),
    ids=lambda vector: vector["name"],
)
def test_reference_vector_is_reproduced_hex_exact(vector: dict) -> None:
    data = (REPO_ROOT / vector["input_path"]).read_bytes()

    raw = decode_planes(data)
    # The digest contract: planes_sha256 covers the planes tail only
    # (bytes 12..), never the 12-byte header.
    assert hashlib.sha256(raw[12:]).hexdigest() == vector["planes_sha256"], vector["name"]

    canonical = parse_canonical(raw)
    assert canonical.frames == vector["frames"], vector["name"]
    assert canonical.width == vector["width"], vector["name"]
    assert canonical.height == vector["height"], vector["name"]


@pytest.mark.parametrize(
    "error",
    json.loads((REPO_ROOT / "reference.json").read_text(encoding="utf-8"))["errors"],
    ids=lambda error: error["name"],
)
def test_recorded_error_inputs_are_refused(error: dict) -> None:
    # Every decode() failure the contract records surfaces as
    # STATUS_REJECTED (-2) through the FFI, never a crash.
    with pytest.raises(FfiError) as err:
        decode_planes(bytes.fromhex(error["input_hex"]))
    assert err.value.status == -2, error["name"]


def test_malformed_input_is_refused_not_crashing() -> None:
    with pytest.raises(FfiError) as err:
        decode_planes(b"not an h264 stream at all")
    assert err.value.status == -2


def test_empty_input_is_refused() -> None:
    with pytest.raises(FfiError) as err:
        decode_planes(b"")
    assert err.value.status == -2


def test_planes_match_a_rust_pinned_value() -> None:
    # flat16's digest, pinned in the committed reference.json and
    # re-derived by the Rust unit tests; this test fails loudly even if
    # reference.json were regenerated wrongly. The 12 header bytes are
    # the recorded geometry: 1 frame, 16x16, big-endian.
    data = (REPO_ROOT / "tests" / "fixtures" / "flat16.h264").read_bytes()
    raw = decode_planes(data)
    assert hashlib.sha256(raw[12:]).hexdigest() == "524961aab71df0b48291ed8507c78051cde1dad5ec6e305fdd083e7667726519"
    assert raw[:12] == bytes([0, 0, 0, 1, 0, 0, 0, 16, 0, 0, 0, 16])

    canonical = parse_canonical(raw)
    assert isinstance(canonical, Canonical)
    assert (canonical.frames, canonical.width, canonical.height) == (1, 16, 16)
    # 16x16 luma + 2 * 8x8 chroma bytes for the single frame.
    assert len(canonical.planes) == 16 * 16 + 2 * 8 * 8
