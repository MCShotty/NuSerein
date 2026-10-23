/* Windows capture processing owns a bounded pool instead of allocating and
 * synchronously mapping a full desktop texture from every WGC callback. */
#include "capture_windows.h"
#include <stdlib.h>
#include <string.h>

#if defined(_WIN32)
#ifndef COBJMACROS
#define COBJMACROS
#endif
#include <windows.h>
#include <d3d11.h>
#include <d3d11_4.h>
#include <dxgi.h>
#include <libavutil/frame.h>
#include <libavutil/hwcontext.h>
#include <libavutil/hwcontext_d3d11va.h>
#include <libavutil/pixfmt.h>

#define CAPTURE_GPU_SLOTS 24
#define CAPTURE_CPU_SLOTS 3
#define CAPTURE_MAX_SLOTS CAPTURE_GPU_SLOTS
#define CAPTURE_MAX_BYTES ((size_t)512 * 1024 * 1024)
#define CAPTURE_MAX_PIXELS ((size_t)7680 * 4320)

static const GUID capture_iid_video_device =
    {0x10ec4d5b,0x975a,0x4689,{0xb9,0xe4,0xd0,0xaa,0xc3,0x0f,0xe3,0x33}};
static const GUID capture_iid_video_context =
    {0x61f21c45,0x3c0e,0x4a74,{0x9c,0xea,0x67,0x10,0x0d,0x9a,0xd5,0xe4}};
static const GUID capture_iid_dxgi_device =
    {0x54ec77fa,0x1377,0x44e6,{0x8c,0x32,0x88,0xfd,0x5f,0x44,0xc8,0x4c}};
static const GUID capture_iid_multithread =
    {0x9b7e4e00,0x342c,0x4106,{0xa1,0x9f,0x4f,0x27,0x04,0xf6,0x89,0xf0}};

typedef struct Capture Capture;
typedef struct CaptureSlot CaptureSlot;
typedef struct CaptureFrame { Capture *capture; CaptureSlot *slot; } CaptureFrame;

struct CaptureSlot {
    AVFrame *picture;
    ID3D11Texture2D *texture, *staging, *source;
    ID3D11VideoProcessorOutputView *view;
    ID3D11Query *done;
    int state; /* 0=free, 1=submitted, 2=owned by the consumer */
    uint64_t sequence;
    SereinCaptureDescription description;
    CaptureFrame lease;
};

/* All generations sharing one immediate context also share this lock. A rare
 * compatibility download in a retired pool cannot block the next callback:
 * submit uses TryEnterCriticalSection on this same lock and skips that input. */
typedef struct CaptureLock { LONG references; CRITICAL_SECTION value; } CaptureLock;

struct Capture {
    LONG references;
    CaptureLock *synchronization;
    ID3D11Device *device;
    ID3D11DeviceContext *context;
    ID3D11VideoDevice *video;
    ID3D11VideoContext *video_context;
    ID3D11VideoProcessorEnumerator *enumerator;
    ID3D11VideoProcessor *processor;
    ID3D11VideoProcessorInputView *input_view;
    ID3D11Texture2D *input_texture;
    uint32_t input_slice;
    AVBufferRef *hw_device, *hw_frames;
    CaptureSlot slots[CAPTURE_MAX_SLOTS];
    unsigned count;
    uint32_t source_width, source_height, width, height, format;
    DXGI_FORMAT input_format, output_format;
    int configured, gpu_export, adapter_matches, scaled;
    LONG force_cpu;
    SereinVideoAdapter selected;
    int has_selected;
    uint64_t sequence;
    SereinCaptureStats stats;
    ID3D11VideoProcessorEnumerator *preview_enumerator;
    ID3D11VideoProcessor *preview_processor;
    ID3D11VideoProcessorInputView *preview_input_view;
    ID3D11Texture2D *preview_input_texture;
    uint32_t preview_input_slice;
    ID3D11Texture2D *preview_texture, *preview_staging;
    ID3D11VideoProcessorOutputView *preview_view;
    ID3D11Query *preview_done;
    ID3D11Texture2D *download_staging;
    uint32_t preview_width, preview_height;
    int preview_pending;
};

static int dimensions(uint32_t width, uint32_t height)
{
    return width && height && width <= 7680 && height <= 7680 &&
           (width <= 4320 || height <= 4320) &&
           (size_t)width * height <= CAPTURE_MAX_PIXELS;
}

static void capture_lock(void *opaque) { EnterCriticalSection(&((Capture *)opaque)->synchronization->value); }
static void capture_unlock(void *opaque) { LeaveCriticalSection(&((Capture *)opaque)->synchronization->value); }

static void synchronization_release(CaptureLock *synchronization)
{
    if (synchronization && InterlockedDecrement(&synchronization->references) == 0) {
        DeleteCriticalSection(&synchronization->value);
        free(synchronization);
    }
}

static int adapter_matches(ID3D11Device *device, const SereinVideoAdapter *selected)
{
    IDXGIDevice *dxgi = NULL;
    IDXGIAdapter *adapter = NULL;
    DXGI_ADAPTER_DESC description;
    int matches = 0;
    if (!selected) return 1; /* Explicit standalone synthetic fixture only. */
    if (selected->identity != SEREIN_GPU_WINDOWS_LUID) return 0;
    if (FAILED(ID3D11Device_QueryInterface(device, &capture_iid_dxgi_device, (void **)&dxgi)) || !dxgi)
        return 0;
    if (SUCCEEDED(IDXGIDevice_GetAdapter(dxgi, &adapter)) && adapter &&
        SUCCEEDED(IDXGIAdapter_GetDesc(adapter, &description))) {
        uint64_t luid = (uint32_t)description.AdapterLuid.LowPart |
                        ((uint64_t)(uint32_t)description.AdapterLuid.HighPart << 32);
        matches = luid == selected->value && description.VendorId == selected->vendor_id &&
                  description.DeviceId == selected->device_id;
    }
    if (adapter) IDXGIAdapter_Release(adapter);
    IDXGIDevice_Release(dxgi);
    return matches;
}

