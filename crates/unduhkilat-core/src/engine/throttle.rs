//! Hierarchical token-bucket bandwidth limiting (FR-025–FR-028).
//!
//! Workers acquire from the per-download bucket and the global bucket before
//! writing each chunk, so effective throughput is `min(item, global)`.
//! A rate of 0 means unlimited. Rates can be changed live.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a limiter may bank unused tokens (burst), as a fraction of one second.
const BURST_SECONDS: f64 = 0.25;
/// While waiting out a debt, re-check the rate at this granularity so that lifting
/// a limit (setting 0) takes effect promptly. PROVISIONAL "throttle tick": the
/// docs leave the live-change latency bound TBD (TASK-24).
pub const THROTTLE_TICK: Duration = Duration::from_millis(100);

struct State {
    tokens: f64,
    last: Instant,
}

pub struct TokenBucket {
    rate_bps: AtomicU64,
    state: Mutex<State>,
}

impl TokenBucket {
    pub fn new(kbps: u64) -> Self {
        Self { rate_bps: AtomicU64::new(kbps.saturating_mul(1024)), state: Mutex::new(State { tokens: 0.0, last: Instant::now() }) }
    }

    pub fn unlimited() -> Self {
        Self::new(0)
    }

    /// Change the limit live. `0` lifts it.
    pub fn set_rate_kbps(&self, kbps: u64) {
        let new = kbps.saturating_mul(1024);
        let old = self.rate_bps.swap(new, Ordering::AcqRel);
        if old == 0 && new != 0 {
            // Start metering from now with an empty bucket.
            let mut st = self.state.lock().unwrap();
            st.tokens = 0.0;
            st.last = Instant::now();
        }
    }

    pub fn rate_kbps(&self) -> u64 {
        self.rate_bps.load(Ordering::Acquire) / 1024
    }

    /// Wait until `n` bytes may be consumed.
    pub async fn acquire(&self, n: usize) {
        let rate = self.rate_bps.load(Ordering::Acquire);
        if rate == 0 || n == 0 {
            return;
        }
        let rate_f = rate as f64;
        let mut wait = {
            let mut st = self.state.lock().unwrap();
            let now = Instant::now();
            let elapsed = now.duration_since(st.last).as_secs_f64();
            st.last = now;
            st.tokens = (st.tokens + elapsed * rate_f).min(rate_f * BURST_SECONDS);
            st.tokens -= n as f64; // may go negative: that debt is what we sleep off
            if st.tokens < 0.0 { Duration::from_secs_f64(-st.tokens / rate_f) } else { Duration::ZERO }
        };
        while !wait.is_zero() {
            let slice = wait.min(THROTTLE_TICK);
            tokio::time::sleep(slice).await;
            wait -= slice;
            if self.rate_bps.load(Ordering::Acquire) == 0 {
                return; // limit lifted while we were waiting
            }
        }
    }
}

/// Global bucket shared by every transfer.
pub struct Throttler {
    pub global: TokenBucket,
}

impl Throttler {
    pub fn new(global_kbps: u64) -> Self {
        Self { global: TokenBucket::new(global_kbps) }
    }

    /// Acquire from both the item and global buckets: effective rate = min of the two.
    pub async fn acquire(&self, item: &TokenBucket, n: usize) {
        item.acquire(n).await;
        self.global.acquire(n).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_does_not_wait() {
        let b = TokenBucket::unlimited();
        let t = Instant::now();
        for _ in 0..1000 {
            b.acquire(1 << 20).await;
        }
        assert!(t.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn limited_rate_is_respected() {
        // 100 KB/s, consume 150 KB => ~1.5 s minus a small burst allowance.
        let b = TokenBucket::new(100);
        let t = Instant::now();
        for _ in 0..15 {
            b.acquire(10 * 1024).await;
        }
        let secs = t.elapsed().as_secs_f64();
        assert!((1.2..2.2).contains(&secs), "took {secs}s");
    }

    #[tokio::test]
    async fn effective_rate_is_the_lower_of_item_and_global() {
        let th = Throttler::new(50); // global 50 KB/s
        let item = TokenBucket::new(1000); // item far faster
        let t = Instant::now();
        for _ in 0..5 {
            th.acquire(&item, 10 * 1024).await; // 50 KB total
        }
        let secs = t.elapsed().as_secs_f64();
        assert!((0.6..1.6).contains(&secs), "global should dominate, took {secs}s");

        let th = Throttler::new(1000);
        let item = TokenBucket::new(50); // item slower
        let t = Instant::now();
        for _ in 0..5 {
            th.acquire(&item, 10 * 1024).await;
        }
        let secs = t.elapsed().as_secs_f64();
        assert!((0.6..1.6).contains(&secs), "item should dominate, took {secs}s");
    }

    #[tokio::test]
    async fn live_change_to_zero_releases_a_waiting_worker() {
        let b = std::sync::Arc::new(TokenBucket::new(1)); // 1 KB/s: 64 KB would take ~64 s
        let b2 = b.clone();
        let h = tokio::spawn(async move { b2.acquire(64 * 1024).await });
        tokio::time::sleep(Duration::from_millis(250)).await;
        b.set_rate_kbps(0);
        let t = Instant::now();
        tokio::time::timeout(Duration::from_secs(2), h).await.expect("worker released").unwrap();
        assert!(t.elapsed() < Duration::from_secs(1));
    }
}
