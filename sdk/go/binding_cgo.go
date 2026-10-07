// SPDX-License-Identifier: MIT
// Copyright (c) 2026 pith-hash

//go:build !windows && cgo

package pithh264

/*
#include <dlfcn.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

typedef int32_t (*pith_h264_decode_fn)(const uint8_t *, size_t, uint8_t **, size_t *);
typedef void (*pith_h264_free_fn)(uint8_t *, size_t);

static int32_t pith_h264_call_decode(void *fn, const uint8_t *data, size_t len,
                                     uint8_t **out, size_t *out_len) {
    return ((pith_h264_decode_fn)fn)(data, len, out, out_len);
}

static void pith_h264_call_free(void *fn, uint8_t *ptr, size_t len) {
    ((pith_h264_free_fn)fn)(ptr, len);
}
*/
import "C"

import (
	"fmt"
	"unsafe"
)

// ffiSymbols resolves both exported symbols of one open cdylib handle.
func ffiSymbols(handle unsafe.Pointer, libPath string) (decode, freeSym unsafe.Pointer, err error) {
	for _, name := range []string{"pith_h264_decode_planes", "pith_h264_free"} {
		cName := C.CString(name)
		sym := C.dlsym(handle, cName)
		C.free(unsafe.Pointer(cName))
		if sym == nil {
			return nil, nil, fmt.Errorf("pithh264: symbol %s missing from %s", name, libPath)
		}
		if name == "pith_h264_decode_planes" {
			decode = sym
		} else {
			freeSym = sym
		}
	}
	return decode, freeSym, nil
}

// openCdylib dlopens libPath with error text surfaced verbatim.
func openCdylib(libPath string) (unsafe.Pointer, error) {
	cPath := C.CString(libPath)
	defer C.free(unsafe.Pointer(cPath))
	handle := C.dlopen(cPath, C.RTLD_NOW|C.RTLD_LOCAL)
	if handle == nil {
		msg := "unknown dlopen failure"
		if e := C.dlerror(); e != nil {
			msg = C.GoString(e)
		}
		return nil, fmt.Errorf("pithh264: dlopen(%s): %s", libPath, msg)
	}
	return handle, nil
}

// ffiDecodePlanes opens the cdylib, resolves pith_h264_decode_planes
// and calls it. The handle is released before returning; repeated calls
// reuse the loader's own refcount.
func ffiDecodePlanes(libPath string, data *byte, n int, out **byte, outLen *uintptr) (int32, error) {
	handle, err := openCdylib(libPath)
	if err != nil {
		return 0, err
	}
	defer C.dlclose(handle)

	decodeSym, _, err := ffiSymbols(handle, libPath)
	if err != nil {
		return 0, err
	}
	var cOut *C.uint8_t
	var cLen C.size_t
	rc := C.pith_h264_call_decode(decodeSym, (*C.uint8_t)(unsafe.Pointer(data)), C.size_t(n), &cOut, &cLen)
	*out = (*byte)(unsafe.Pointer(cOut))
	*outLen = uintptr(cLen)
	return int32(rc), nil
}

// ffiFree releases a buffer handed out by ffiDecodePlanes. Null is
// accepted (the cdylib ignores it), matching the C contract.
func ffiFree(libPath string, ptr *byte, n uintptr) {
	handle, err := openCdylib(libPath)
	if err != nil {
		return // the library vanished mid-flight; nothing to free
	}
	defer C.dlclose(handle)
	if _, freeSym, err := ffiSymbols(handle, libPath); err == nil {
		C.pith_h264_call_free(freeSym, (*C.uint8_t)(unsafe.Pointer(ptr)), C.size_t(n))
	}
}