void *serein_capture_open(void *native_device, void *native_context,
                         const SereinVideoAdapter *adapter, int gpu_export)
{
    ID3D11Device *device = native_device;
    ID3D11DeviceContext *context = native_context;
    ID3D11Multithread *multithread = NULL;
    Capture *capture;
    if (!device || !context || (gpu_export != 0 && gpu_export != 1)) return NULL;
    capture = calloc(1, sizeof(*capture));
    if (!capture) return NULL;
    capture->synchronization = calloc(1, sizeof(*capture->synchronization));
    if (!capture->synchronization) { free(capture); return NULL; }
    capture->synchronization->references = 1;
    InitializeCriticalSection(&capture->synchronization->value);
    capture->references = 1;
    capture->device = device;
    capture->context = context;
    ID3D11Device_AddRef(device);
    ID3D11DeviceContext_AddRef(context);
    /* Capture callbacks and FFmpeg may use this same immediate context from
     * separate workers. The D3D runtime, not a Rust Send cast, protects it. */
    if (SUCCEEDED(ID3D11DeviceContext_QueryInterface(context, &capture_iid_multithread, (void **)&multithread)) && multithread) {
        ID3D11Multithread_SetMultithreadProtected(multithread, TRUE);
        ID3D11Multithread_Release(multithread);
    } else {
        /* CPU-only mode keeps every context operation under the shared lock. */
        gpu_export = 0;
    }
    capture->adapter_matches = adapter_matches(device, adapter);
    if (adapter) { capture->selected = *adapter; capture->has_selected = 1; }
    capture->gpu_export = gpu_export && capture->adapter_matches;
    ID3D11Device_QueryInterface(device, &capture_iid_video_device, (void **)&capture->video);
    ID3D11DeviceContext_QueryInterface(context, &capture_iid_video_context, (void **)&capture->video_context);
    return capture;
}

static void slot_clear(CaptureSlot *slot)
{
    if (slot->source) ID3D11Texture2D_Release(slot->source);
    if (slot->view) ID3D11VideoProcessorOutputView_Release(slot->view);
    if (slot->staging) ID3D11Texture2D_Release(slot->staging);
    if (slot->texture) ID3D11Texture2D_Release(slot->texture);
    if (slot->done) ID3D11Query_Release(slot->done);
    av_frame_free(&slot->picture);
    memset(slot, 0, sizeof(*slot));
}

static void pool_clear(Capture *capture)
{
    for (unsigned i = 0; i < CAPTURE_MAX_SLOTS; ++i) slot_clear(&capture->slots[i]);
    av_buffer_unref(&capture->hw_frames);
    av_buffer_unref(&capture->hw_device);
    if (capture->processor) ID3D11VideoProcessor_Release(capture->processor);
    if (capture->enumerator) ID3D11VideoProcessorEnumerator_Release(capture->enumerator);
    if (capture->input_view) ID3D11VideoProcessorInputView_Release(capture->input_view);
    if (capture->input_texture) ID3D11Texture2D_Release(capture->input_texture);
    capture->input_view = NULL;
    capture->input_texture = NULL;
    capture->processor = NULL;
    capture->enumerator = NULL;
    capture->count = 0;
}

void serein_capture_close(void *opaque)
{
    Capture *capture = opaque;
    if (!capture || InterlockedDecrement(&capture->references) != 0) return;
    pool_clear(capture);
    if (capture->preview_done) ID3D11Query_Release(capture->preview_done);
    if (capture->download_staging) ID3D11Texture2D_Release(capture->download_staging);
    if (capture->preview_view) ID3D11VideoProcessorOutputView_Release(capture->preview_view);
    if (capture->preview_staging) ID3D11Texture2D_Release(capture->preview_staging);
    if (capture->preview_texture) ID3D11Texture2D_Release(capture->preview_texture);
    if (capture->preview_processor) ID3D11VideoProcessor_Release(capture->preview_processor);
    if (capture->preview_enumerator) ID3D11VideoProcessorEnumerator_Release(capture->preview_enumerator);
    if (capture->preview_input_view) ID3D11VideoProcessorInputView_Release(capture->preview_input_view);
    if (capture->preview_input_texture) ID3D11Texture2D_Release(capture->preview_input_texture);
    if (capture->video_context) ID3D11VideoContext_Release(capture->video_context);
    if (capture->video) ID3D11VideoDevice_Release(capture->video);
    ID3D11DeviceContext_Release(capture->context);
    ID3D11Device_Release(capture->device);
    synchronization_release(capture->synchronization);
    free(capture);
}

static int make_processor(Capture *capture, uint32_t width, uint32_t height,
                          uint32_t output_width, uint32_t output_height,
                          DXGI_FORMAT input, DXGI_FORMAT output,
                          ID3D11VideoProcessorEnumerator **enumerator,
                          ID3D11VideoProcessor **processor)
{
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC description = {0};
    UINT support = 0;
    if (!capture->video || !capture->video_context) return 0;
    description.InputFrameFormat = D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE;
    description.InputFrameRate.Numerator = description.OutputFrameRate.Numerator = 60;
    description.InputFrameRate.Denominator = description.OutputFrameRate.Denominator = 1;
    description.InputWidth = width;
    description.InputHeight = height;
    description.OutputWidth = output_width;
    description.OutputHeight = output_height;
    description.Usage = D3D11_VIDEO_USAGE_PLAYBACK_NORMAL;
    if (FAILED(ID3D11VideoDevice_CreateVideoProcessorEnumerator(capture->video, &description, enumerator)) || !*enumerator)
        return 0;
    if (FAILED(ID3D11VideoProcessorEnumerator_CheckVideoProcessorFormat(*enumerator, input, &support)) ||
        !(support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT)) goto failed;
    if (FAILED(ID3D11VideoProcessorEnumerator_CheckVideoProcessorFormat(*enumerator, output, &support)) ||
        !(support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT)) goto failed;
    if (FAILED(ID3D11VideoDevice_CreateVideoProcessor(capture->video, *enumerator, 0, processor)) || !*processor)
        goto failed;
    return 1;
failed:
    ID3D11VideoProcessorEnumerator_Release(*enumerator);
    *enumerator = NULL;
    return 0;
}

