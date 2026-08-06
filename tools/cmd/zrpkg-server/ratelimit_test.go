package main

import (
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestAllowPermitsUpToTheBurst(t *testing.T) {
	rl := newRateLimiter(1, 3)
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
	rl := newRateLimiter(1, 1) // one token per second, capacity 1
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
	rl := newRateLimiter(100, 2) // fast refill, small cap
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
	rl := newRateLimiter(1, 1)
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
	rl := newRateLimiter(1, 1)
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
	rl := newRateLimiter(1, 1)
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

func TestClientIPStripsThePort(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.RemoteAddr = "203.0.113.5:54321"
	if got := clientIP(req); got != "203.0.113.5" {
		t.Errorf("got %q", got)
	}
}

func TestClientIPFallsBackToTheRawValue(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.RemoteAddr = "not-a-host-port"
	if got := clientIP(req); got != "not-a-host-port" {
		t.Errorf("got %q", got)
	}
}
