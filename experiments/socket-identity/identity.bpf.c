#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wmissing-declarations"
#include "vmlinux.h"
#pragma clang diagnostic pop

#define SEC(name) __attribute__((section(name), used))
#define INLINE __attribute__((always_inline)) inline
#define PT_REGS_PARM1(ctx) ((ctx)->di)

static void *(*bpf_map_lookup_elem)(void *, const void *) = (void *)1;
static long (*bpf_map_update_elem)(void *, const void *, const void *, __u64) = (void *)2;
static long (*bpf_map_delete_elem)(void *, const void *) = (void *)3;
static __u64 (*bpf_get_current_pid_tgid)(void) = (void *)14;
static long (*bpf_probe_read_kernel)(void *, __u32, const void *) = (void *)113;
static void *(*bpf_get_current_task_btf)(void) = (void *)158;
static long (*bpf_ringbuf_output)(void *, void *, __u64, __u64) = (void *)130;

#define READ(source, member) ({ \
    typeof((source)->member) value = {}; \
    bpf_probe_read_kernel(&value, sizeof(value), \
        (const void *)__builtin_preserve_access_index(&(source)->member)); \
    value; \
})

struct map_def {
    __u32 type, key_size, value_size, max_entries, flags;
};

struct map_def SEC("maps") events = {
    27, 0, 0, 262144, 0
};

struct map_def SEC("maps") pending = {
    1, 8, 8, 8192, 0
};


struct map_def SEC("maps") pending_bind = {
    1, 8, 8, 8192, 0
};

struct identity_event {
    __u64 socket_ptr;
    __u64 cookie;
    __u64 inode;
    __u32 pid;
    __u16 family;
    __u16 port;
    __u32 kind;
};

static INLINE struct socket *from_fd(__s64 fd) {
    if (fd < 0 || fd > 1048576)
        return 0;

    struct task_struct *task = bpf_get_current_task_btf();
    struct files_struct *files = READ(task, files);

    if (!files)
        return 0;

    struct fdtable *table = READ(files, fdt);

    if (!table || fd >= READ(table, max_fds))
        return 0;

    struct file **fds = READ(table, fd);
    struct file *file = 0;

    bpf_probe_read_kernel(&file, sizeof(file), fds + fd);

    if (!file)
        return 0;

    return READ(file, private_data);
}

SEC("tracepoint/syscalls/sys_enter_listen")
int enter_listen(struct trace_event_raw_sys_enter *ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u64 fd = ctx->args[0];

    bpf_map_update_elem(&pending, &tid, &fd, 0);
    return 0;
}

SEC("tracepoint/syscalls/sys_exit_listen")
int exit_listen(struct trace_event_raw_sys_exit *ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u64 *fd = bpf_map_lookup_elem(&pending, &tid);

    if (!fd)
        return 0;

    if (ctx->ret == 0) {
        struct socket *socket = from_fd(*fd);

        if (socket) {
            struct sock *sk = READ(socket, sk);

            if (sk) {
                struct identity_event event = {};

                event.kind = 1;
                event.socket_ptr = (__u64)sk;
                event.cookie = READ(sk, __sk_common.skc_cookie.counter);

                // struct socket is the first member of socket_alloc.
                struct socket_alloc *allocation =
                    (struct socket_alloc *)socket;
                event.inode = READ(allocation, vfs_inode.i_ino);
                event.pid = tid >> 32;
                event.family = READ(sk, __sk_common.skc_family);
                event.port = READ(sk, __sk_common.skc_num);

                bpf_ringbuf_output(
                    &events,
                    &event,
                    sizeof(event),
                    0
                );
            }
        }
    }

    bpf_map_delete_elem(&pending, &tid);
    return 0;
}


SEC("tracepoint/sock/inet_sock_set_state")
int inspect_tcp_state(struct trace_event_raw_inet_sock_set_state *ctx) {
    if (ctx->protocol != 6)
        return 0;

    if (ctx->oldstate != 10 && ctx->newstate != 10)
        return 0;

    struct identity_event event = {};

    event.kind = ctx->newstate == 10 ? 2 : 3;
    event.socket_ptr = (__u64)ctx->skaddr;
    event.pid = bpf_get_current_pid_tgid() >> 32;
    event.family = ctx->family;
    event.port = ctx->sport;

    // No inode is available directly from this tracepoint.
    // cookie remains zero.
    bpf_ringbuf_output(&events, &event, sizeof(event), 0);

    return 0;
}


SEC("tracepoint/syscalls/sys_enter_bind")
int enter_bind_udp(struct trace_event_raw_sys_enter *ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u64 fd = ctx->args[0];

    bpf_map_update_elem(&pending_bind, &tid, &fd, 0);
    return 0;
}

SEC("tracepoint/syscalls/sys_exit_bind")
int exit_bind_udp(struct trace_event_raw_sys_exit *ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u64 *fd = bpf_map_lookup_elem(&pending_bind, &tid);

    if (!fd)
        return 0;

    if (ctx->ret == 0) {
        struct socket *socket = from_fd(*fd);

        if (socket) {
            struct sock *sk = READ(socket, sk);

            if (sk) {
                __u16 protocol = READ(sk, sk_protocol);

                if (protocol == 17) {
                    struct identity_event event = {};
                    struct socket_alloc *allocation =
                        (struct socket_alloc *)socket;

                    event.kind = 4;
                    event.socket_ptr = (__u64)sk;
                    event.inode = READ(allocation, vfs_inode.i_ino);
                    event.pid = tid >> 32;
                    event.family = READ(sk, __sk_common.skc_family);
                    event.port = READ(sk, __sk_common.skc_num);

                    bpf_ringbuf_output(
                        &events, &event, sizeof(event), 0
                    );
                }
            }
        }
    }

    bpf_map_delete_elem(&pending_bind, &tid);
    return 0;
}

SEC("kprobe/__sock_release")
int release_udp(struct pt_regs *ctx) {
    struct socket *socket =
        (struct socket *)PT_REGS_PARM1(ctx);

    if (!socket)
        return 0;

    struct sock *sk = READ(socket, sk);

    if (!sk)
        return 0;

    if (READ(sk, sk_protocol) != 17)
        return 0;

    struct identity_event event = {};
    struct socket_alloc *allocation =
        (struct socket_alloc *)socket;

    event.kind = 5;
    event.socket_ptr = (__u64)sk;
    event.inode = READ(allocation, vfs_inode.i_ino);
    event.pid = bpf_get_current_pid_tgid() >> 32;
    event.family = READ(sk, __sk_common.skc_family);
    event.port = READ(sk, __sk_common.skc_num);

    bpf_ringbuf_output(&events, &event, sizeof(event), 0);
    return 0;
}

char LICENSE[] SEC("license") = "GPL";
