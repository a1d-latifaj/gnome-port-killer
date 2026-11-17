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
static __u64 (*bpf_ktime_get_ns)(void) = (void *)5;
static __u64 (*bpf_get_socket_cookie)(void *) = (void *)46;
static long (*bpf_probe_read_kernel)(void *, __u32, const void *) = (void *)113;
static long (*bpf_ringbuf_output)(void *, void *, __u64, __u64) = (void *)130;

struct map_def { __u32 type, key_size, value_size, max_entries, flags; };
struct map_def SEC("maps") events = {27, 0, 0, 262144, 0};
struct map_def SEC("maps") lifecycle_events = {27, 0, 0, 262144, 0};

struct socket_event {
    __u64 socket_id;
    __u64 timestamp_ns;
    __u32 pid;
    __u16 event_type;
    __u16 family;
    __u16 protocol;
    __u16 local_port;
    __u16 version;
    __u16 reserved;
};
struct map_def SEC("maps") lost = {2, 4, 8, 1, 0};
struct udp_socket_metadata {
    __u64 socket_id;
    __u32 pid;
    __u16 family;
    __u16 local_port;
};

struct map_def SEC("maps") udp_tracked = {
    1, 8, sizeof(struct udp_socket_metadata), 16384, 0
};

struct tcp_socket_metadata {
    __u64 socket_id;
    __u32 pid;
    __u16 family;
    __u16 local_port;
};

struct map_def SEC("maps") tcp_tracked = {
    1, 8, sizeof(struct tcp_socket_metadata), 16384, 0
};

enum { TCP = 1, UDP = 2, UNIX = 3 };

static INLINE void record_loss(void) {
    __u32 zero = 0;
    __u64 *counter = bpf_map_lookup_elem(&lost, &zero);
    if (counter)
        __sync_fetch_and_add(counter, 1);
}
static INLINE void emit(__u32 proto) {
    // Deliberately contains no identity or socket metadata on the shared channel.
    struct { __u32 proto; __u32 reserved; } event = {proto, 0};
    if (bpf_ringbuf_output(&events, &event, sizeof(event), 0)) {
        __u32 zero = 0;
        __u64 *counter = bpf_map_lookup_elem(&lost, &zero);
        if (counter) __sync_fetch_and_add(counter, 1);
    }
}

static INLINE void emit_lifecycle(
    __u64 socket_id,
    __u32 pid,
    __u16 event_type,
    __u16 family,
    __u16 protocol,
    __u16 local_port
) {
    struct socket_event event = {
        .socket_id = socket_id,
        .timestamp_ns = bpf_ktime_get_ns(),
        .pid = pid,
        .event_type = event_type,
        .family = family,
        .protocol = protocol,
        .local_port = local_port,
        .version = 2,
        .reserved = 0,
    };

    if (bpf_ringbuf_output(
            &lifecycle_events, &event, sizeof(event), 0)) {
        __u32 zero = 0;
        __u64 *counter = bpf_map_lookup_elem(&lost, &zero);
        if (counter)
            __sync_fetch_and_add(counter, 1);
    }
}

static INLINE int handle_udp_bind(struct socket *socket, int ret) {
    if (ret != 0 || !socket)
        return 0;

    struct sock *sk = socket->sk;
    if (!sk || sk->sk_protocol != 17)
        return 0;

    __u16 family = READ(sk, __sk_common.skc_family);
    if (family != 2 && family != 10)
        return 0;

    __u16 port = READ(sk, __sk_common.skc_num);
    if (!port)
        return 0;

    __u64 socket_id = bpf_get_socket_cookie(sk);
    if (!socket_id) {
        record_loss();
        return 0;
    }

    __u64 key = (__u64)sk;
    struct udp_socket_metadata metadata = {
        .socket_id = socket_id,
        .pid = (__u32)(bpf_get_current_pid_tgid() >> 32),
        .family = family,
        .local_port = port,
    };

    if (bpf_map_update_elem(&udp_tracked, &key, &metadata, 0)) {
        record_loss();
        return 0;
    }

    emit_lifecycle(
        metadata.socket_id,
        metadata.pid,
        3,
        metadata.family,
        17,
        metadata.local_port
    );
    emit(UDP);
    return 0;
}

SEC("fexit/inet_bind")
int udp_bind4(__u64 *ctx) {
    return handle_udp_bind((struct socket *)ctx[0], (int)ctx[3]);
}

SEC("fexit/inet6_bind")
int udp_bind6(__u64 *ctx) {
    return handle_udp_bind((struct socket *)ctx[0], (int)ctx[3]);
}

