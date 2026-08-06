package main

import (
	"net"
	"net/http"
	"sync"
	"time"
)

// evictAfter is how long a client's bucket survives with no requests. A
// public server on a rented host accumulates one bucket per IP that ever
// knocks on it; without eviction that map only grows.
const evictAfter = 10 * time.Minute

type bucket struct {
	tokens   float64
	lastSeen time.Time
}

// rateLimiter is a per-IP token bucket. It exists to stop one client from
// hammering the repository, not to shape traffic precisely, so a plain
// fixed-rate refill - no fairness queue, no burst smoothing - is enough.
type rateLimiter struct {
	mu      sync.Mutex
	buckets map[string]*bucket
	rate    float64 // tokens added per second
	burst   float64 // bucket capacity
}

func newRateLimiter(rate, burst float64) *rateLimiter {
	rl := &rateLimiter{
		buckets: make(map[string]*bucket),
		rate:    rate,
		burst:   burst,
	}
	go rl.evictStale()
	return rl
}

// allow reports whether a request from ip may proceed, consuming a token if
// so. now is threaded through explicitly so refill and eviction are testable
// without sleeping in the test.
func (rl *rateLimiter) allow(ip string, now time.Time) bool {
	rl.mu.Lock()
	defer rl.mu.Unlock()

	b, ok := rl.buckets[ip]
	if !ok {
		b = &bucket{tokens: rl.burst, lastSeen: now}
		rl.buckets[ip] = b
	}

	b.tokens += now.Sub(b.lastSeen).Seconds() * rl.rate
	if b.tokens > rl.burst {
		b.tokens = rl.burst
	}
	b.lastSeen = now

	if b.tokens < 1 {
		return false
	}
	b.tokens--
	return true
}

func (rl *rateLimiter) evictStale() {
	for {
		time.Sleep(evictAfter)
		rl.evictOlderThan(time.Now().Add(-evictAfter))
	}
}

func (rl *rateLimiter) evictOlderThan(cutoff time.Time) {
	rl.mu.Lock()
	defer rl.mu.Unlock()
	for ip, b := range rl.buckets {
		if b.lastSeen.Before(cutoff) {
			delete(rl.buckets, ip)
		}
	}
}

// limit wraps a handler, rejecting requests once an IP exceeds its rate with
// 429 before the wrapped handler - and the filesystem access it does - ever
// runs.
func (rl *rateLimiter) limit(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !rl.allow(clientIP(r), time.Now()) {
			w.Header().Set("Retry-After", "1")
			http.Error(w, "rate limit exceeded", http.StatusTooManyRequests)
			return
		}
		next.ServeHTTP(w, r)
	})
}

// clientIP strips the port RemoteAddr carries, falling back to the whole
// value if it is not in host:port form.
func clientIP(r *http.Request) string {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}
