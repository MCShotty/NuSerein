/* Include the actual patched AMF encoder. Only its clock, input source and
 * driver interfaces are synthetic; no GPU, account or capture is used. */
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#define av_gettime_relative fixture_time
#define av_usleep fixture_sleep
#define ff_encode_get_frame fixture_input
#define ff_get_encode_buffer fixture_packet
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wswitch"
#pragma GCC diagnostic ignored "-Wsign-compare"
#include "amfenc.c"
#pragma GCC diagnostic pop
#undef av_gettime_relative
#undef av_usleep
#undef ff_encode_get_frame
#undef ff_get_encode_buffer

typedef struct Buffer {
    AMFBuffer interface;
    int references;
    amf_size size;
    unsigned char bytes[16];
    struct Buffer *frame_reference;
    int64_t pts;
} Buffer;
typedef struct Surface {
    AMFSurface interface;
    int references;
    Buffer *frame_reference;
    int64_t pts;
} Surface;

static AMFContext driver_context;
static AMFComponent driver_encoder;
static AVFrame *original;
static AmfContext *active_context;
static Surface *driver_owned[64];
static int owned_count, live_buffers, live_surfaces, terminate_calls;
static int cancelled, cancel_after, queries, sleeps, submit_full, submit_fail;
static int property_fail, output_after, output_count;
static int input_eof;
static int64_t clock_us;

int64_t fixture_time(void) { return clock_us; }
int fixture_sleep(unsigned delay)
{
    clock_us += delay;
    sleeps++;
    if (cancel_after && sleeps >= cancel_after) cancelled = 1;
    assert(sleeps <= 15001); /* An unpatched loop fails instead of hanging CI. */
    return 0;
}
int fixture_input(AVCodecContext *codec, AVFrame *frame)
{ (void)codec; (void)frame; return input_eof ? AVERROR_EOF : AVERROR(EAGAIN); }
int fixture_packet(AVCodecContext *codec, AVPacket *packet, int64_t size, int flags)
{ (void)codec; (void)flags; assert(size > 0 && size <= 16); return av_new_packet(packet, (int)size); }
static int is_cancelled(void *opaque) { assert(opaque == &cancelled); return cancelled; }