SEC("fexit/inet_listen")
int tcp_listen(__u64 *ctx) {
    struct socket *socket = (struct socket *)ctx[0];
    int ret = (int)ctx[2];

    if (ret != 0 || !socket)
        return 0;

    struct sock *sk = socket->sk;
    if (!sk || sk->sk_protocol != 6)
        return 0;

    __u16 family = READ(sk, __sk_common.skc_family);
    if (family != 2 && family != 10)
        return 0;

    if (READ(sk, __sk_common.skc_state) != 10)
        return 0;

    __u16 port = READ(sk, __sk_common.skc_num);
    if (!port)
        return 0;

    __u64 socket_id = bpf_get_socket_cookie(sk);
    if (!socket_id) {
        record_loss();
        return 0;
    }

    __u64 key = (__u64)sk;
    struct tcp_socket_metadata metadata = {
        .socket_id = socket_id,
        .pid = (__u32)(bpf_get_current_pid_tgid() >> 32),
        .family = family,
        .local_port = port,
    };

    if (bpf_map_update_elem(&tcp_tracked, &key, &metadata, 0)) {
        record_loss();
        return 0;
    }

    emit_lifecycle(
        metadata.socket_id,
        metadata.pid,
        1,
        metadata.family,
        6,
        metadata.local_port
    );

    return 0;
}

SEC("tracepoint/sock/inet_sock_set_state")
int tcp_state(struct trace_event_raw_inet_sock_set_state *ctx) {
    __u32 oldstate = ctx->oldstate;
    __u32 newstate = ctx->newstate;

    if (ctx->protocol != 6)
        return 0;

    if (oldstate == 10 && newstate != 10) {
        __u64 key = (__u64)ctx->skaddr;
        struct sock *sk = (struct sock *)ctx->skaddr;
        // Tracepoint programs cannot call bpf_get_socket_cookie. SOCK_DIAG
        // materializes skc_cookie for snapshot-origin listeners, so a CO-RE
        // read provides the same authoritative SO_COOKIE identity here.
        __u64 socket_id = READ(sk, __sk_common.skc_cookie.counter);

        struct tcp_socket_metadata *tracked =
            bpf_map_lookup_elem(&tcp_tracked, &key);

        if (tracked) {
            if (socket_id && socket_id != ~0ULL &&
                tracked->socket_id == socket_id) {
                emit_lifecycle(
                    tracked->socket_id,
                    tracked->pid,
                    2,
                    tracked->family,
                    6,
                    tracked->local_port
                );
            } else {
                record_loss();
            }

            if (bpf_map_delete_elem(&tcp_tracked, &key))
                record_loss();
        } else if (socket_id && socket_id != ~0ULL && ctx->sport &&
                   (ctx->family == 2 || ctx->family == 10)) {
            // Listeners that predate probe attachment are discovered by the
            // initial SOCK_DIAG snapshot and have no tcp_tracked entry.
            emit_lifecycle(socket_id, 0, 2, ctx->family, 6, ctx->sport);
        } else {
            record_loss();
        }
    }

    if (oldstate == 10 || newstate == 10)
        emit(TCP);

    return 0;
}
SEC("fentry/__sock_release")
int release_socket(__u64 *ctx) {
    struct socket *socket = (struct socket *)ctx[0];
    if (!socket)
        return 0;
    struct sock *sk = socket->sk;
    if (!sk) return 0;
    __u16 family = READ(sk, __sk_common.skc_family);
    __u16 type = READ(socket, type);
    if ((family == 2 || family == 10) && type == 2) {
        __u64 key = (__u64)sk;
        struct udp_socket_metadata *tracked =
            bpf_map_lookup_elem(&udp_tracked, &key);

        if (tracked) {
            __u64 socket_id = bpf_get_socket_cookie(sk);

            if (socket_id && socket_id != ~0ULL &&
                tracked->socket_id == socket_id) {
                emit_lifecycle(
                    tracked->socket_id,
                    tracked->pid,
                    4,
                    tracked->family,
                    17,
                    tracked->local_port
                );
                emit(UDP);
            } else {
                record_loss();
            }

            if (bpf_map_delete_elem(&udp_tracked, &key))
                record_loss();
        } else {
            // Snapshot-origin UDP sockets predate udp_tracked. Emitting their
            // cookie is safe: userspace ignores closes for unknown cookies.
            __u16 port = READ(sk, __sk_common.skc_num);
            __u64 socket_id = bpf_get_socket_cookie(sk);
            if (socket_id && port) {
                emit_lifecycle(socket_id, 0, 4, family, 17, port);
                emit(UDP);
            }
        }
    }
    if ((family == 2 || family == 10) && type == 1) {
        __u64 key = (__u64)sk;

        struct tcp_socket_metadata *tracked =
            bpf_map_lookup_elem(&tcp_tracked, &key);

        if (tracked) {
            __u64 socket_id = bpf_get_socket_cookie(sk);

            if (socket_id && socket_id != ~0ULL &&
                tracked->socket_id == socket_id) {
                emit_lifecycle(
                    tracked->socket_id,
                    tracked->pid,
                    2,
                    tracked->family,
                    6,
                    tracked->local_port
                );
                emit(TCP);
            } else {
                record_loss();
            }

            if (bpf_map_delete_elem(&tcp_tracked, &key))
                record_loss();
        }
    }

    if (family == 1 && (READ(sk, __sk_common.skc_state) == 10 ||
        (type == 2 && READ((struct unix_sock *)sk, addr) != 0))) emit(UNIX);
    return 0;
}
char LICENSE[] SEC("license") = "GPL";