static int query_new(Capture *capture, ID3D11Query **query)
{
    D3D11_QUERY_DESC description = { D3D11_QUERY_EVENT, 0 };
    if (FAILED(ID3D11Device_CreateQuery(capture->device, &description, query))) return 0;
    capture->stats.resources_created++;
    return 1;
}

static int texture_new(Capture *capture, uint32_t width, uint32_t height,
                        DXGI_FORMAT format, int staging, ID3D11Texture2D **texture)
{
    D3D11_TEXTURE2D_DESC description = {0};
    description.Width = width;
    description.Height = height;
    description.MipLevels = description.ArraySize = 1;
    description.Format = format;
    description.SampleDesc.Count = 1;
    description.Usage = staging ? D3D11_USAGE_STAGING : D3D11_USAGE_DEFAULT;
    description.CPUAccessFlags = staging ? D3D11_CPU_ACCESS_READ : 0;
    description.BindFlags = staging ? 0 : D3D11_BIND_RENDER_TARGET;
    if (FAILED(ID3D11Device_CreateTexture2D(capture->device, &description, NULL, texture))) return 0;
    capture->stats.resources_created++;
    return 1;
}

static int output_view(Capture *capture, ID3D11Texture2D *texture,
                        uint32_t slice, ID3D11VideoProcessorEnumerator *enumerator,
                        ID3D11VideoProcessorOutputView **view)
{
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC description = {0};
    D3D11_TEXTURE2D_DESC texture_description;
    ID3D11Texture2D_GetDesc(texture, &texture_description);
    if (texture_description.ArraySize == 1 && slice == 0) {
        description.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;
        description.Texture2D.MipSlice = 0;
    } else {
        description.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2DARRAY;
        description.Texture2DArray.MipSlice = 0;
        description.Texture2DArray.FirstArraySlice = slice;
        description.Texture2DArray.ArraySize = 1;
    }
    if (FAILED(ID3D11VideoDevice_CreateVideoProcessorOutputView(capture->video,
            (ID3D11Resource *)texture, enumerator, &description, view))) return 0;
    capture->stats.resources_created++;
    return 1;
}

static int initialize_gpu(Capture *capture, uint32_t width, uint32_t height)
{
    AVHWDeviceContext *device;
    AVD3D11VADeviceContext *native;
    AVHWFramesContext *frames;
    AVD3D11VAFramesContext *native_frames;
    /* Lookahead is unchanged: twenty-four reusable NV12 frames accommodate
     * sixteen-picture analysis plus capture/transport credits. */
    size_t bytes = (size_t)((width + 127) & ~127u) * ((height + 127) & ~127u) * 3 / 2;
    if (!capture->gpu_export || bytes > (CAPTURE_MAX_BYTES - 4 * 1024 * 1024) / CAPTURE_GPU_SLOTS)
        return 0;
    capture->hw_device = av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA);
    if (!capture->hw_device) return 0;
    device = (AVHWDeviceContext *)capture->hw_device->data;
    native = device->hwctx;
    native->device = capture->device;
    native->device_context = capture->context;
    ID3D11Device_AddRef(native->device);
    ID3D11DeviceContext_AddRef(native->device_context);
    /* FFmpeg creates its own device lock. Its lifetime follows AVFrame refs,
     * which may outlive Capture; no callback retains a Capture pointer. The
     * shared immediate context itself is multithread-protected above. */
    if (av_hwdevice_ctx_init(capture->hw_device) < 0) return 0;
    capture->hw_frames = av_hwframe_ctx_alloc(capture->hw_device);
    if (!capture->hw_frames) return 0;
    frames = (AVHWFramesContext *)capture->hw_frames->data;
    frames->format = AV_PIX_FMT_D3D11;
    frames->sw_format = AV_PIX_FMT_NV12;
    frames->width = width;
    frames->height = height;
    frames->initial_pool_size = CAPTURE_GPU_SLOTS;
    native_frames = frames->hwctx;
    native_frames->BindFlags = D3D11_BIND_RENDER_TARGET;
    if (av_hwframe_ctx_init(capture->hw_frames) < 0) return 0;
    for (unsigned i = 0; i < CAPTURE_GPU_SLOTS; ++i) {
        CaptureSlot *slot = &capture->slots[i];
        slot->picture = av_frame_alloc();
        if (!slot->picture || av_hwframe_get_buffer(capture->hw_frames, slot->picture, 0) < 0)
            return 0;
        slot->texture = (ID3D11Texture2D *)slot->picture->data[0];
        ID3D11Texture2D_AddRef(slot->texture);
        if (!output_view(capture, slot->texture, (uint32_t)(uintptr_t)slot->picture->data[1], capture->enumerator, &slot->view) ||
            !query_new(capture, &slot->done)) return 0;
        slot->picture->color_range = AVCOL_RANGE_MPEG;
        slot->picture->colorspace = AVCOL_SPC_SMPTE170M;
        slot->picture->color_primaries = AVCOL_PRI_BT709;
        slot->picture->color_trc = AVCOL_TRC_IEC61966_2_1;
    }
    capture->count = CAPTURE_GPU_SLOTS;
    return 1;
}

