// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The C ABI of remote/client-qt (crate remote-qt, libremote_qt.a). Events come back as JSON text through one
// callback, from engine threads. See remote/client-qt/src/lib.rs and remote/docs/reference/client-app.md.
#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct RqHandle RqHandle;
typedef void (*rq_event_fn)(void *user, const char *json);

RqHandle *rq_new(const char *data_dir, const char *device_name, int rendezvous, rq_event_fn cb, void *user);
void rq_free(RqHandle *);
void rq_refresh(RqHandle *);
void rq_pair(RqHandle *, const char *host_id, const char *code, const char *addr);
void rq_resolve_address(RqHandle *, const char *text);
void rq_forget(RqHandle *, const char *host_id);
char *rq_decode_support(void);
// The client data folder the engine uses by default ($SODIUM_REMOTE_DATA or the standard place, after the first-start migration). Free with rq_free_string.
char *rq_default_data_dir(void);
// The server (remote/client-qt/src/lib.rs): all return JSON text, free with rq_free_string.
char *rq_server_info(void);
char *rq_server_link_parse(const char *link);
char *rq_server_check(const char *address, const char *ca_pem, const char *token);
char *rq_server_file_parse(const char *path);
char *rq_server_import(const char *link);
char *rq_server_remove(void);
char *rq_server_export(const char *path);
// 1: the browse client was started again (reads server.toml anew); 0: a session runs, so it applies when the session ends.
int rq_restart_browse(RqHandle *);
void rq_free_string(char *);
int rq_start(RqHandle *, const char *opts_json, void *wl_display, void *wl_surface);
void rq_stop(RqHandle *);
void rq_view(RqHandle *, int x, int y, int w, int h, int scale, int fullscreen);
void rq_hud_rects(RqHandle *, const int *rects, int n);
void rq_capture(RqHandle *, int on);
void rq_menu_block(RqHandle *, int on);
void rq_select_display(RqHandle *, uint32_t id);
void rq_key_passthrough(RqHandle *, int on);
void rq_send_cad(RqHandle *);
char *rq_stats(RqHandle *);

// "This computer" on Windows, and on generic Linux with remote-qt's `linux-host` feature (remote/client-qt/src/host.rs):
// host-desktop's Session in this process. host_status events carry client.view_only (GNOME without
// "Allow Remote Interaction").
typedef struct RhHandle RhHandle;
RhHandle *rh_new(const char *data_dir, rq_event_fn cb, void *user);
void rh_free(RhHandle *);
void rh_enable(RhHandle *, int on, const char *name);
void rh_open_pairing(RhHandle *);
void rh_close_pairing(RhHandle *);
void rh_disconnect(RhHandle *);
void rh_set_name(RhHandle *, const char *name);
void rh_revoke(RhHandle *, const char *id);
char *rh_paired(RhHandle *);
char *rh_self(RhHandle *);
// Linux: the desktop's sharing permission (the portal's dialog and its restore token). Events, and what rh_consent
// returns (null on other OSes):
//   {"ev":"host_consent","state":S,"detail":"..."}
//   S = "ask"          no saved permission: the next connection shows the desktop's dialog
//       "asking"       the dialog is up
//       "remembered"   a saved permission: devices connect without anyone at the computer
//       "declined"     the last dialog was cancelled (the device was told "ended at the computer")
//       "unavailable"  no portal with RemoteDesktop and ScreenCast; detail says why
// rh_forget_consent forgets a remembered "Allow" (works with hosting off). rh_default_data_dir: the host's default
// data directory (~/.local/share/sodium-remote/host; null elsewhere). Strings are freed with rq_free_string.
char *rh_consent(RhHandle *);
void rh_forget_consent(RhHandle *);
char *rh_default_data_dir(void);

// Software video: no hardware H.264 encoder means OpenH264, Cisco's DLL, downloaded once.
// The setting lives in settings.json beside paired.json (default on); it works with hosting off too.
// rh_codec_retry: after a "failed" event, try the download again.
// Events (through the rh_new callback), JSON text; the state is worked out when hosting starts:
//   {"ev":"codec","event":"codec","state":S,"percent":N,"notice":"OpenH264 Video Codec provided by Cisco Systems, Inc."[,"error":"English, for logs"]}
//   S = "needed"       once ever: no hardware encoder, software video on, codec not there yet (the one-time dialog); "downloading" follows
//       "downloading"  percent 0..99
//       "ready"        percent 100 (also at a later start where it was already downloaded)
//       "failed"       download, checksum, unpacking or load failed; offer Try again. A build whose pin is still the placeholder fails here without any network
//       "off_blocked"  software video is off and there is no hardware encoder, so clients cannot connect
void rh_set_software_video(RhHandle *, int on);
int rh_software_video(RhHandle *);
void rh_codec_retry(RhHandle *);

#ifdef __cplusplus
}
#endif
