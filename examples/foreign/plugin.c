/* C11 plugin. All wire records and signatures come from the Rust interface. */
#include "foreign.h"
#include <stdlib.h>
#include <string.h>

#ifdef __cplusplus
#define ZP_ALIGNOF(T) alignof(T)
#else
#define ZP_ALIGNOF(T) _Alignof(T)
#endif

typedef struct object { uint64_t calls; int poisoned; } object;
static size_t active;
static int32_t checked_object(const void *context) {
    if (!context) return ZP_INVALID_ARGUMENT;
    return ((const object *)context)->poisoned ? ZP_POISONED : ZP_OK;
}

static int32_t ZP_CALL create(void **out) {
    if (!out) return ZP_INVALID_ARGUMENT;
    *out = calloc(1, sizeof(object));
    if (!*out) return ZP_BUSY;
    ++active;
    return ZP_OK;
}
static int32_t ZP_CALL destroy(void *context) {
    free(context);
    --active;
    return ZP_OK;
}
static int32_t ZP_CALL calculate(void *context, const void *arguments, void *output) {
    object *self = (object *)context;
    const Foreign_calculate_args *args = (const Foreign_calculate_args *)arguments;
    if (!self || !args || !output) return ZP_INVALID_ARGUMENT;
    if (self->poisoned) return ZP_POISONED;
#ifdef __cplusplus
    try {
        /* C++ exceptions are contained inside the exported C boundary. */
        if (args->arg_0.x == -99) throw 42;
#endif
        int64_t value;
        if (args->arg_1 == zp_foreign_2emode_v1_Sum)
            value = (int64_t)args->arg_0.x + args->arg_0.y;
        else if (args->arg_1 == zp_foreign_2emode_v1_Product)
            value = (int64_t)args->arg_0.x * args->arg_0.y;
        else return ZP_INVALID_ARGUMENT;
        *(Foreign_calculate_out *)output = value;
        ++self->calls;
        return ZP_OK;
#ifdef __cplusplus
    } catch (...) {
        self->poisoned = 1;
        return ZP_PANICKED;
    }
#endif
}
static int32_t ZP_CALL transform(void *context, const void *arguments, void *output) {
    int32_t state = checked_object(context);
    if (state) return state;
    const Foreign_transform_args *args = (const Foreign_transform_args *)arguments;
    if (!context || !args || !output || !zp_range_valid(args->arg_0.ptr, args->arg_0.len)
        || !zp_range_valid(args->arg_1.ptr, args->arg_1.len)) return ZP_INVALID_ARGUMENT;
    if (args->arg_1.len < args->arg_0.len) return ZP_BUFFER_TOO_SMALL;
    for (size_t i = 0; i < args->arg_0.len; ++i) {
        uint8_t byte = args->arg_0.ptr[i];
        args->arg_1.ptr[i] = byte >= 'a' && byte <= 'z' ? (uint8_t)(byte - 32) : byte;
    }
    *(Foreign_transform_out *)output = (uint64_t)args->arg_0.len;
    return ZP_OK;
}
static int32_t ZP_CALL echo(void *context, const void *arguments, void *output) {
    int32_t state = checked_object(context);
    if (state) return state;
    const Foreign_echo_args *args = (const Foreign_echo_args *)arguments;
    if (!context || !args || !output) return ZP_INVALID_ARGUMENT;
    switch (args->arg_0.tag) {
    case zp_foreign_2eevent_v1_Empty:
    case zp_foreign_2eevent_v1_Number:
    case zp_foreign_2eevent_v1_Position:
        *(Foreign_echo_out *)output = args->arg_0;
        return ZP_OK;
    default: return ZP_INVALID_ARGUMENT;
    }
}
static int32_t ZP_CALL release_buffer(void *context, const uint8_t *data, size_t len) {
    (void)context;
    (void)len;
    free((void *)data);
    --active;
    return ZP_OK;
}
static int32_t ZP_CALL owned(void *context, const void *arguments, void *output) {
    int32_t state = checked_object(context);
    if (state) return state;
    const Foreign_owned_args *args = (const Foreign_owned_args *)arguments;
    if (!context || !args || !output || !zp_range_valid(args->arg_0.ptr, args->arg_0.len))
        return ZP_INVALID_ARGUMENT;
    uint8_t *data = (uint8_t *)malloc(args->arg_0.len ? args->arg_0.len : 1);
    if (!data) return ZP_BUSY;
    if (args->arg_0.len) memcpy(data, args->arg_0.ptr, args->arg_0.len);
    zp_raw_buffer result = {data, args->arg_0.len, NULL, release_buffer};
    *(Foreign_owned_out *)output = result;
    ++active;
    return ZP_OK;
}