static amf_long buffer_acquire(AMFBuffer *self)
{ return ++((Buffer *)self)->references; }
static amf_long buffer_release(AMFBuffer *self)
{
    Buffer *buffer = (Buffer *)self;
    assert(buffer->references > 0);
    if (--buffer->references) return buffer->references;
    if (buffer->frame_reference) buffer_release(&buffer->frame_reference->interface);
    live_buffers--;
    free(buffer);
    return 0;
}
static AMF_RESULT buffer_query(AMFBuffer *self, const AMFGuid *iid, void **out)
{ (void)iid; *out = self; buffer_acquire(self); return AMF_OK; }
static amf_size buffer_size(AMFBuffer *self) { return ((Buffer *)self)->size; }
static void *buffer_native(AMFBuffer *self) { return ((Buffer *)self)->bytes; }
static amf_bool buffer_has(AMFBuffer *self, const wchar_t *name)
{ return !wcscmp(name, L"av_frame_ref") && ((Buffer *)self)->frame_reference != NULL; }
static AMF_RESULT buffer_property(AMFBuffer *self, const wchar_t *name, AMFVariantStruct *value)
{
    Buffer *buffer = (Buffer *)self;
    if (!wcscmp(name, L"av_frame_ref") && buffer->frame_reference)
        return AMFVariantAssignInterface(value, (AMFInterface *)&buffer->frame_reference->interface);
    return AMFVariantAssignInt64(value, !wcscmp(name, PTS_PROP) ? buffer->pts : AMF_VIDEO_ENCODER_OUTPUT_DATA_TYPE_IDR);
}
static const AMFBufferVtbl buffer_vtable = {
    .Acquire = buffer_acquire, .Release = buffer_release, .QueryInterface = buffer_query,
    .GetSize = buffer_size, .GetNative = buffer_native, .HasProperty = buffer_has,
    .GetProperty = buffer_property,
};
static Buffer *new_buffer(amf_size size)
{
    assert(size <= 16);
    Buffer *buffer = calloc(1, sizeof(*buffer));
    assert(buffer);
    buffer->interface.pVtbl = &buffer_vtable;
    buffer->references = 1;
    buffer->size = size;
    live_buffers++;
    return buffer;
}
static amf_long surface_release(AMFSurface *self)
{
    Surface *surface = (Surface *)self;
    assert(surface->references > 0);
    if (--surface->references) return surface->references;
    if (surface->frame_reference) buffer_release(&surface->frame_reference->interface);
    live_surfaces--;
    free(surface);
    return 0;
}
static AMF_RESULT surface_set(AMFSurface *self, const wchar_t *name, AMFVariantStruct value)
{
    Surface *surface = (Surface *)self;
    assert(!wcscmp(name, L"av_frame_ref") && value.type == AMF_VARIANT_INTERFACE);
    if (property_fail) return AMF_FAIL;
    assert(!surface->frame_reference);
    surface->frame_reference = (Buffer *)value.pInterface;
    buffer_acquire(&surface->frame_reference->interface);
    return AMF_OK;
}
static amf_bool surface_has(AMFSurface *self, const wchar_t *name)
{ (void)self; (void)name; return false; }
static amf_pts surface_pts(AMFSurface *self) { return ((Surface *)self)->pts; }
static const AMFSurfaceVtbl surface_vtable = {
    .Release = surface_release, .SetProperty = surface_set,
    .HasProperty = surface_has, .GetPts = surface_pts,
};
static Surface *new_surface(void)
{
    Surface *surface = calloc(1, sizeof(*surface));
    assert(surface);
    surface->interface.pVtbl = &surface_vtable;
    surface->references = 1;
    live_surfaces++;
    return surface;
}
static void own_surface(Surface *surface)
{
    assert(owned_count < 64);
    driver_owned[owned_count++] = surface;
    surface->references++;
}
static AMF_RESULT allocate_buffer(AMFContext *self, AMF_MEMORY_TYPE type, amf_size size, AMFBuffer **out)
{ (void)self; assert(type == AMF_MEMORY_HOST); *out = &new_buffer(size)->interface; return AMF_OK; }
static AMF_RESULT terminate_context(AMFContext *self)
{
    (void)self;
    int delayed = active_context->delayed_frame && active_context->delayed_frame->buf[0];
    assert(av_buffer_get_ref_count(original->buf[0]) == 1 + delayed);
    return AMF_OK;
}
static amf_long release_context(AMFContext *self) { (void)self; return 0; }
static const AMFContextVtbl context_vtable = {
    .AllocBuffer = allocate_buffer, .Terminate = terminate_context, .Release = release_context,
};
static AMF_RESULT submit(AMFComponent *self, AMFData *data)
{
    (void)self;
    if (submit_fail) return AMF_FAIL;
    if (submit_full) return AMF_INPUT_FULL;
    own_surface((Surface *)data);
    return AMF_OK;
}
static AMF_RESULT query(AMFComponent *self, AMFData **out)
{
    (void)self;
    queries++;
    *out = NULL;
    if (output_after && queries >= output_after && owned_count) {
        Surface *surface = driver_owned[0];
        for (int i = 1; i < owned_count; ++i) driver_owned[i - 1] = driver_owned[i];
        owned_count--;
        Buffer *buffer = new_buffer(4);
        buffer->frame_reference = surface->frame_reference;
        buffer_acquire(&buffer->frame_reference->interface);
        buffer->pts = surface->pts;
        *out = (AMFData *)&buffer->interface;
        surface_release(&surface->interface);
        output_count++;
        return AMF_OK;
    }
    return AMF_REPEAT;
}
static AMF_RESULT terminate_encoder(AMFComponent *self)
{
    (void)self;
    /* Clones must stay alive until the driver releases its surface ownership. */
    if (owned_count) assert(av_buffer_get_ref_count(original->buf[0]) > 1);
    for (int i = 0; i < owned_count; ++i) surface_release(&driver_owned[i]->interface);
    owned_count = 0;
    terminate_calls++;
    return AMF_OK;
}
static amf_long release_encoder(AMFComponent *self) { (void)self; return 0; }
static AMF_RESULT drain(AMFComponent *self) { (void)self; return submit_full ? AMF_INPUT_FULL : AMF_OK; }
static const AMFComponentVtbl encoder_vtable = {
    .SubmitInput = submit, .QueryOutput = query, .Terminate = terminate_encoder,
    .Release = release_encoder, .Drain = drain,
};

