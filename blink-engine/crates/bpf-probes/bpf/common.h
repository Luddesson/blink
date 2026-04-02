/* SPDX-License-Identifier: GPL-2.0 OR BSD-2-Clause */
/*
 * Common definitions shared between Blink BPF programs.
 *
 * These structs MUST match the #[repr(C)] Rust counterparts in linux_impl.rs.
 */

#ifndef __BLINK_BPF_COMMON_H
#define __BLINK_BPF_COMMON_H

#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>

/* ─── Event structs emitted via ring buffer ─────────────────────────────── */

struct rtt_event {
    __u64 rtt_us;
    __u32 saddr;
    __u32 daddr;
};

struct sched_event {
    __u64 latency_us;
    __u32 pid;
    __u32 _pad;
};

struct syscall_event {
    __u64 latency_us;
    __u64 syscall_nr;
};

/* ─── Polymarket CDN IP filter (104.18.0.0/16) ──────────────────────────── */

/* Network byte order: 104.18.x.x → 0x6812xxxx (big-endian on x86) */
static __always_inline int is_polymarket_ip(__u32 daddr_be) {
    /* Mask upper 16 bits in network byte order */
    __u32 masked = daddr_be & 0x0000FFFF;  /* first 2 octets in network order */
    /* 104 = 0x68, 18 = 0x12 → network order bytes: 0x68, 0x12 */
    return masked == 0x00001268;  /* 104.18.x.x in little-endian u32 */
}

/* ─── Ring buffer size: 256 KB (handles >100k events/sec) ───────────────── */
#define RINGBUF_SIZE (256 * 1024)

#endif /* __BLINK_BPF_COMMON_H */
