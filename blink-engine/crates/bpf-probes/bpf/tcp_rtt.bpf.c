/* SPDX-License-Identifier: GPL-2.0 OR BSD-2-Clause */
/*
 * tcp_rtt.bpf.c — TCP RTT measurement for Polymarket CLOB connections.
 *
 * Attaches to: tracepoint/tcp/tcp_rcv_established
 * Filters:     Only connections to Polymarket CDN (104.18.0.0/16)
 * Output:      rtt_event via ring buffer
 *
 * Compile (CO-RE):
 *   clang -O2 -g -target bpf -D__TARGET_ARCH_x86 \
 *         -I/path/to/vmlinux.h -c tcp_rtt.bpf.c -o tcp_rtt.bpf.o
 *
 * vmlinux.h generation:
 *   bpftool btf dump file /sys/kernel/btf/vmlinux format c > vmlinux.h
 */

#include "common.h"

/* ─── Ring buffer map ──────────────────────────────────────────────────── */

struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, RINGBUF_SIZE);
} rtt_events SEC(".maps");

/* ─── Tracepoint: tcp/tcp_rcv_established ──────────────────────────────── */
/*
 * Kernel tracepoint args (from /sys/kernel/debug/tracing/events/tcp/tcp_rcv_established/format):
 *   __u16  sport;
 *   __u16  dport;
 *   __u16  family;
 *   __u8   saddr[4];
 *   __u8   daddr[4];
 *   __u8   saddr_v6[16];
 *   __u8   daddr_v6[16];
 *   __u64  sock_cookie;
 *
 * We access the sock structure to read srtt_us from tcp_sock.
 */

SEC("tp/tcp/tcp_rcv_established")
int tcp_rtt_probe(struct trace_event_raw_tcp_event_sk *ctx)
{
    struct sock *sk = (struct sock *)ctx->skaddr;
    if (!sk)
        return 0;

    /* Read destination address to filter for Polymarket CDN. */
    __u32 daddr = BPF_CORE_READ(sk, __sk_common.skc_daddr);
    if (!is_polymarket_ip(daddr))
        return 0;

    /* Read smoothed RTT from tcp_sock (in µs, already smoothed by kernel). */
    struct tcp_sock *tp = (struct tcp_sock *)sk;
    __u32 srtt_us = BPF_CORE_READ(tp, srtt_us);

    /* srtt_us is stored as srtt << 3 internally; divide by 8 for actual µs. */
    __u64 rtt = (__u64)(srtt_us >> 3);

    /* Emit event via ring buffer. */
    struct rtt_event *evt = bpf_ringbuf_reserve(&rtt_events, sizeof(*evt), 0);
    if (!evt)
        return 0;

    evt->rtt_us = rtt;
    evt->saddr  = BPF_CORE_READ(sk, __sk_common.skc_rcv_saddr);
    evt->daddr  = daddr;

    bpf_ringbuf_submit(evt, 0);
    return 0;
}

char LICENSE[] SEC("license") = "Dual BSD/GPL";