static int configure(Capture *capture, uint32_t width, uint32_t height,
                      uint32_t target_width, uint32_t target_height, uint32_t format)
{
    int gpu = 0;
    size_t per_slot;
    capture->source_width = width;
    capture->source_height = height;
    capture->input_format = format == 1 ? DXGI_FORMAT_R16G16B16A16_FLOAT : DXGI_FORMAT_B8G8R8A8_UNORM;
    capture->width = width;
    capture->height = height;
    capture->format = format;
    capture->output_format = capture->input_format;
    capture->scaled = 0;
    if (format == 0 && make_processor(capture, width, height, target_width, target_height,
            capture->input_format, DXGI_FORMAT_NV12, &capture->enumerator, &capture->processor)) {
        gpu = initialize_gpu(capture, target_width, target_height);
        if (gpu) {
            capture->width = target_width;
            capture->height = target_height;
            capture->format = 2;
            capture->output_format = DXGI_FORMAT_NV12;
            capture->scaled = 1;
        } else {
            pool_clear(capture);
        }
    }
    if (!gpu) {
        /* Preserve half-float HDR bytes for the existing shared tone mapper.
         * SDR fallback still scales on the video processor before readback. */
        if (format == 0 && make_processor(capture, width, height, target_width, target_height,
                capture->input_format, DXGI_FORMAT_B8G8R8A8_UNORM, &capture->enumerator, &capture->processor)) {
            capture->width = target_width;
            capture->height = target_height;
            capture->scaled = 1;
        }
        per_slot = (size_t)capture->width * capture->height * (format == 1 ? 8 : 4) * (capture->scaled ? 2 : 1);
        capture->count = (unsigned)((CAPTURE_MAX_BYTES - 4 * 1024 * 1024) / per_slot);
        if (capture->count > CAPTURE_CPU_SLOTS) capture->count = CAPTURE_CPU_SLOTS;
        if (capture->count < 2) goto failed;
        for (unsigned i = 0; i < capture->count; ++i) {
            CaptureSlot *slot = &capture->slots[i];
            if (!texture_new(capture, capture->width, capture->height, capture->output_format, 1, &slot->staging) ||
                !query_new(capture, &slot->done)) goto failed;
            if (capture->scaled && (!texture_new(capture, capture->width, capture->height, capture->output_format, 0, &slot->texture) ||
                !output_view(capture, slot->texture, 0, capture->enumerator, &slot->view))) goto failed;
        }
    }
    capture->configured = 1;
    return 1;
failed:
    pool_clear(capture);
    return 0;
}