static void begin(AmfContext *ctx, AVCodecContext *codec, SereinAmfInterruptV1 *control)
{
    assert(!live_buffers && !live_surfaces && !owned_count);
    memset(ctx, 0, sizeof(*ctx));
    active_context = ctx;
    memset(codec, 0, sizeof(*codec));
    cancelled = cancel_after = queries = sleeps = submit_full = submit_fail = 0;
    property_fail = output_after = output_count = terminate_calls = input_eof = 0;
    clock_us = 1000000;
    driver_context.pVtbl = &context_vtable;
    driver_encoder.pVtbl = &encoder_vtable;
    ctx->context = &driver_context;
    ctx->encoder = &driver_encoder;
    ctx->hwsurfaces_in_queue_max = 23;
    ctx->timestamp_list = av_fifo_alloc2(64, sizeof(int64_t), 0);
    ctx->delayed_frame = av_frame_alloc();
    assert(ctx->timestamp_list && ctx->delayed_frame);
    *control = (SereinAmfInterruptV1){1, sizeof(*control), is_cancelled, &cancelled};
    ctx->serein_interrupt = (uint8_t *)control;
    ctx->serein_interrupt_size = sizeof(*control);
    codec->priv_data = ctx;
    codec->codec = avcodec_find_encoder_by_name("libopenh264");
    assert(codec->codec);
    original = av_frame_alloc();
    assert(original);
    original->format = AV_PIX_FMT_YUV420P;
    original->width = original->height = 16;
    assert(av_frame_get_buffer(original, 0) == 0);
}
static void finish(AmfContext *ctx, AVCodecContext *codec)
{
    assert(ff_amf_encode_close(codec) == 0);
    assert(terminate_calls == 1 && !live_buffers && !live_surfaces && !owned_count);
    assert(av_buffer_get_ref_count(original->buf[0]) == 1);
    for (int i = 0; i < 48; ++i) assert(!ctx->serein_retained[i]);
    assert(ff_amf_encode_close(codec) == 0 && terminate_calls == 1);
    av_frame_free(&original);
}
static Surface *attach(AmfContext *ctx, int64_t pts)
{
    Surface *surface = new_surface();
    surface->pts = pts;
    assert(serein_amf_attach_frame(ctx, &surface->interface, original) == 0);
    return surface;
}
static void held_queue(AmfContext *ctx, int count, int delayed)
{
    for (int i = 0; i < count; ++i) {
        Surface *surface = attach(ctx, i);
        if (delayed && i == count - 1) {
            ctx->delayed_surface = &surface->interface;
            assert(av_frame_ref(ctx->delayed_frame, original) == 0);
        } else {
            int64_t timestamp = i;
            own_surface(surface);
            surface_release(&surface->interface);
            assert(av_fifo_write(ctx->timestamp_list, &timestamp, 1) == 0);
        }
    }
}

