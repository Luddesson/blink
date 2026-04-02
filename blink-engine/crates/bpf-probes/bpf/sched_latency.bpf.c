/* SPDX-License-Identifier: GPL-2.0 OR BSD-2-Clause */
/*
 * sched_latency.bpf.c — Scheduler wakeup-to-run latency for Blink engine.
 *
 * Attaches to:
 *   tracepoint/sched/sched_wakeup   — records wakeup timestamp
 *   tracepoint/sched/sched_switch   — computes wakeup-to-run delta
 *
 * Filters: Only the target PID (Blink engine process), set via BPF map.
 * Output:  sched_event via ring buffer
 * Alert:   Events with latency > 100µs indicate CPU contention.
 *
 * Compile (CO-RE):
 *   clang -O2 -g -target bpf -D__TARGET_ARCH_x86 \
 *         -I/path/to/vmlinux.h -c sched_latency.bpf.c -o sched_latency.bpf.o
 */

#include "common.h"

/* ─── Maps ─────────────────────────────────────────────────────────────── */

/* Ring buffer for sched latency events. */
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, RINGBUF_SIZE);
} sched_events SEC(".maps");

/* Per-task wakeup timestamp: task PID → ktime_ns when woken. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __type(key, __u32);
    __type(value, __u64);
    __uint(max_entries, 1024);
} wakeup_ts SEC(".maps");

/* Target PID to filter on. Set from userspace before attaching. */
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __type(key, __u32);
    __type(value, __u32);
    __uint(max_entries, 1);
} target_pid SEC(".maps");

/* ─── Helper: check if PID matches our target ─────────────────────────── */

static __always_inline int is_target_pid(__u32 pid)
{
    __u32 key = 0;
    __u32 *target = bpf_map_lookup_elem(&target_pid, &key);
    if (!target)
        return 0;
    return pid == *target;
}

/* ─── Tracepoint: sched/sched_wakeup ──────────────────────────────────── */
/*
 * Records the ktime when the target task is woken up.
 * The actual latency is computed in sched_switch when the task runs.
 */

SEC("tp/sched/sched_wakeup")
int sched_wakeup_probe(struct trace_event_raw_sched_wakeup_template *ctx)
{
    __u32 pid = ctx->pid;
    if (!is_target_pid(pid))
        return 0;

    __u64 ts = bpf_ktime_get_ns();
    bpf_map_update_elem(&wakeup_ts, &pid, &ts, BPF_ANY);
    return 0;
}

/* ─── Tracepoint: sched/sched_switch ──────────────────────────────────── */
/*
 * When the target task is switched IN (next_pid == target), compute
 * the delta from wakeup time. This is the scheduler latency.
 */

SEC("tp/sched/sched_switch")
int sched_switch_probe(struct trace_event_raw_sched_switch *ctx)
{
    __u32 next_pid = ctx->next_pid;
    if (!is_target_pid(next_pid))
        return 0;

    /* Look up wakeup timestamp for this task. */
    __u64 *wakeup = bpf_map_lookup_elem(&wakeup_ts, &next_pid);
    if (!wakeup)
        return 0;

    __u64 now = bpf_ktime_get_ns();
    __u64 delta_ns = now - *wakeup;
    __u64 delta_us = delta_ns / 1000;

    /* Clean up — remove wakeup entry so we don't double-count. */
    bpf_map_delete_elem(&wakeup_ts, &next_pid);

    /* Emit event via ring buffer. */
    struct sched_event *evt = bpf_ringbuf_reserve(&sched_events, sizeof(*evt), 0);
    if (!evt)
        return 0;

    evt->latency_us = delta_us;
    evt->pid        = next_pid;
    evt->_pad       = 0;

    bpf_ringbuf_submit(evt, 0);
    return 0;
}

char LICENSE[] SEC("license") = "Dual BSD/GPL";
