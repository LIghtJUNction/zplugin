// Zig implements the generated C wire protocol; no Rust runtime is linked.
const c = @import("cabi");

var active: usize = 0;
const Object = extern struct { calls: u64 };
const Task = extern struct { seed: u64, phase: u32, waker: c.zp_raw_waker };

fn view(comptime value: []const u8) c.zp_bytes {
    return .{ .ptr = value.ptr, .len = value.len };
}
fn mutable(comptime T: type, pointer: ?*anyopaque) ?*T {
    return @ptrCast(@alignCast(pointer orelse return null));
}
fn arguments(comptime T: type, pointer: ?*const anyopaque) ?*const T {
    return @ptrCast(@alignCast(pointer orelse return null));
}
fn create(out: [*c]?*anyopaque) callconv(.c) i32 {
    if (out == null) return c.ZP_INVALID_ARGUMENT;
    out.* = c.calloc(1, @sizeOf(Object));
    if (out.* == null) return c.ZP_BUSY;
    active += 1;
    return c.ZP_OK;
}
fn destroy(context: ?*anyopaque) callconv(.c) i32 {
    c.free(context);
    active -= 1;
    return c.ZP_OK;
}
fn calculate(context: ?*anyopaque, args_ptr: ?*const anyopaque, out_ptr: ?*anyopaque) callconv(.c) i32 {
    const self = mutable(Object, context) orelse return c.ZP_INVALID_ARGUMENT;
    const args = arguments(c.Foreign_calculate_args, args_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    const out = mutable(c.Foreign_calculate_out, out_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    const x: i64 = args.arg_0.x;
    const y: i64 = args.arg_0.y;
    out.* = switch (args.arg_1) {
        c.zp_foreign_2emode_v1_Sum => x + y,
        c.zp_foreign_2emode_v1_Product => x * y,
        else => return c.ZP_INVALID_ARGUMENT,
    };
    self.calls +%= 1;
    return c.ZP_OK;
}
fn transform(context: ?*anyopaque, args_ptr: ?*const anyopaque, out_ptr: ?*anyopaque) callconv(.c) i32 {
    if (context == null) return c.ZP_INVALID_ARGUMENT;
    const args = arguments(c.Foreign_transform_args, args_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    const out = mutable(c.Foreign_transform_out, out_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    if (c.zp_range_valid(args.arg_0.ptr, args.arg_0.len) == 0 or
        c.zp_range_valid(args.arg_1.ptr, args.arg_1.len) == 0) return c.ZP_INVALID_ARGUMENT;
    if (args.arg_1.len < args.arg_0.len) return c.ZP_BUFFER_TOO_SMALL;
    for (0..args.arg_0.len) |i| {
        const byte = args.arg_0.ptr[i];
        args.arg_1.ptr[i] = if (byte >= 'a' and byte <= 'z') byte - 32 else byte;
    }
    out.* = args.arg_0.len;
    return c.ZP_OK;
}
fn echo(context: ?*anyopaque, args_ptr: ?*const anyopaque, out_ptr: ?*anyopaque) callconv(.c) i32 {
    if (context == null) return c.ZP_INVALID_ARGUMENT;
    const args = arguments(c.Foreign_echo_args, args_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    const out = mutable(c.Foreign_echo_out, out_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    switch (args.arg_0.tag) {
        c.zp_foreign_2eevent_v1_Empty, c.zp_foreign_2eevent_v1_Number, c.zp_foreign_2eevent_v1_Position => {},
        else => return c.ZP_INVALID_ARGUMENT,
    }
    out.* = args.arg_0;
    return c.ZP_OK;
}
fn releaseBuffer(_: ?*anyopaque, data: [*c]const u8, _: usize) callconv(.c) i32 {
    c.free(@constCast(data));
    active -= 1;
    return c.ZP_OK;
}
fn owned(context: ?*anyopaque, args_ptr: ?*const anyopaque, out_ptr: ?*anyopaque) callconv(.c) i32 {
    if (context == null) return c.ZP_INVALID_ARGUMENT;
    const args = arguments(c.Foreign_owned_args, args_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    const out = mutable(c.Foreign_owned_out, out_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    if (c.zp_range_valid(args.arg_0.ptr, args.arg_0.len) == 0) return c.ZP_INVALID_ARGUMENT;
    const data = c.malloc(@max(args.arg_0.len, 1)) orelse return c.ZP_BUSY;
    if (args.arg_0.len != 0) _ = c.memcpy(data, args.arg_0.ptr, args.arg_0.len);
    out.* = .{ .data = @ptrCast(data), .len = args.arg_0.len, .context = null, .release = releaseBuffer };
    active += 1;
    return c.ZP_OK;
}
fn clearWaker(self: *Task) i32 {
    if (self.waker.vtable == null) return c.ZP_OK;
    const waker = self.waker;
    self.waker.vtable = null;
    return waker.vtable.*.release.?(waker.context);
}
fn taskPoll(context: ?*anyopaque, waker: [*c]const c.zp_raw_waker, out: ?*anyopaque, state: [*c]u32) callconv(.c) i32 {
    const self = mutable(Task, context) orelse return c.ZP_INVALID_ARGUMENT;
    if (waker == null or waker.*.vtable == null or out == null or state == null) return c.ZP_INVALID_ARGUMENT;
    if (self.phase == 3) return c.ZP_CANCELLED;
    if (self.phase == 2) return c.ZP_COMPLETED;
    if (self.phase == 0) {
        if (waker.*.vtable.*.size < @sizeOf(c.zp_waker_vtable) or
            waker.*.vtable.*.abi_version != c.ZP_EXTENSION_VERSION) return c.ZP_INVALID_ARGUMENT;
        const table = waker.*.vtable.*;
        if (table.retain == null or table.wake == null or table.release == null) return c.ZP_INVALID_ARGUMENT;
        const code = table.retain.?(waker.*.context);
        if (code != 0) return code;
        self.waker = waker.*;
        self.phase = 1;
        state.* = c.ZP_PENDING;
        return table.wake.?(waker.*.context);
    }
    const code = clearWaker(self);
    if (code != 0) return code;
    self.phase = 2;
    (mutable(u64, out) orelse unreachable).* = self.seed +% 1;
    state.* = c.ZP_READY;
    return c.ZP_OK;
}
fn taskCancel(context: ?*anyopaque) callconv(.c) i32 {
    const self = mutable(Task, context) orelse return c.ZP_INVALID_ARGUMENT;
    if (self.phase < 2) self.phase = 3;
    return clearWaker(self);
}
fn taskRelease(context: ?*anyopaque) callconv(.c) i32 {
    const self = mutable(Task, context) orelse return c.ZP_INVALID_ARGUMENT;
    const code = clearWaker(self);
    if (code != 0) return code;
    return destroy(context);
}
const task_table: c.zp_task_vtable = .{
    .size = @sizeOf(c.zp_task_vtable),
    .abi_version = c.ZP_EXTENSION_VERSION,
    .output_size = @sizeOf(u64),
    .output_align = @alignOf(u64),
    .output_schema = view("u64"),
    .poll = taskPoll,
    .cancel = taskCancel,
    .release = taskRelease,
};
fn start(context: ?*anyopaque, args_ptr: ?*const anyopaque, out_ptr: ?*anyopaque) callconv(.c) i32 {
    if (context == null) return c.ZP_INVALID_ARGUMENT;
    const args = arguments(c.Foreign_start_args, args_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    const out = mutable(c.Foreign_start_out, out_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    const task = mutable(Task, c.calloc(1, @sizeOf(Task))) orelse return c.ZP_BUSY;
    task.seed = args.arg_0;
    out.* = .{ .context = task, .vtable = &task_table };
    active += 1;
    return c.ZP_OK;
}
fn resourceCall(context: ?*anyopaque, operation: u32, input: c.zp_bytes, output: c.zp_bytes_mut, written: [*c]usize) callconv(.c) i32 {
    if (context == null or written == null or c.zp_range_valid(input.ptr, input.len) == 0 or
        c.zp_range_valid(output.ptr, output.len) == 0) return c.ZP_INVALID_ARGUMENT;
    if (operation != 1) return c.ZP_UNSUPPORTED;
    if (output.len < input.len) return c.ZP_BUFFER_TOO_SMALL;
    for (0..input.len) |i| output.ptr[i] = input.ptr[input.len - i - 1];
    written.* = input.len;
    return c.ZP_OK;
}
const resource_table: c.zp_resource_vtable = .{
    .size = @sizeOf(c.zp_resource_vtable),
    .abi_version = c.ZP_EXTENSION_VERSION,
    .type_id = view("foreign.reverse"),
    .resource_version = 1,
    .call = resourceCall,
    .release = destroy,
};
fn resource(context: ?*anyopaque, _: ?*const anyopaque, out_ptr: ?*anyopaque) callconv(.c) i32 {
    if (context == null) return c.ZP_INVALID_ARGUMENT;
    const out = mutable(c.Foreign_resource_out, out_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    const data = c.malloc(1) orelse return c.ZP_BUSY;
    out.* = .{ .context = data, .vtable = &resource_table };
    active += 1;
    return c.ZP_OK;
}
fn callback(context: ?*anyopaque, args_ptr: ?*const anyopaque, out_ptr: ?*anyopaque) callconv(.c) i32 {
    if (context == null) return c.ZP_INVALID_ARGUMENT;
    const args = arguments(c.Foreign_callback_args, args_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    const out = mutable(c.Foreign_callback_out, out_ptr) orelse return c.ZP_INVALID_ARGUMENT;
    const cb = args.arg_0;
    if (cb.vtable == null) return c.ZP_INVALID_ARGUMENT;
    if (cb.vtable.*.size < @sizeOf(c.zp_callback_vtable) or
        cb.vtable.*.abi_version != c.ZP_EXTENSION_VERSION) return c.ZP_INVALID_ARGUMENT;
    const table = cb.vtable.*;
    if (table.invoke == null or table.retain == null or table.release == null) return c.ZP_INVALID_ARGUMENT;
    const retained = table.retain.?(cb.context);
    if (retained != 0) return retained;
    var written: usize = 0;
    const code = table.invoke.?(cb.context, args.arg_1, args.arg_2, &written);
    const released = table.release.?(cb.context);
    if (code != 0) return code;
    if (released != 0) return released;
    if (written > args.arg_2.len) return c.ZP_INVALID_ARGUMENT;
    out.* = written;
    return c.ZP_OK;
}
fn quiesce() callconv(.c) i32 {
    return if (active == 0) c.ZP_OK else c.ZP_BUSY;
}
const methods = [_]c.zp_method{
    .{ .signature = view(c.Foreign_calculate_SIGNATURE), .call = calculate },
    .{ .signature = view(c.Foreign_echo_SIGNATURE), .call = echo },
    .{ .signature = view(c.Foreign_transform_SIGNATURE), .call = transform },
    .{ .signature = view(c.Foreign_owned_SIGNATURE), .call = owned },
    .{ .signature = view(c.Foreign_start_SIGNATURE), .call = start },
    .{ .signature = view(c.Foreign_resource_SIGNATURE), .call = resource },
    .{ .signature = view(c.Foreign_callback_SIGNATURE), .call = callback },
};
const interface: c.zp_interface_descriptor = .{
    .size = @sizeOf(c.zp_interface_descriptor),
    .version = c.Foreign_VERSION,
    .id = view(c.Foreign_ID),
    .method_count = c.Foreign_METHOD_COUNT,
    .methods = &methods,
    .create = create,
    .destroy = destroy,
};
const interfaces = [_][*c]const c.zp_interface_descriptor{&interface};
const plugin: c.zp_plugin_descriptor = .{
    .header = .{ .magic = c.ZP_MAGIC, .major = c.ZP_ABI_MAJOR, .minor = c.ZP_ABI_MINOR, .size = @sizeOf(c.zp_plugin_descriptor) },
    .plugin_version = .{ .major = 1, .minor = 0, .patch = 0 },
    .flags = c.ZP_COOPERATIVE,
    .interface_count = 1,
    .interfaces = &interfaces,
    .quiesce = quiesce,
};
export fn zplugin_entry_v1() callconv(.c) *const c.zp_header {
    return &plugin.header;
}
