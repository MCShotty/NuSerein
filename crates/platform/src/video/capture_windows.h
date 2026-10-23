#ifndef SEREIN_CAPTURE_WINDOWS_H
#define SEREIN_CAPTURE_WINDOWS_H

#include "video_encode_ffmpeg.h"

/* Opaque, bounded Windows capture surfaces. No borrowed WGC texture escapes
 * submit; queued GPU work retains its COM reference until completion. */
typedef struct SereinCaptureDescription {
    uint32_t width, height, stride, format;
    uint32_t source_width, source_height;
    uint32_t unit_nits, white_nits, peak_nits;
    size_t bytes;
} SereinCaptureDescription;

typedef struct SereinCaptureStats {
    uint64_t submitted, completed, skipped, resources_created;
    uint64_t readback_bytes, readback_ns, gpu_frames;
} SereinCaptureStats;

/* format: 0=BGRA8, 1=linear RGBA16F, 2=NV12. GPU export is optional and
 * requires the exact selected adapter; CPU fallback never changes that choice. */
void *serein_capture_open(void *device, void *context,
                         const SereinVideoAdapter *adapter, int gpu_export);
void serein_capture_close(void *capture);
int serein_capture_submit(void *capture, void *texture, uint32_t width,
                          uint32_t height, uint32_t target_width,
                          uint32_t target_height, uint32_t format,
                          const SereinCaptureLuminance *luminance);
int serein_capture_take(void *capture, void **frame);
void serein_capture_stats(void *capture, SereinCaptureStats *stats);
int serein_capture_wants_cpu(void *capture);
void *serein_capture_cpu_replacement(void *capture);
void *serein_capture_replacement(void *capture, int gpu_export);
void serein_capture_frame_prefer_cpu(void *frame);
void serein_capture_frame_close(void *frame);
int serein_capture_frame_description(void *frame, SereinCaptureDescription *description);
/* Borrowed AVFrame, or NULL for a CPU-only staging frame. */
const void *serein_capture_frame_avframe(const void *frame);
int serein_capture_frame_download(void *frame, uint8_t *output, size_t capacity);
/* 1=ready, 0=pending/unsupported, -1=failure. Thumbnail transfers are nonblocking.
 * The caller supplies at most 640x360 pixels; format and stride describe raw
 * RGB/half samples, retaining HDR conversion in the shared Rust color code. */
int serein_capture_frame_preview(void *frame, uint32_t max_width,
                                 uint32_t max_height, uint8_t *output,
                                 size_t capacity, SereinCaptureDescription *description);
/* Offline native fixtures create owned textures, never a WGC session. */
int serein_capture_synthetic_test(int hardware, int hdr);

#endif
