// Fixed, read-only CO-RE probes. Compiled once; relocated against runtime BTF by Aya.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wmissing-declarations"
#include "vmlinux.h"
#pragma clang diagnostic pop
#define SEC(name) __attribute__((section(name), used))
#define INLINE __attribute__((always_inline)) inline
#define READ(source, member) ({ \
    typeof((source)->member) value = {}; \
    bpf_probe_read_kernel((void *)&value, sizeof(value), \
        (const void *)__builtin_preserve_access_index(&(source)->member)); value; })

static void *(*bpf_map_lookup_elem)(void *, const void *) = (void *)1;
static long (*bpf_map_update_elem)(void *, const void *, const void *, __u64) = (void *)2;
static long (*bpf_map_delete_elem)(void *, const void *) = (void *)3;
static __u64 (*bpf_get_current_pid_tgid)(void) = (void *)14;
static long (*bpf_probe_read_kernel)(void *, __u32, const void *) = (void *)113;
static void *(*bpf_get_current_task_btf)(void) = (void *)158;
static long (*bpf_ringbuf_output)(void *, void *, __u64, __u64) = (void *)130;

struct map_def { __u32 type, key_size, value_size, max_entries, flags; };
struct map_def SEC("maps") events = {27, 0, 0, 262144, 0};
struct map_def SEC("maps") lost = {2, 4, 8, 1, 0};
struct map_def SEC("maps") pending = {1, 8, 8, 8192, 0};
struct map_def SEC("maps") udp_tracked = {1, 8, 8, 16384, 0};

enum { TCP = 1, UDP = 2, UNIX = 3 };
static INLINE void emit(__u32 proto) {
    // Deliberately contains no identity or socket metadata on the shared channel.
    struct { __u32 proto; __u32 reserved; } event = {proto, 0};
    if (bpf_ringbuf_output(&events, &event, sizeof(event), 0)) {
        __u32 zero = 0;
        __u64 *counter = bpf_map_lookup_elem(&lost, &zero);
        if (counter) __sync_fetch_and_add(counter, 1);
    }
}

static INLINE struct socket *from_fd(__s64 fd) {
    if (fd < 0 || fd > 1048576) return 0;
    struct task_struct *task = bpf_get_current_task_btf();
    struct files_struct *files = READ(task, files);
    if (!files) return 0;
    struct fdtable *table = READ(files, fdt);
    if (!table || fd >= READ(table, max_fds)) return 0;
    struct file **fds = READ(table, fd);
    struct file *file = 0;
    bpf_probe_read_kernel(&file, sizeof(file), fds + fd);
    if (!file) return 0;
    return READ(file, private_data); // Only used after successful socket syscalls.
}

SEC("tracepoint/syscalls/sys_enter_bind")
int enter_bind(struct trace_event_raw_sys_enter *ctx) {
    __u64 tid = bpf_get_current_pid_tgid(), fd = ctx->args[0];
    bpf_map_update_elem(&pending, &tid, &fd, 0);
    return 0;
}
SEC("tracepoint/syscalls/sys_exit_bind")
int exit_bind(struct trace_event_raw_sys_exit *ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u64 *fd = bpf_map_lookup_elem(&pending, &tid);
    if (fd && ctx->ret == 0) {
        struct socket *socket = from_fd(*fd);
        if (socket) {
            struct sock *sk = READ(socket, sk);
            if (sk && READ(socket, type) == 2) {
                __u16 family = READ(sk, __sk_common.skc_family);
                if (family == 1) emit(UNIX);
                else if ((family == 2 || family == 10) &&
                         READ(sk, __sk_common.skc_num) > 0) {
                    __u64 key = (__u64)sk;
                    __u64 inode = READ(((struct socket_alloc *)socket), vfs_inode.i_ino);
                    if (bpf_map_update_elem(&udp_tracked, &key, &inode, 0) == 0)
                        emit(UDP);
                }
            }
        }
    }
    bpf_map_delete_elem(&pending, &tid);
    return 0;
}
// Listen FD is stored separately so failed bind/listen calls cannot mix state.
SEC("tracepoint/syscalls/sys_enter_listen")
int enter_listen(struct trace_event_raw_sys_enter *ctx) {
    __u64 tid = bpf_get_current_pid_tgid(), fd = ctx->args[0];
    bpf_map_update_elem(&pending, &tid, &fd, 0);
    return 0;
}
SEC("tracepoint/syscalls/sys_exit_listen")
int exit_listen(struct trace_event_raw_sys_exit *ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u64 *fd = bpf_map_lookup_elem(&pending, &tid);
    if (fd && ctx->ret == 0) {
        struct socket *socket = from_fd(*fd);
        if (socket) {
            struct sock *sk = READ(socket, sk);
            if (sk && READ(sk, __sk_common.skc_family) == 1) emit(UNIX);
        }
    }
    bpf_map_delete_elem(&pending, &tid);
    return 0;
}
SEC("tracepoint/sock/inet_sock_set_state")
int tcp_state(struct trace_event_raw_inet_sock_set_state *ctx) {
    if (ctx->protocol == 6 && (ctx->oldstate == 10 || ctx->newstate == 10)) emit(TCP);
    return 0;
}
SEC("kprobe/__sock_release")
int release_socket(struct pt_regs *ctx) {
#if defined(__TARGET_ARCH_x86)
    struct socket *socket = (void *)ctx->di;
#elif defined(__TARGET_ARCH_arm64)
    struct socket *socket = (void *)ctx->regs[0];
#endif
    struct sock *sk = READ(socket, sk);
    if (!sk) return 0;
    __u16 family = READ(sk, __sk_common.skc_family);
    __u16 type = READ(socket, type);
    if ((family == 2 || family == 10) && type == 2) {
        __u64 key = (__u64)sk;
        __u64 *tracked_inode = bpf_map_lookup_elem(&udp_tracked, &key);
        if (tracked_inode) {
            __u64 inode = READ(((struct socket_alloc *)socket), vfs_inode.i_ino);
            if (*tracked_inode == inode)
                emit(UDP);
            bpf_map_delete_elem(&udp_tracked, &key);
        }
    }
    if (family == 1 && (READ(sk, __sk_common.skc_state) == 10 ||
        (type == 2 && READ((struct unix_sock *)sk, addr) != 0))) emit(UNIX);
    return 0;
}
SEC("tracepoint/sched/sched_process_exit")
int process_exit(void *ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    bpf_map_delete_elem(&pending, &tid);
    return 0;
}
char LICENSE[] SEC("license") = "GPL";