typedef struct task {
    uint64_t seed;
    uint32_t phase; /* 0 new, 1 pending, 2 ready, 3 cancelled */
    zp_raw_waker waker;
} task;
static int32_t clear_waker(task *self) {
    if (!self->waker.vtable) return ZP_OK;
    zp_raw_waker waker = self->waker;
    self->waker.vtable = NULL;
    return waker.vtable->release(waker.context);
}
static int32_t ZP_CALL task_poll(void *context, const zp_raw_waker *waker, void *out, uint32_t *state) {
    task *self = (task *)context;
    if (!self || !waker || !waker->vtable || !out || !state) return ZP_INVALID_ARGUMENT;
    if (self->phase == 3) return ZP_CANCELLED;
    if (self->phase == 2) return ZP_COMPLETED;
    if (self->phase == 0) {
        if (waker->vtable->size < sizeof(zp_waker_vtable)
            || waker->vtable->abi_version != ZP_EXTENSION_VERSION
            || !waker->vtable->retain || !waker->vtable->wake || !waker->vtable->release)
            return ZP_INVALID_ARGUMENT;
        int32_t code = waker->vtable->retain(waker->context);
        if (code) return code;
        self->waker = *waker;
        self->phase = 1;
        *state = ZP_PENDING;
        /* A wake before returning Pending is valid: the executor must repoll.
         * Keeping a retained waker demonstrates that it outlives this poll. */
        return self->waker.vtable->wake(self->waker.context);
    }
    int32_t code = clear_waker(self);
    if (code) return code;
    self->phase = 2;
    *(uint64_t *)out = self->seed + 1;
    *state = ZP_READY;
    return ZP_OK;
}
static int32_t ZP_CALL task_cancel(void *context) {
    task *self = (task *)context;
    if (self->phase < 2) self->phase = 3;
    return clear_waker(self);
}
static int32_t ZP_CALL task_release(void *context) {
    task *self = (task *)context;
    int32_t code = clear_waker(self);
    if (code) return code;
    free(self);
    --active;
    return ZP_OK;
}
static const zp_task_vtable task_vtable = {
    sizeof(zp_task_vtable), ZP_EXTENSION_VERSION, sizeof(uint64_t), ZP_ALIGNOF(uint64_t),
    ZP_LITERAL("u64"), task_poll, task_cancel, task_release
};
static int32_t ZP_CALL start(void *context, const void *arguments, void *output) {
    int32_t state = checked_object(context);
    if (state) return state;
    const Foreign_start_args *args = (const Foreign_start_args *)arguments;
    if (!context || !args || !output) return ZP_INVALID_ARGUMENT;
    task *value = (task *)calloc(1, sizeof(task));
    if (!value) return ZP_BUSY;
    value->seed = args->arg_0;
    zp_raw_task result = {value, &task_vtable};
    *(Foreign_start_out *)output = result;
    ++active;
    return ZP_OK;
}

static int32_t ZP_CALL resource_call(void *context, uint32_t operation, zp_bytes input,
                                   zp_bytes_mut output, size_t *written) {
    if (!context || !written || !zp_range_valid(input.ptr, input.len)
        || !zp_range_valid(output.ptr, output.len)) return ZP_INVALID_ARGUMENT;
    if (operation != 1) return ZP_UNSUPPORTED;
    if (output.len < input.len) return ZP_BUFFER_TOO_SMALL;
    for (size_t i = 0; i < input.len; ++i) output.ptr[i] = input.ptr[input.len - 1 - i];
    *written = input.len;
    return ZP_OK;
}
static const zp_resource_vtable resource_vtable = {
    sizeof(zp_resource_vtable), ZP_EXTENSION_VERSION, ZP_LITERAL("foreign.reverse"), 1,
    resource_call, destroy
};
static int32_t ZP_CALL resource(void *context, const void *arguments, void *output) {
    int32_t state = checked_object(context);
    if (state) return state;
    (void)arguments;
    if (!context || !output) return ZP_INVALID_ARGUMENT;
    void *value = malloc(1);
    if (!value) return ZP_BUSY;
    zp_raw_resource result = {value, &resource_vtable};
    *(Foreign_resource_out *)output = result;
    ++active;
    return ZP_OK;
}
static int32_t ZP_CALL callback(void *context, const void *arguments, void *output) {
    int32_t state = checked_object(context);
    if (state) return state;
    const Foreign_callback_args *args = (const Foreign_callback_args *)arguments;
    if (!context || !args || !output) return ZP_INVALID_ARGUMENT;
    const zp_raw_callback *cb = &args->arg_0;
    if (!cb->vtable || cb->vtable->size < sizeof(zp_callback_vtable)
        || cb->vtable->abi_version != ZP_EXTENSION_VERSION || !cb->vtable->invoke
        || !cb->vtable->retain || !cb->vtable->release) return ZP_INVALID_ARGUMENT;
    int32_t code = cb->vtable->retain(cb->context);
    if (code) return code;
    size_t written = 0;
    code = cb->vtable->invoke(cb->context, args->arg_1, args->arg_2, &written);
    int32_t released = cb->vtable->release(cb->context);
    if (code) return code;
    if (released) return released;
    if (written > args->arg_2.len) return ZP_INVALID_ARGUMENT;
    *(Foreign_callback_out *)output = (uint64_t)written;
    return ZP_OK;
}
static int32_t ZP_CALL quiesce(void) { return active == 0 ? ZP_OK : ZP_BUSY; }
static const zp_method methods[] = {
    Foreign_calculate_METHOD(calculate), Foreign_echo_METHOD(echo), Foreign_transform_METHOD(transform),
    Foreign_owned_METHOD(owned), Foreign_start_METHOD(start),
    Foreign_resource_METHOD(resource), Foreign_callback_METHOD(callback)
};
static const zp_interface_descriptor interface = Foreign_INTERFACE_INIT(methods, create, destroy);
static const zp_interface_descriptor *const interfaces[] = {&interface};
static const zp_plugin_descriptor plugin = {
    ZP_HEADER_INIT, {1, 0, 0}, ZP_COOPERATIVE, 1, interfaces, quiesce
};
ZP_EXTERN ZP_EXPORT const zp_header *ZP_CALL zplugin_entry_v1(void) { return &plugin.header; }
