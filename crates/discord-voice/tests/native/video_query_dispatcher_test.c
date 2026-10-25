/* Each variant remains a separately compiled offline fixture. */

#if defined(SEREIN_NO_SDK_FIXTURE)
/* An installed FFmpeg encoder without query SDK support remains Unknown. */
#include <assert.h>
#include <stdio.h>
#include <libavcodec/avcodec.h>
#include <libavutil/opt.h>
#ifdef NDEBUG
#error Driver fixtures require assertions
#endif
int serein_video_query(int backend, int codec);
const AVCodec *avcodec_find_encoder_by_name(const char *name)
{
    static const AVCodec encoder = {0};
    assert(name);
    return &encoder;
}
int main(void)
{
    for (int codec = 0; codec < 3; codec++) {
        for (int backend = 1; backend <= 4; backend++) {
            if (backend != 2) assert(serein_video_query(backend, codec) == -1);
        }
    }
    puts("Missing query SDK: installed FFmpeg modules stay inconclusive");
    return 0;
}

#elif !defined(SEREIN_NO_SDK_FIXTURE)
/* A missing compiled encoder must never query a driver or advertise support. */
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libavutil/opt.h>
#include "video_encode_ffmpeg.h"
#ifdef NDEBUG
#error Driver fixtures require assertions
#endif

int serein_video_query(int backend, int codec);
static int installed = 1, calls = 0, last_codec = -1;
static const AVCodec encoder = {0};
static AVCodecContext context;
static int allocated, freed;
AVCodecContext *avcodec_alloc_context3(const AVCodec *codec) {
    assert(codec == &encoder); memset(&context, 0, sizeof(context));
    context.priv_data = &context; allocated++; return &context;
}
void avcodec_free_context(AVCodecContext **codec) {
    assert(*codec == &context); *codec = NULL; freed++;
}
const AVOption *av_opt_find(void *object, const char *name, const char *unit, int flags, int search) {
    (void)flags; (void)search; assert(object == &context);
    static const AVOption range = {.type = AV_OPT_TYPE_INT, .min = 0, .max = 100};
    static const AVOption value = {.type = AV_OPT_TYPE_CONST, .default_val.i64 = 5};
    if (!unit && (!strcmp(name, "preset") || !strcmp(name, "quality"))) return &range;
    return !strcmp(name, "p5") ? &value : NULL;
}

const AVCodec *avcodec_find_encoder_by_name(const char *name)
{
    assert(name && !strstr(name, "vaapi") && !strstr(name, "_mf"));
    return installed ? &encoder : NULL;
}

static int query(int codec) { calls++; last_codec = codec; return codec >= 6 ? ((1 << 5) | (1 << 10) | (1 << 18)) : 1; }
int serein_query_nvenc(int codec) { return query(codec); }
int serein_query_amf(int codec) { return query(codec); }
int serein_query_qsv(int codec) { return query(codec); }
int serein_query_videotoolbox(int codec) { return query(codec); }
int serein_query_nvenc_on_adapter(int codec, const SereinVideoAdapter *a) { assert(a); return query(codec); }
int serein_query_amf_on_adapter(int codec, const SereinVideoAdapter *a) { assert(a); return query(codec); }
int serein_query_qsv_on_adapter(int codec, const SereinVideoAdapter *a) { assert(a); return query(codec); }
int serein_query_videotoolbox_on_adapter(int codec, const SereinVideoAdapter *a) { assert(a); return query(codec); }

int main(void)
{
    for (int backend = 1; backend <= 4; backend++) {
        for (int codec = 0; codec < 3; codec++) {
            installed = 0;
            calls = 0;
            assert(serein_video_query(backend, codec) == 0 && calls == 0);
            installed = 1;
            const int absent = backend == 2 && codec == 2;
            assert(serein_video_query(backend, codec) == !absent);
            assert(calls == !absent);
            if (!absent) assert(last_codec == codec);
        }
    }
    calls = 0;
    assert(serein_video_query(0, 0) == -1);
    assert(serein_video_query(5, 0) == -1);
    assert(serein_video_query(1, -1) == -1);
    assert(serein_video_query(1, 3) == -1);
    assert(calls == 0);
    SereinVideoAdapter adapter = {SEREIN_GPU_PCI, 0x10de, 0x2684, 0, 2, 0, 0, 0};
    assert(serein_video_query_on_adapter(1, 2, &adapter) == 1 && calls == 1);
    calls = 0;
    assert(serein_video_query_on_adapter(4, 2, &adapter) == 0 && calls == 0);
    assert(serein_video_query_on_adapter(3, 2, &adapter) == 0 && calls == 0);
    assert(serein_video_query_on_adapter(1, 2, NULL) == -1 && calls == 0);
    assert(serein_video_query_on_adapter(1, 6, &adapter) == (1 << 5));
    assert(allocated == 1 && freed == 1);
    calls = 0;
    adapter.identity = SEREIN_GPU_UNIDENTIFIED;
    assert(serein_video_query_on_adapter(1, 2, &adapter) == -1 && calls == 0);
    puts("Driver dispatch: absent encoders and invalid requests never query hardware");
    return 0;
}

#else
#error Unknown offline fixture variant
#endif
