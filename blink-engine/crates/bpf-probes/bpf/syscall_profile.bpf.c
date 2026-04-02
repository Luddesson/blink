/* SPDX-License-Identifier: GPL-2.0 OR BSD-2-Clause */
/*
 * syscall_profile.bpf.c — Syscall latency profiler for Blink engine.
 *
 * Attaches to:
 *   raw_tracepoint/sys_enter — records syscall entry timestamp
 *   raw_tracepoint/sys_exit  — computes syscall duration
 *
 * Profiles:  send/sendto (44), recv/recvfrom (45), epoll_wait (232) on x86_64
 * Filters:   Target PID only
 * Output:    syscall_event via ring buffer
 * Histogram: µs buckets [1, 2, 5, 10, 50, 100, 500, 1000]
 *
 * Compile (CO-RE):
 *   clang -O2 -g -target bpf -D__TARGET_ARCH_x86 \
 *         -I/path/to/vmlinux.h -c syscall_profile.bpf.c -o syscall_profile.bpf.o
 */

#include "common.h"

/* ─── Syscall numbers (x86_64) ─────────────────────────────────────────── */

#define SYS_SENDTO      44
#define SYS_RECVFROM    45
#define SYS_EPOLL_WAIT  232

/* ─── Maps ─────────────────────────────────────────────────────────────── */

/* Ring buffer for syscall latency events. */
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, RINGBUF_SIZE);
} syscall_events SEC(".maps");

/* Per-task syscall entry timestamp: (pid << 32 | syscall_nr) → ktime_ns. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __type(key, __u64);
    __type(value, __u64);
    __uint(max_entries, 4096);
} syscall_start SEC(".maps");

/* Target PID to filter on. */
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __type(key, __u32);
    __type(value, __u32);
    __uint(max_entries, 1);
} target_pid SEC(".maps");

/* ─── Helpers ──────────────────────────────────────────────────────────── */

static __always_inline int is_target_pid(__u32 pid)
{
    __u32 key = 0;
    __u32 *target = bpf_map_lookup_elem(&target_pid, &key);
    if (!target)
        return 0;
    return pid == *target;
}

static __always_inline int is_tracked_syscall(__u64 nr)
{
    return nr == SYS_SENDTO || nr == SYS_RECVFROM || nr == SYS_EPOLL_WAIT;
}

/* Composite key: upper 32 bits = PID, lower 32 bits = syscall number. */
static __always_inline __u64 make_key(__u32 pid, __u64 syscall_nr)
{
    return ((__u64)pid << 32) | (syscall_nr & 0xFFFFFFFF);
}

/* ─── raw_tracepoint/sys_enter ─────────────────────────────────────────── */
/*
 * struct bpf_raw_tracepoint_args for sys_enter:
 *   args[0] = struct pt_regs *
 *   args[1] = long syscall_nr
 */

SEC("raw_tp/sys_enter")
int sys_enter_probe(struct bpf_raw_tracepoint_args *ctx)
{
    __u64 syscall_nr = ctx->args[1];
    if (!is_tracked_syscall(syscall_nr))
        return 0;

    __u32 pid = (__u32)(bpf_get_current_pid_tgid() >> 32);
    if (!is_target_pid(pid))
        return 0;

    __u64 ts  = bpf_ktime_get_ns();
    __u64 key = make_key(pid, syscall_nr);
    bpf_map_update_elem(&syscall_start, &key, &ts, BPF_ANY);

    return 0;
}

/* ─── raw_tracepoint/sys_exit ──────────────────────────────────────────── */
/*
 * struct bpf_raw_tracepoint_args for sys_exit:
 *   args[0] = struct pt_regs *
 *   args[1] = long ret
 *
 * We need to recover the syscall number from pt_regs->orig_ax (x86_64).
 */

SEC("raw_tp/sys_exit")
int sys_exit_probe(struct bpf_raw_tracepoint_args *ctx)
{
    __u32 pid = (__u32)(bpf_get_current_pid_tgid() >> 32);
    if (!is_target_pid(pid))
        return 0;

    /* Recover syscall number from pt_regs. */
    struct pt_regs *regs = (struct pt_regs *)ctx->args[0];
    __u64 syscall_nr = BPF_CORE_READ(regs, orig_ax);

    if (!is_tracked_syscall(syscall_nr))
        return 0;

    __u64 key = make_key(pid, syscall_nr);
    __u64 *start_ts = bpf_map_lookup_elem(&syscall_start, &key);
    if (!start_ts)
        return 0;

    __u64 now = bpf_ktime_get_ns();
    __u64 delta_ns = now - *start_ts;
    __u64 delta_us = delta_ns / 1000;

    /* Clean up entry timestamp. */
    bpf_map_delete_elem(&syscall_start, &key);

    /* Emit event via ring buffer. */
    struct syscall_event *evt = bpf_ringbuf_reserve(&syscall_events, sizeof(*evt), 0);
    if (!evt)
        return 0;

    evt->latency_us = delta_us;
    evt->syscall_nr = syscall_nr;

    bpf_ringbuf_submit(evt, 0);
    return 0;
}

char LICENSE[] SEC("license") = "Dual BSD/GPL";
