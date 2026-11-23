// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The capture contract between the sodiumwm plugin and remoted, as C wire structs:
// the canonical copy. remote/docs/capture-ipc.md is the specification. The plugin's
// compositor/plugin/src/remote/Protocol.hpp declares the same structs, enums and
// constants (a test in remote/host checks both, and remoted's Rust constants).
// Every struct is little-endian, fixed layout, with explicit padding.
#pragma once
/* Plain C, so any language can bind it; names carry no prefix because the plugin
   keeps them in a C++ namespace. */

#include <stddef.h>
#include <stdint.h>


// The highest version this plugin speaks. HELLO_ACK answers min(this, HELLO's), so an
// older remoted keeps working and never sees what it doesn't know.
#define VERSION (4)

enum eType {
    // remoted -> plugin
    HELLO       = 1,
    START       = 2,
    STOP        = 3,
    RELEASE     = 4,
    FRAME_CLOCK = 5,
    // 64..95: input, injected at the seat
    INPUT_POINTER_ABS = 64,
    INPUT_POINTER_REL = 65,
    INPUT_BUTTON      = 66,
    INPUT_AXIS        = 67,
    INPUT_KEY         = 68,
    INPUT_GESTURE     = 69,
    INPUT_RESET       = 70,
    LOCAL_INPUT       = 71, // v4

    // plugin -> remoted
    HELLO_ACK    = 129,
    BUFFERS      = 130,
    FRAME        = 131,
    CURSOR_SHAPE = 132,
    CURSOR_POS   = 133,
    HINT         = 134,
    STATS        = 135,
    STOPPED      = 136,
    CARET        = 137,
    POINTER_LOCK = 138,
};

typedef struct SHeader {
    uint32_t type;
    uint32_t length; // payload bytes after the header
} SHeader;

typedef struct SHello {
    uint32_t version;
} SHello;

#define START_FLAG_LINEAR (1u << 0) // prefer LINEAR (tests); BUFFERS.modifier says what was given

typedef struct SStart {
    char     output[32]; // NUL-padded
    uint32_t ringSize;
    uint32_t flags;
} SStart;

typedef struct SRelease {
    uint32_t generation;
    uint32_t bufferId;
} SRelease;

typedef struct SFrameClock {
    int64_t  phaseOffsetNs; // frames finish at t with (t - phaseOffsetNs) % period == 0, CLOCK_MONOTONIC; 0 = natural schedule
    uint32_t refreshMhz;    // the period; 0 = the output's own
    uint32_t graceNs;       // after idle: a frame at most this late still renders now, for the slot it just missed (v3)
} SFrameClock;

// ---- input (remoted -> plugin). tNs: when it happened, CLOCK_MONOTONIC on
// this host (remoted maps the client's clock); 0, or a time in the future or
// more than a second old, means now.

typedef struct SInputPointerAbs {
    double   x, y; // buffer pixels of the captured output, top-left origin
    uint64_t tNs;
} SInputPointerAbs;

typedef struct SInputPointerRel { // under pointer lock; logical pixels, as libinput reports a mouse
    double   dx, dy;
    double   dxUnaccel, dyUnaccel;
    uint64_t tNs;
} SInputPointerRel;

enum eButtonState {
    STATE_UP   = 0,
    STATE_DOWN = 1,
};

typedef struct SInputButton {
    uint32_t button; // evdev BTN_*
    uint32_t state;  // eButtonState
    uint64_t tNs;
} SInputButton;

enum eAxisSource {
    AXIS_WHEEL       = 0,
    AXIS_FINGER      = 1,
    AXIS_CONTINUOUS  = 2,
    AXIS_WHEEL_TILT  = 3,
};

#define AXIS_FLAG_STOP (1u << 0) // the end of a finger/continuous scroll on this axis (value is ignored)
#define AXIS_FLAG_MORE (1u << 1) // the next INPUT_AXIS belongs to the same wl_pointer frame (both axes of one event)

typedef struct SInputAxis {
    uint32_t source; // eAxisSource
    uint32_t axis;   // 0 vertical, 1 horizontal
    double   value;  // logical pixels (wl_pointer.axis); 0 on a wheel: value120 / 8
    int32_t  value120;
    uint32_t flags;
    uint64_t tNs;
} SInputAxis;