int main(void)
{
    AmfContext ctx;
    AVCodecContext codec;
    SereinAmfInterruptV1 control;
    Surface *surface;
    AVPacket *packet = av_packet_alloc();
    assert(packet);
    /* Actual AMF_INPUT_FULL retry loop: cancellation is observed on its next poll. */
    begin(&ctx, &codec, &control);
    held_queue(&ctx, 17, 1);
    submit_full = 1; cancel_after = 3;
    assert(ff_amf_receive_packet(&codec, packet) == AVERROR_EXIT);
    assert(sleeps == 3 && queries == 3 && !packet->size);
    finish(&ctx, &codec);

    /* The hardware threshold can also wait forever without AMF_INPUT_FULL. */
    begin(&ctx, &codec, &control);
    held_queue(&ctx, 23, 1);
    assert(ff_amf_receive_packet(&codec, packet) == AVERROR(ETIMEDOUT));
    assert(sleeps == 15000 && clock_us == 16000000 && !packet->size);
    finish(&ctx, &codec);

    /* Both delayed Drain and incomplete EOF also use the same cancellable wait. */
    for (int eof = 0; eof <= 1; ++eof) {
        begin(&ctx, &codec, &control);
        input_eof = 1;
        ctx.eof = eof;
        ctx.delayed_drain = !eof;
        submit_full = !eof;
        cancel_after = 2;
        assert(ff_amf_receive_packet(&codec, packet) == AVERROR_EXIT);
        assert(sleeps == 2 && !packet->size);
        finish(&ctx, &codec);
    }

    /* Ordinary lack of input remains EAGAIN, without starting an idle deadline. */
    begin(&ctx, &codec, &control);
    assert(ff_amf_receive_packet(&codec, packet) == AVERROR(EAGAIN));
    assert(!sleeps && !ctx.serein_progress_deadline);
    clock_us += 60000000;
    held_queue(&ctx, 23, 1);
    output_after = 3;
    assert(ff_amf_receive_packet(&codec, packet) == 0);
    assert(packet->size == 4 && packet->pts == 0 && output_count == 1);
    assert(ctx.serein_progress_deadline == 0 && ctx.hwsurfaces_in_queue == 22);
    av_packet_unref(packet);
    finish(&ctx, &codec);

    /* A static screen retains lookahead pictures while the caller is idle.
     * A prior EAGAIN must not expire that healthy session before the next query. */
    begin(&ctx, &codec, &control);
    held_queue(&ctx, 17, 1);
    assert(ff_amf_receive_packet(&codec, packet) == AVERROR(EAGAIN));
    assert(ctx.serein_progress_deadline > clock_us && !sleeps);
    clock_us += 60000000;
    surface = attach(&ctx, 17);
    ctx.delayed_surface = &surface->interface;
    assert(av_frame_ref(ctx.delayed_frame, original) == 0);
    output_after = queries + 1;
    assert(ff_amf_receive_packet(&codec, packet) == 0 && packet->size == 4);
    assert(packet->pts == 0 && !sleeps && output_count == 1);
    av_packet_unref(packet);
    finish(&ctx, &codec);

    /* Rejected native input still owns queued references until close. */
    begin(&ctx, &codec, &control);
    held_queue(&ctx, 17, 1);
    submit_fail = 1;
    assert(ff_amf_receive_packet(&codec, packet) < 0);
    finish(&ctx, &codec);

    /* Property copying failure and the 48-frame cap release the extra clone. */
    begin(&ctx, &codec, &control);
    surface = new_surface();
    property_fail = 1;
    assert(serein_amf_attach_frame(&ctx, &surface->interface, original) < 0);
    assert(av_buffer_get_ref_count(original->buf[0]) == 1 && !live_buffers);
    surface_release(&surface->interface);
    property_fail = 0;
    held_queue(&ctx, 48, 0);
    surface = new_surface();
    assert(serein_amf_attach_frame(&ctx, &surface->interface, original) < 0);
    assert(av_buffer_get_ref_count(original->buf[0]) == 49 && live_buffers == 48);
    surface_release(&surface->interface);
    finish(&ctx, &codec);

    /* Bad ABI records fail before invoking their pointer or the driver. */
    begin(&ctx, &codec, &control);
    control.version = 2;
    assert(ff_amf_receive_packet(&codec, packet) == AVERROR(EINVAL) && !queries);
    control.version = 1; ctx.serein_interrupt_size--;
    assert(ff_amf_receive_packet(&codec, packet) == AVERROR(EINVAL) && !queries);
    finish(&ctx, &codec);
    av_packet_free(&packet);
    puts("Actual patched AMF loop: cancellation, 15s deadline, idle/progress, rejected input, 48-frame ownership and repeated close passed");
    return 0;
}
