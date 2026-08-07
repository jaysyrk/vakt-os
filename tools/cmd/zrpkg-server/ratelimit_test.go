package main

import (
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestAllowPermitsUpToTheBurst(t *testing.T) {
	rl := newRateLimiter(1, 3, false)
	now := time.Now()

	for i := 0; i < 3; i++ {
		if !rl.allow("1.2.3.4", now) {
			t.Fatalf("request %d within the burst should be allowed", i)
		}
	}
	if rl.allow("1.2.3.4", now) {
		t.Error("a request past the burst should be denied")
	}
}

func TestAllowRefillsOverTime(t *testing.T) {
	rl := newRateLimiter(1, 1, false) // one token per second, capacity 1
	now := time.Now()

	if !rl.allow("1.2.3.4", now) {
		t.Fatal("the first request should consume the initial token")
	}
	if rl.allow("1.2.3.4", now) {
		t.Fatal("a second immediate request should be denied")
	}
	if !rl.allow("1.2.3.4", now.Add(2*time.Second)) {
		t.Error("a request after the refill interval should be allowed")
	}
}

func TestAllowDoesNotRefillPastTheBurstCap(t *testing.T) {
	rl := newRateLimiter(100, 2, false) // fast refill, small cap
	now := time.Now()

	// A long gap should still cap the bucket at burst, not let tokens pile up
	// without bound.
	later := now.Add(time.Hour)
	allowed := 0
	for i := 0; i < 5; i++ {
		if rl.allow("1.2.3.4", later) {
			allowed++
		}
	}
	if allowed != 2 {
		t.Errorf("expected exactly burst (2) requests to succeed after a long gap, got %d", allowed)
	}
}

func TestAllowTracksEachIPSeparately(t *testing.T) {
	rl := newRateLimiter(1, 1, false)
	now := time.Now()

	if !rl.allow("1.1.1.1", now) {
		t.Fatal("first IP's first request should be allowed")
	}
	if !rl.allow("2.2.2.2", now) {
		t.Error("a different IP must have its own budget")
	}
	if rl.allow("1.1.1.1", now) {
		t.Error("the first IP should still be limited")
	}
}

func TestEvictOlderThanRemovesOnlyStaleBuckets(t *testing.T) {
	rl := newRateLimiter(1, 1, false)
	now := time.Now()
	rl.allow("stale", now.Add(-time.Hour))
	rl.allow("fresh", now)

	rl.evictOlderThan(now.Add(-time.Minute))

	rl.mu.Lock()
	_, staleStillThere := rl.buckets["stale"]
	_, freshStillThere := rl.buckets["fresh"]
	rl.mu.Unlock()

	if staleStillThere {
		t.Error("a bucket idle past the cutoff should have been evicted")
	}
	if !freshStillThere {
		t.Error("a recently used bucket must survive eviction")
	}
}

func TestLimitRejectsWithTooManyRequestsOnceExhausted(t *testing.T) {
	rl := newRateLimiter(1, 1, false)
	handler := rl.limit(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))

	req := httptest.NewRequest(http.MethodGet, "/index.json", nil)
	req.RemoteAddr = "9.9.9.9:1234"

	first := httptest.NewRecorder()
	handler.ServeHTTP(first, req)
	if first.Code != http.StatusOK {
		t.Fatalf("first request: got %d, want 200", first.Code)
	}

	second := httptest.NewRecorder()
	handler.ServeHTTP(second, req)
	if second.Code != http.StatusTooManyRequests {
		t.Errorf("second request: got %d, want 429", second.Code)
	}
}

func TestRemoteAddrHostStripsThePort(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.RemoteAddr = "203.0.113.5:54321"
	if got := remoteAddrHost(req); got != "203.0.113.5" {
		t.Errorf("got %q", got)
	}
}

func TestRemoteAddrHostFallsBackToTheRawValue(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.RemoteAddr = "not-a-host-port"
	if got := remoteAddrHost(req); got != "not-a-host-port" {
		t.Errorf("got %q", got)
	}
}

func TestClientIPIgnoresXRealIPByDefault(t *testing.T) {
	rl := newRateLimiter(1, 1, false)
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.RemoteAddr = "127.0.0.1:8080"
	req.Header.Set("X-Real-IP", "203.0.113.9")

	if got := rl.clientIP(req); got != "127.0.0.1" {
		t.Errorf("trustProxy off should ignore X-Real-IP, got %q", got)
	}
}

func TestClientIPTrustsXRealIPFromLoopbackWhenEnabled(t *testing.T) {
	rl := newRateLimiter(1, 1, true)
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.RemoteAddr = "127.0.0.1:8080"
	req.Header.Set("X-Real-IP", "203.0.113.9")

	if got := rl.clientIP(req); got != "203.0.113.9" {
		t.Errorf("got %q, want the proxied client address", got)
	}
}

// A request that isn't itself coming from loopback can't spoof its way past
// the limit by setting its own X-Real-IP header, even with trustProxy on -
// only a connection that is actually the local proxy is trusted to set it.
func TestClientIPIgnoresXRealIPFromNonLoopbackEvenWhenTrusted(t *testing.T) {
	rl := newRateLimiter(1, 1, true)
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.RemoteAddr = "203.0.113.1:12345"
	req.Header.Set("X-Real-IP", "203.0.113.9")

	if got := rl.clientIP(req); got != "203.0.113.1" {
		t.Errorf("a direct, non-loopback client must be keyed on its own address, got %q", got)
	}
}

func TestClientIPFallsBackWhenXRealIPHeaderIsMissing(t *testing.T) {
	rl := newRateLimiter(1, 1, true)
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.RemoteAddr = "127.0.0.1:8080"

	if got := rl.clientIP(req); got != "127.0.0.1" {
		t.Errorf("got %q", got)
	}
}