// LOCAL_INPUT (v4): the desk's own keyboards and pointers, for the privacy screen
// Back to LOCAL_NORMAL when the connection closes.
enum eLocalInput {
    LOCAL_NORMAL  = 0, // the desk's input works
    LOCAL_IGNORED = 1, // every keyboard and pointer but the plugin's own (and the input method's) is ignored
    LOCAL_PRIVACY = 2, // as 1, but desk keys go to the privacy screen layer (namespace PRIVACY_LAYER)
};

#define PRIVACY_LAYER "sodiumshell-privacy"

typedef struct SLocalInput {
    uint32_t mode; // eLocalInput
    uint32_t pad;
} SLocalInput;

typedef struct SInputKey {
    uint32_t keycode; // evdev KEY_*
    uint32_t state;   // eButtonState
    uint64_t tNs;
} SInputKey;

enum eGestureKind {
    GESTURE_SWIPE = 1,
    GESTURE_PINCH = 2,
    GESTURE_HOLD  = 3,
};

enum eGesturePhase {
    PHASE_BEGIN  = 0,
    PHASE_UPDATE = 1,
    PHASE_END    = 2,
    PHASE_CANCEL = 3,
};

typedef struct SInputGesture {
    uint32_t kind;  // eGestureKind
    uint32_t phase; // eGesturePhase
    uint32_t fingers;
    uint32_t pad;
    double   dx, dy;   // update: finger motion since the last update, libinput touchpad units (about logical pixels)
    double   scale;    // pinch update: relative to the begin (1.0 = unchanged)
    double   rotation; // pinch update: degrees clockwise since the last update
    uint64_t tNs;
} SInputGesture;

typedef struct SHelloAck {
    uint32_t version;
    char     hyprland[32];
} SHelloAck;

typedef struct SBuffers {
    uint32_t generation;
    uint32_t count;
    uint32_t width;
    uint32_t height;
    uint32_t drmFormat;
    uint32_t nPlanes;
    uint64_t modifier;
    // then nPlanes x SPlane
} SBuffers;

typedef struct SPlane {
    uint32_t offset;
    uint32_t stride;
} SPlane;

#define FRAME_FLAG_FENCE (1u << 0)

typedef struct SFrame {
    uint32_t generation;
    uint32_t bufferId;
    uint64_t seq;
    uint64_t tRenderedNs;
    uint32_t nDamage;
    uint32_t flags;
    // then nDamage x SRect
} SFrame;

typedef struct SRect {
    int32_t x, y, w, h;
} SRect;

typedef struct SCursorShape {
    uint64_t hash;
    uint32_t width;
    uint32_t height;
    int32_t  hotX;
    int32_t  hotY;
} SCursorShape;

typedef struct SCursorPos {
    int32_t  x; // the hotspot, buffer pixels
    int32_t  y;
    uint32_t visible;
    uint32_t pad;
    uint64_t tNs;
} SCursorPos;

typedef struct SStats {
    uint64_t frames;
    uint64_t droppedNoBuffer;
    uint64_t droppedNoClient;
} SStats;

enum eStopReason {
    STOP_NO_OUTPUT      = 1, // START named an output that doesn't exist
    STOP_OUTPUT_REMOVED = 2, // the output went away while capturing
    STOP_ALLOC_FAILED   = 3, // no exportable buffers (GBM/EGL)
    STOP_UNSUPPORTED    = 4, // not the GL renderer, a transformed output, a second START
};

typedef struct SStopped {
    uint32_t reason;
    uint32_t pad;
} SStopped;

enum eHintKind {
    HINT_ANIM_START  = 1, // rect: what animates so far; detail: eHintSource bits; seq: the first FRAME that shows it
    HINT_ANIM_SETTLE = 2, // rect: everything the animation touched; detail: eHintSource bits seen; seq: the FRAME with its last state
    HINT_WINDOW_MOVE = 3, // motion: rect is where the content is in FRAME seq, (dx, dy) how far it moved since FRAME seq - 1
    HINT_SCROLL      = 4, // motion, best effort: the injected scroll under the pointer (content moves by -value)
};

