/* Bounded, per-camera FFmpeg temporal filter. No URLs or global state. */
#include <libavfilter/avfilter.h>
#include <libavfilter/buffersink.h>
#include <libavfilter/buffersrc.h>
#include <libavutil/error.h>
#include <libavutil/frame.h>
#include <libavutil/pixfmt.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    AVFilterGraph *graph;
    AVFilterContext *source;
    AVFilterContext *sink;
    int width, height;
} SereinDenoise;

void serein_denoise_free(SereinDenoise *d) {
    if (!d) return;
    avfilter_graph_free(&d->graph);
    free(d);
}

SereinDenoise *serein_denoise_new(int width, int height) {
    /* Five YUV pictures plus input/output and the camera's own buffers. */
    if (width <= 0 || height <= 0 || width > 7680 || height > 4320 || (width & 1) || (height & 1) ||
        (int64_t)width * height > 1920 * 1080) return NULL;
    SereinDenoise *d = calloc(1, sizeof(*d));
    if (!d) return NULL;
    d->width = width;
    d->height = height;
    d->graph = avfilter_graph_alloc();
    if (!d->graph) goto fail;
    d->graph->nb_threads = 2;
    const AVFilter *source = avfilter_get_by_name("buffer");
    const AVFilter *filter = avfilter_get_by_name("atadenoise");
    const AVFilter *sink = avfilter_get_by_name("buffersink");
    if (!source || !filter || !sink) goto fail;
    char args[128];
    snprintf(args, sizeof(args), "video_size=%dx%d:pix_fmt=%d:time_base=1/90000:pixel_aspect=1/1", width, height, AV_PIX_FMT_YUV420P);
    AVFilterContext *temporal = NULL;
    if (avfilter_graph_create_filter(&d->source, source, "camera", args, NULL, d->graph) < 0 ||
        avfilter_graph_create_filter(&temporal, filter, "denoise", "s=5", NULL, d->graph) < 0 ||
        avfilter_graph_create_filter(&d->sink, sink, "output", NULL, NULL, d->graph) < 0 ||
        avfilter_link(d->source, 0, temporal, 0) < 0 ||
        avfilter_link(temporal, 0, d->sink, 0) < 0 ||
        avfilter_graph_config(d->graph, NULL) < 0) goto fail;
    return d;
fail:
    serein_denoise_free(d);
    return NULL;
}

/* Returns 1 for output, 0 during temporal warmup, -1 on failure. Input and
 * output may alias; the graph owns its input before output is written. */
int serein_denoise_process(SereinDenoise *d, const uint8_t *input, uint8_t *output, int64_t pts, int64_t *out_pts) {
    if (!d || !input || !output || !out_pts) return -1;
    AVFrame *in = av_frame_alloc(), *out = av_frame_alloc();
    int result = -1;
    if (!in || !out) goto done;
    in->format = AV_PIX_FMT_YUV420P;
    in->width = d->width;
    in->height = d->height;
    in->pts = pts;
    if (av_frame_get_buffer(in, 32) < 0) goto done;
    size_t offset = 0;
    for (int p = 0; p < 3; p++) {
        int w = p ? d->width / 2 : d->width;
        int h = p ? d->height / 2 : d->height;
        for (int y = 0; y < h; y++)
            memcpy(in->data[p] + (size_t)y * in->linesize[p], input + offset + (size_t)y * w, w);
        offset += (size_t)w * h;
    }
    if (av_buffersrc_add_frame_flags(d->source, in, AV_BUFFERSRC_FLAG_KEEP_REF) < 0) goto done;
    int status = av_buffersink_get_frame(d->sink, out);
    if (status == AVERROR(EAGAIN)) { result = 0; goto done; }
    if (status < 0 || out->format != AV_PIX_FMT_YUV420P || out->width != d->width || out->height != d->height || out->pts == AV_NOPTS_VALUE) goto done;
    offset = 0;
    for (int p = 0; p < 3; p++) {
        int w = p ? d->width / 2 : d->width;
        int h = p ? d->height / 2 : d->height;
        for (int y = 0; y < h; y++)
            memcpy(output + offset + (size_t)y * w, out->data[p] + (size_t)y * out->linesize[p], w);
        offset += (size_t)w * h;
    }
    *out_pts = out->pts;
    result = 1;
done:
    av_frame_free(&in);
    av_frame_free(&out);
    return result;
}