static int process(Capture *capture, ID3D11Texture2D *source, uint32_t slice,
                    ID3D11VideoProcessorEnumerator *enumerator, ID3D11VideoProcessor *processor,
                    ID3D11VideoProcessorOutputView *output, uint32_t source_width,
                    uint32_t source_height, uint32_t width, uint32_t height,
                    int input_yuv, int output_yuv)
{
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC input_description = {0};
    ID3D11VideoProcessorInputView *input = NULL;
    ID3D11VideoProcessorInputView **cached_view = enumerator == capture->enumerator ? &capture->input_view : &capture->preview_input_view;
    ID3D11Texture2D **cached_texture = enumerator == capture->enumerator ? &capture->input_texture : &capture->preview_input_texture;
    uint32_t *cached_slice = enumerator == capture->enumerator ? &capture->input_slice : &capture->preview_input_slice;
    D3D11_VIDEO_PROCESSOR_STREAM stream = {0};
    D3D11_VIDEO_PROCESSOR_COLOR_SPACE input_color = {0}, output_color = {0};
    RECT source_rect = {0, 0, (LONG)source_width, (LONG)source_height};
    RECT output_rect = {0, 0, (LONG)width, (LONG)height};
    RECT destination_rect = output_rect;
    D3D11_VIDEO_COLOR background = {0};
    HRESULT result;
    input_description.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;
    input_description.Texture2D.ArraySlice = slice;
    if (*cached_texture != source || *cached_slice != slice || !*cached_view) {
        if (*cached_view) ID3D11VideoProcessorInputView_Release(*cached_view);
        if (*cached_texture) ID3D11Texture2D_Release(*cached_texture);
        *cached_view = NULL;
        *cached_texture = NULL;
        result = ID3D11VideoDevice_CreateVideoProcessorInputView(capture->video,
            (ID3D11Resource *)source, enumerator, &input_description, cached_view);
        if (FAILED(result) || !*cached_view) return 0;
        *cached_texture = source;
        ID3D11Texture2D_AddRef(source);
        *cached_slice = slice;
        capture->stats.resources_created++;
    }
    input = *cached_view;
    input_color.Nominal_Range = input_yuv ? 1 : 2;
    output_color.Nominal_Range = output_yuv ? 1 : 2;
    /* Zero matrix selector is BT.601, matching the existing CPU I420 path. */
    ID3D11VideoContext_VideoProcessorSetStreamColorSpace(capture->video_context, processor, 0, &input_color);
    ID3D11VideoContext_VideoProcessorSetOutputColorSpace(capture->video_context, processor, &output_color);
    ID3D11VideoContext_VideoProcessorSetStreamFrameFormat(capture->video_context, processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
    ID3D11VideoContext_VideoProcessorSetStreamSourceRect(capture->video_context, processor, 0, TRUE, &source_rect);
    /* Fit uniformly. Even codec alignment must not stretch an odd source. */
    if ((uint64_t)source_width * height > (uint64_t)source_height * width) {
        uint32_t scaled = (uint32_t)(((uint64_t)source_height * width + source_width / 2) / source_width);
        if (!scaled) scaled = 1;
        destination_rect.top = (LONG)(height - scaled) / 2;
        destination_rect.bottom = destination_rect.top + (LONG)scaled;
    } else {
        uint32_t scaled = (uint32_t)(((uint64_t)source_width * height + source_height / 2) / source_height);
        if (!scaled) scaled = 1;
        destination_rect.left = (LONG)(width - scaled) / 2;
        destination_rect.right = destination_rect.left + (LONG)scaled;
    }
    background.RGBA.A = 1.0f;
    ID3D11VideoContext_VideoProcessorSetOutputBackgroundColor(capture->video_context, processor, FALSE, &background);
    ID3D11VideoContext_VideoProcessorSetStreamDestRect(capture->video_context, processor, 0, TRUE, &destination_rect);
    ID3D11VideoContext_VideoProcessorSetOutputTargetRect(capture->video_context, processor, TRUE, &output_rect);
    /* Capture has no interlacing or temporal filtering; avoid hidden history. */
    ID3D11VideoContext_VideoProcessorSetStreamAutoProcessingMode(capture->video_context, processor, 0, FALSE);
    stream.Enable = TRUE;
    stream.pInputSurface = input;
    result = ID3D11VideoContext_VideoProcessorBlt(capture->video_context, processor, output, 0, 1, &stream);
    return SUCCEEDED(result);
}

int serein_capture_submit(void *opaque, void *native_texture, uint32_t width,
                          uint32_t height, uint32_t target_width,
                          uint32_t target_height, uint32_t format,
                          const SereinCaptureLuminance *luminance)
{
    Capture *capture = opaque;
    ID3D11Texture2D *texture = native_texture;
    D3D11_TEXTURE2D_DESC texture_description;
    CaptureSlot *slot = NULL;
    if (!capture || !texture || !dimensions(width, height) || !dimensions(target_width, target_height) ||
        target_width > width || target_height > height || (target_width & 1) || (target_height & 1) || format > 1)
        return -1;
    /* A rare worker-side compatibility readback must never make the capture
     * callback wait for a mapped resource. Drop this input under contention. */
    if (!TryEnterCriticalSection(&capture->synchronization->value)) return 0;
    ID3D11Texture2D_GetDesc(texture, &texture_description);
    if (texture_description.Width != width || texture_description.Height != height ||
        texture_description.Format != (format == 1 ? DXGI_FORMAT_R16G16B16A16_FLOAT : DXGI_FORMAT_B8G8R8A8_UNORM)) goto failed;
    if (!capture->configured && !configure(capture, width, height, target_width, target_height, format)) goto failed;
    /* A geometry/color transition gets a new Processor. The old pool survives
     * only for already-owned frames and is never overwritten underneath them. */
    if (capture->source_width != width || capture->source_height != height ||
        capture->input_format != texture_description.Format ||
        (capture->scaled && (capture->width != target_width || capture->height != target_height))) goto failed;
    for (unsigned i = 0; i < capture->count; ++i) {
        CaptureSlot *candidate = &capture->slots[i];
        if (candidate->state == 0 && (!candidate->picture || av_buffer_get_ref_count(candidate->picture->buf[0]) == 1)) {
            slot = candidate;
            break;
        }
    }
    if (!slot) {
        capture->stats.skipped++;
        capture_unlock(capture);
        return 0;
    }
    slot->source = texture;
    ID3D11Texture2D_AddRef(texture);
    if (capture->scaled) {
        if (!process(capture, texture, 0, capture->enumerator, capture->processor, slot->view,
                width, height, capture->width, capture->height, 0, capture->format == 2)) goto failed_slot;
        if (slot->staging)
            ID3D11DeviceContext_CopyResource(capture->context, (ID3D11Resource *)slot->staging, (ID3D11Resource *)slot->texture);
    } else {
        ID3D11DeviceContext_CopyResource(capture->context, (ID3D11Resource *)slot->staging, (ID3D11Resource *)texture);
    }
    ID3D11DeviceContext_End(capture->context, (ID3D11Asynchronous *)slot->done);
    /* Flush submits the asynchronous work without waiting for its GPU completion. */
    ID3D11DeviceContext_Flush(capture->context);
    slot->description = (SereinCaptureDescription){
        capture->width, capture->height,
        capture->width * (capture->format == 1 ? 8 : capture->format == 2 ? 1 : 4),
        capture->format, width, height,
        luminance ? luminance->unit_nits : 0,
        luminance ? luminance->white_nits : 0,
        luminance ? luminance->peak_nits : 0,
        (size_t)capture->width * capture->height * (capture->format == 1 ? 8 : capture->format == 2 ? 3 : 4) / (capture->format == 2 ? 2 : 1)
    };
    slot->sequence = ++capture->sequence;
    slot->state = 1;
    capture->stats.submitted++;
    capture_unlock(capture);
    return 1;
failed_slot:
    ID3D11Texture2D_Release(slot->source);
    slot->source = NULL;
failed:
    capture_unlock(capture);
    return -1;
}

int serein_capture_take(void *opaque, void **output)
{
    Capture *capture = opaque;
    CaptureSlot *slot = NULL;
    CaptureFrame *frame;
    BOOL complete = FALSE;
    HRESULT result;
    if (output) *output = NULL;
    if (!capture || !output) return -1;
    capture_lock(capture);
    for (unsigned i = 0; i < capture->count; ++i) {
        CaptureSlot *candidate = &capture->slots[i];
        if (candidate->state == 1 && (!slot || candidate->sequence < slot->sequence)) slot = candidate;
    }
    if (!slot) { capture_unlock(capture); return 0; }
    result = ID3D11DeviceContext_GetData(capture->context, (ID3D11Asynchronous *)slot->done,
        &complete, sizeof(complete), D3D11_ASYNC_GETDATA_DONOTFLUSH);
    if (result == S_FALSE || (result == S_OK && !complete)) { capture_unlock(capture); return 0; }
    if (FAILED(result)) { capture_unlock(capture); return -1; }
    frame = &slot->lease;
    ID3D11Texture2D_Release(slot->source);
    slot->source = NULL;
    slot->state = 2;
    frame->capture = capture;
    frame->slot = slot;
    InterlockedIncrement(&capture->references);
    capture->stats.completed++;
    capture->stats.gpu_frames += slot->picture != NULL;
    *output = frame;
    capture_unlock(capture);
    return 1;
}

void serein_capture_stats(void *opaque, SereinCaptureStats *stats)
{
    Capture *capture = opaque;
    if (!stats) return;
    memset(stats, 0, sizeof(*stats));
    if (!capture) return;
    capture_lock(capture);
    *stats = capture->stats;
    capture_unlock(capture);
}

int serein_capture_wants_cpu(void *opaque)
{
    Capture *capture = opaque;
    return capture && InterlockedCompareExchange(&capture->force_cpu, 0, 0) != 0;
}

void *serein_capture_cpu_replacement(void *opaque)
{
    Capture *capture = opaque;
    return serein_capture_replacement(capture, 0);
}

void *serein_capture_replacement(void *opaque, int gpu_export)
{
    Capture *capture = opaque;
    Capture *replacement;
    if (!capture) return NULL;
    replacement = serein_capture_open(capture->device, capture->context,
        capture->has_selected ? &capture->selected : NULL, gpu_export);
    if (replacement) {
        synchronization_release(replacement->synchronization);
        replacement->synchronization = capture->synchronization;
        InterlockedIncrement(&replacement->synchronization->references);
    }
    return replacement;
}

void serein_capture_frame_prefer_cpu(void *opaque)
{
    CaptureFrame *frame = opaque;
    if (frame) InterlockedExchange(&frame->capture->force_cpu, 1);
}

void serein_capture_frame_close(void *opaque)
{
    CaptureFrame *frame = opaque;
    Capture *capture;
    if (!frame) return;
    capture = frame->capture;
    capture_lock(capture);
    frame->slot->state = 0;
    capture_unlock(capture);
    serein_capture_close(capture);
}

int serein_capture_frame_description(void *opaque, SereinCaptureDescription *description)
{
    const CaptureFrame *frame = opaque;
    if (!frame || !description) return 0;
    *description = frame->slot->description;
    return 1;
}

const void *serein_capture_frame_avframe(const void *opaque)
{
    const CaptureFrame *frame = opaque;
    return frame ? frame->slot->picture : NULL;
}

int serein_capture_frame_download(void *opaque, uint8_t *output, size_t capacity)
{
    CaptureFrame *frame = opaque;
    Capture *capture;
    CaptureSlot *slot;
    D3D11_MAPPED_SUBRESOURCE mapped = {0};
    LARGE_INTEGER started, ended, frequency;
    uint32_t rows;
    if (!frame || !output) return -1;
    slot = frame->slot;
    capture = frame->capture;
    if (capacity < slot->description.bytes) return -1;
    capture_lock(capture);
    QueryPerformanceCounter(&started);
    ID3D11Texture2D *staging = slot->staging;
    if (!staging) {
        /* A hardware encoder may reject texture input. This rare compatibility
         * download happens on the encoder worker, never the WGC callback. */
        if (!capture->download_staging && !texture_new(capture, capture->width, capture->height,
                capture->output_format, 1, &capture->download_staging)) goto failed;
        staging = capture->download_staging;
        ID3D11DeviceContext_CopySubresourceRegion(capture->context, (ID3D11Resource *)staging, 0,
            0, 0, 0, (ID3D11Resource *)slot->texture, (UINT)(uintptr_t)slot->picture->data[1], NULL);
    }
    if (FAILED(ID3D11DeviceContext_Map(capture->context, (ID3D11Resource *)staging, 0,
            D3D11_MAP_READ, 0, &mapped))) goto failed;
    if (!mapped.pData || mapped.RowPitch < slot->description.stride) {
        ID3D11DeviceContext_Unmap(capture->context, (ID3D11Resource *)staging, 0);
        goto failed;
    }
    rows = slot->description.height + (slot->description.format == 2 ? slot->description.height / 2 : 0);
    for (uint32_t row = 0; row < rows; ++row)
        memcpy(output + (size_t)row * slot->description.stride,
            (const uint8_t *)mapped.pData + (size_t)row * mapped.RowPitch, slot->description.stride);
    ID3D11DeviceContext_Unmap(capture->context, (ID3D11Resource *)staging, 0);
    QueryPerformanceCounter(&ended);
    QueryPerformanceFrequency(&frequency);
    capture->stats.readback_bytes += slot->description.bytes;
    if (frequency.QuadPart > 0)
        capture->stats.readback_ns += (uint64_t)((ended.QuadPart - started.QuadPart) * 1000000000ULL / frequency.QuadPart);
    capture_unlock(capture);
    return 1;
failed:
    capture_unlock(capture);
    return -1;
}

static void preview_clear(Capture *capture)
{
    if (capture->preview_done) ID3D11Query_Release(capture->preview_done);
    if (capture->preview_view) ID3D11VideoProcessorOutputView_Release(capture->preview_view);
    if (capture->preview_staging) ID3D11Texture2D_Release(capture->preview_staging);
    if (capture->preview_texture) ID3D11Texture2D_Release(capture->preview_texture);
    if (capture->preview_processor) ID3D11VideoProcessor_Release(capture->preview_processor);
    if (capture->preview_enumerator) ID3D11VideoProcessorEnumerator_Release(capture->preview_enumerator);
    if (capture->preview_input_view) ID3D11VideoProcessorInputView_Release(capture->preview_input_view);
    if (capture->preview_input_texture) ID3D11Texture2D_Release(capture->preview_input_texture);
    capture->preview_done = NULL;
    capture->preview_view = NULL;
    capture->preview_staging = capture->preview_texture = NULL;
    capture->preview_processor = NULL;
    capture->preview_enumerator = NULL;
    capture->preview_input_view = NULL;
    capture->preview_input_texture = NULL;
    capture->preview_pending = 0;
}

int serein_capture_frame_preview(void *opaque, uint32_t max_width,
                                 uint32_t max_height, uint8_t *output,
                                 size_t capacity, SereinCaptureDescription *description)
{
    CaptureFrame *frame = opaque;
    Capture *capture;
    CaptureSlot *slot;
    D3D11_MAPPED_SUBRESOURCE mapped = {0};
    uint32_t width, height, bytes;
    int half;
    HRESULT result;
    BOOL complete = FALSE;
    if (!frame || !output || !description || !max_width || !max_height || max_width > 640 || max_height > 360)
        return -1;
    capture = frame->capture;
    slot = frame->slot;
    width = capture->width;
    height = capture->height;
    if (width <= max_width && height <= max_height) {
        /* A thumbnail never enlarges an already small source. */
    } else if ((uint64_t)width * max_height > (uint64_t)height * max_width) {
        height = (uint32_t)((uint64_t)height * max_width / width);
        width = max_width;
    } else {
        width = (uint32_t)((uint64_t)width * max_height / height);
        height = max_height;
    }
    if (!width) width = 1;
    if (!height) height = 1;
    half = slot->description.format == 1;
    bytes = half ? 8 : 4;
    if (capacity < (size_t)width * height * bytes) return -1;
    capture_lock(capture);
    if (slot->staging && !slot->picture) {
        /* CPU fallback already has a completed readback. Sample only the
         * thumbnail pixels and preserve half-float HDR for Rust's tone mapper. */
        result = ID3D11DeviceContext_Map(capture->context, (ID3D11Resource *)slot->staging,
            0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT, &mapped);
        if (result == DXGI_ERROR_WAS_STILL_DRAWING) { capture_unlock(capture); return 0; }
        if (FAILED(result)) goto failed;
        if (!mapped.pData || mapped.RowPitch < capture->width * bytes) {
            ID3D11DeviceContext_Unmap(capture->context, (ID3D11Resource *)slot->staging, 0);
            goto failed;
        }
        for (uint32_t row = 0; row < height; ++row) {
            uint32_t source_row = (uint32_t)((uint64_t)row * capture->height / height);
            for (uint32_t column = 0; column < width; ++column) {
                uint32_t source_column = (uint32_t)((uint64_t)column * capture->width / width);
                memcpy(output + ((size_t)row * width + column) * bytes,
                    (const uint8_t *)mapped.pData + (size_t)source_row * mapped.RowPitch + (size_t)source_column * bytes, bytes);
            }
        }
        ID3D11DeviceContext_Unmap(capture->context, (ID3D11Resource *)slot->staging, 0);
    } else {
        /* Only a bounded, GPU-scaled BGRA thumbnail crosses back to the CPU. */
        if (capture->preview_width != width || capture->preview_height != height) {
            preview_clear(capture);
            if (!make_processor(capture, capture->width, capture->height, width, height,
                    capture->output_format, DXGI_FORMAT_B8G8R8A8_UNORM,
                    &capture->preview_enumerator, &capture->preview_processor) ||
                !texture_new(capture, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, 0, &capture->preview_texture) ||
                !texture_new(capture, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, 1, &capture->preview_staging) ||
                !output_view(capture, capture->preview_texture, 0, capture->preview_enumerator, &capture->preview_view) ||
                !query_new(capture, &capture->preview_done)) {
                preview_clear(capture);
                capture_unlock(capture);
                return 0;
            }
            capture->preview_width = width;
            capture->preview_height = height;
        }
        if (!capture->preview_pending) {
            if (!process(capture, slot->texture, (uint32_t)(uintptr_t)slot->picture->data[1],
                    capture->preview_enumerator, capture->preview_processor, capture->preview_view,
                    capture->width, capture->height, width, height, 1, 0)) goto failed;
            ID3D11DeviceContext_CopyResource(capture->context, (ID3D11Resource *)capture->preview_staging,
                (ID3D11Resource *)capture->preview_texture);
            ID3D11DeviceContext_End(capture->context, (ID3D11Asynchronous *)capture->preview_done);
            ID3D11DeviceContext_Flush(capture->context);
            capture->preview_pending = 1;
            capture_unlock(capture);
            return 0;
        }
        result = ID3D11DeviceContext_GetData(capture->context, (ID3D11Asynchronous *)capture->preview_done,
            &complete, sizeof(complete), D3D11_ASYNC_GETDATA_DONOTFLUSH);
        if (result == S_FALSE || (result == S_OK && !complete)) { capture_unlock(capture); return 0; }
        if (FAILED(result)) goto failed;
        result = ID3D11DeviceContext_Map(capture->context, (ID3D11Resource *)capture->preview_staging,
            0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT, &mapped);
        if (result == DXGI_ERROR_WAS_STILL_DRAWING) { capture_unlock(capture); return 0; }
        if (FAILED(result)) goto failed;
        if (!mapped.pData || mapped.RowPitch < width * 4) {
            ID3D11DeviceContext_Unmap(capture->context, (ID3D11Resource *)capture->preview_staging, 0);
            goto failed;
        }
        for (uint32_t row = 0; row < height; ++row)
            memcpy(output + (size_t)row * width * 4, (const uint8_t *)mapped.pData + (size_t)row * mapped.RowPitch, (size_t)width * 4);
        ID3D11DeviceContext_Unmap(capture->context, (ID3D11Resource *)capture->preview_staging, 0);
        capture->preview_pending = 0;
    }
    *description = slot->description;
    description->width = width;
    description->height = height;
    description->stride = width * bytes;
    description->format = half ? 1 : 0;
    description->bytes = (size_t)width * height * bytes;
    capture_unlock(capture);
    return 1;
failed:
    capture_unlock(capture);
    return -1;
}

static int synthetic_take(Capture *capture, void **frame)
{
    for (unsigned i = 0; i < 3000; ++i) {
        int result = serein_capture_take(capture, frame);
        if (result) return result;
        Sleep(1);
    }
    return -1;
}

int serein_capture_synthetic_test(int hardware, int hdr)
{
    ID3D11Device *device = NULL;
    ID3D11DeviceContext *context = NULL;
    ID3D11Texture2D *texture = NULL;
    D3D11_TEXTURE2D_DESC texture_description = {0};
    D3D11_SUBRESOURCE_DATA initial = {0};
    D3D_FEATURE_LEVEL feature;
    Capture *capture = NULL;
    void *frames[CAPTURE_GPU_SLOTS] = {0};
    uint8_t pixels[128 * 72 * 8], downloaded[128 * 72 * 8];
    unsigned count = 0;
    int status = -1;
    SereinCaptureStats warm, after;
    SereinCaptureLuminance luminance = {80, 203, 1000};
    if ((hardware != 0 && hardware != 1) || (hdr != 0 && hdr != 1)) return -1;
    if (FAILED(D3D11CreateDevice(NULL, hardware ? D3D_DRIVER_TYPE_HARDWARE : D3D_DRIVER_TYPE_WARP,
            NULL, D3D11_CREATE_DEVICE_BGRA_SUPPORT, NULL, 0, D3D11_SDK_VERSION,
            &device, &feature, &context))) return 0;
    memset(pixels, 0, sizeof(pixels));
    for (unsigned i = 0; i < 128 * 72; ++i) {
        if (hdr) {
            const uint16_t patch[] = {0x3800,0x3400,0x3000,0x3c00};
            memcpy(pixels + i * 8, patch, 8);
        } else {
            pixels[i * 4] = 32;
            pixels[i * 4 + 1] = 96;
            pixels[i * 4 + 2] = 192;
            pixels[i * 4 + 3] = 255;
        }
    }
    texture_description.Width = 128;
    texture_description.Height = 72;
    texture_description.MipLevels = texture_description.ArraySize = 1;
    texture_description.Format = hdr ? DXGI_FORMAT_R16G16B16A16_FLOAT : DXGI_FORMAT_B8G8R8A8_UNORM;
    texture_description.SampleDesc.Count = 1;
    texture_description.Usage = D3D11_USAGE_DEFAULT;
    texture_description.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
    initial.pSysMem = pixels;
    initial.SysMemPitch = 128 * (hdr ? 8 : 4);
    if (FAILED(ID3D11Device_CreateTexture2D(device, &texture_description, &initial, &texture))) goto cleanup;
    capture = serein_capture_open(device, context, NULL, hardware && !hdr);
    if (!capture) goto cleanup;
    if (serein_capture_submit(capture, texture, 7681, 72, 64, 36, hdr, &luminance) != -1) goto cleanup;
    if (serein_capture_submit(capture, texture, 128, 72, 64, 36, hdr, &luminance) != 1 ||
        synthetic_take(capture, &frames[0]) != 1) goto cleanup;
    count = capture->count;
    if (count < 2 || count > CAPTURE_MAX_SLOTS || (!hdr && hardware && count != CAPTURE_GPU_SLOTS)) {
        /* No hardware video processor is a supported CPU fallback, not an
         * assertion that the hardware NV12 test ran successfully. */
        if (hardware && !hdr && !capture->slots[0].picture) { status = 2; goto cleanup; }
        goto cleanup;
    }
    SereinCaptureDescription description;
    if (!serein_capture_frame_description(frames[0], &description) ||
        description.source_width != 128 || description.source_height != 72 ||
        serein_capture_frame_download(frames[0], downloaded, sizeof(downloaded)) != 1) goto cleanup;
    if (hdr) {
        if (description.format != 1 || description.width != 128 || description.height != 72 ||
            description.unit_nits != 80 || description.white_nits != 203 || description.peak_nits != 1000 ||
            memcmp(downloaded, pixels, description.bytes)) goto cleanup;
    } else if (description.format == 2) {
        /* Same limited-range BT.601 patch as the Rust converter, allowing two
         * code values for device chroma/range rounding. */
        int y = ((66 * 192 + 129 * 96 + 25 * 32 + 128) >> 8) + 16;
        int u = ((-38 * 192 - 74 * 96 + 112 * 32 + 128) >> 8) + 128;
        int v = ((112 * 192 - 94 * 96 - 18 * 32 + 128) >> 8) + 128;
        size_t uv = (size_t)description.width * description.height;
        if (abs((int)downloaded[description.width + 1] - y) > 2 ||
            abs((int)downloaded[uv] - u) > 2 || abs((int)downloaded[uv + 1] - v) > 2) goto cleanup;
    } else {
        size_t center = ((size_t)(description.height / 2) * description.width + description.width / 2) * 4;
        for (unsigned channel = 0; channel < 3; ++channel)
            if (abs((int)downloaded[center + channel] - pixels[channel]) > 2) goto cleanup;
    }
    for (unsigned i = 1; i < count; ++i) {
        if (serein_capture_submit(capture, texture, 128, 72, 64, 36, hdr, &luminance) != 1 ||
            synthetic_take(capture, &frames[i]) != 1) goto cleanup;
    }
    /* Every owned frame consumes a pool credit. No hidden extra surface is
     * allocated when capture outruns its consumer. */
    serein_capture_stats(capture, &warm);
    if (serein_capture_submit(capture, texture, 128, 72, 64, 36, hdr, &luminance) != 0) goto cleanup;
    serein_capture_frame_close(frames[0]);
    frames[0] = NULL;
    if (serein_capture_submit(capture, texture, 128, 72, 64, 36, hdr, &luminance) != 1 ||
        synthetic_take(capture, &frames[0]) != 1) goto cleanup;
    serein_capture_stats(capture, &after);
    if (after.resources_created != warm.resources_created || after.skipped != warm.skipped + 1) goto cleanup;
    /* Closing capture with outstanding pictures must leave those pictures
     * valid, including their source metadata and CPU compatibility download. */
    serein_capture_close(capture);
    capture = NULL;
    if (serein_capture_frame_download(frames[0], downloaded, sizeof(downloaded)) != 1) goto cleanup;
    status = 1;
cleanup:
    for (unsigned i = 0; i < CAPTURE_MAX_SLOTS; ++i) serein_capture_frame_close(frames[i]);
    serein_capture_close(capture);
    if (texture) ID3D11Texture2D_Release(texture);
    if (context) ID3D11DeviceContext_Release(context);
    if (device) ID3D11Device_Release(device);
    return status;
}

#else
/* Compiling the translation unit on other platforms keeps native build wiring
 * uniform without introducing Windows symbols or runtime behavior. */
typedef int serein_capture_windows_translation_unit;
#endif