enum eHintSource {
    SOURCE_WINDOW    = 1u << 0, // a window opens, closes, moves, resizes or fades (Hyprland's window animations)
    SOURCE_WORKSPACE = 1u << 1, // a space slides or fades (workspaces, special workspaces)
    SOURCE_LAYER     = 1u << 2, // a layer surface (bar, dock, launcher, notifications) animates in or out
    SOURCE_TRANSFORM = 1u << 3, // a window drawn through a transformer moves (sodiumwm's overview, wobble)
    SOURCE_EXTERNAL  = 1u << 4, // announced through hl.plugin.sodiumwm.remote_animation() (the shell's own animations)
};

// HINT. Motion hints (kinds 3, 4) come in a batch right before the FRAME they
// describe: detail is the index in the batch, and index 0 starts a new batch
// that replaces any earlier one. A motion hint with an empty rect carries nothing.
typedef struct SHint {
    uint32_t kind;   // eHintKind
    uint32_t detail; // kinds 1, 2: eHintSource bits; kinds 3, 4: index in the batch
    int32_t  x, y, w, h; // buffer pixels, clipped to the buffer
    int32_t  dx, dy;     // kinds 3, 4: buffer pixels
    uint64_t seq;        // the FRAME it belongs to
} SHint;

typedef struct SCaret {
    uint32_t visible; // 0: no focused text input with a caret on the captured output
    uint32_t purpose; // text-input-v3 content purpose (8 password, 9 pin)
    int32_t  x, y, w, h; // buffer pixels (w is often 0)
    uint64_t tNs;
} SCaret;

enum ePointerLock {
    LOCK_NONE     = 0,
    LOCK_LOCKED   = 1, // the pointer doesn't move: send INPUT_POINTER_REL
    LOCK_CONFINED = 2, // the pointer stays inside the rect
};

typedef struct SPointerLock {
    uint32_t state; // ePointerLock
    uint32_t pad;
    int32_t  x, y, w, h; // locked: the surface; confined: the region's bounds; buffer pixels
} SPointerLock;

_Static_assert(sizeof(SHeader) == 8, "capture-ipc layout");
_Static_assert(sizeof(SStart) == 40, "capture-ipc layout");
_Static_assert(sizeof(SFrameClock) == 16, "capture-ipc layout");
_Static_assert(sizeof(SHelloAck) == 36, "capture-ipc layout");
_Static_assert(sizeof(SBuffers) == 32, "capture-ipc layout");
_Static_assert(offsetof(SBuffers, modifier) == 24, "capture-ipc layout");
_Static_assert(sizeof(SFrame) == 32, "capture-ipc layout");
_Static_assert(sizeof(SCursorShape) == 24, "capture-ipc layout");
_Static_assert(sizeof(SCursorPos) == 24, "capture-ipc layout");
_Static_assert(sizeof(SStats) == 24, "capture-ipc layout");
_Static_assert(sizeof(SStopped) == 8, "capture-ipc layout");
_Static_assert(sizeof(SInputPointerAbs) == 24, "capture-ipc layout");
_Static_assert(sizeof(SInputPointerRel) == 40, "capture-ipc layout");
_Static_assert(sizeof(SInputButton) == 16, "capture-ipc layout");
_Static_assert(sizeof(SInputAxis) == 32, "capture-ipc layout");
_Static_assert(offsetof(SInputAxis, value) == 8, "capture-ipc layout");
_Static_assert(offsetof(SInputAxis, tNs) == 24, "capture-ipc layout");
_Static_assert(sizeof(SInputKey) == 16, "capture-ipc layout");
_Static_assert(sizeof(SInputGesture) == 56, "capture-ipc layout");
_Static_assert(offsetof(SInputGesture, dx) == 16, "capture-ipc layout");
_Static_assert(sizeof(SHint) == 40, "capture-ipc layout");
_Static_assert(offsetof(SHint, seq) == 32, "capture-ipc layout");
_Static_assert(sizeof(SCaret) == 32, "capture-ipc layout");
_Static_assert(offsetof(SCaret, tNs) == 24, "capture-ipc layout");
_Static_assert(sizeof(SPointerLock) == 24, "capture-ipc layout");
_Static_assert(sizeof(SLocalInput) == 8, "capture-ipc layout");

// Damage beyond this many rects goes as its bounding box.
#define MAX_DAMAGE_RECTS (32)
#define MAX_RING (8)
