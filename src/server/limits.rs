// Upload limits that protect the shared database now that the service address is public: a request
// rate per installation and per client address, a cap on the records in one batch, an event quota
// and a repository quota per installation, and a total database size above which uploads are
// refused (reads keep working) until routine events expire or the operator makes room. Every limit
// can be changed in the service's environment; a refused upload stays on the client and is retried.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mongodb::bson::doc;
use mongodb::sync::Database;

use super::store::number;
use crate::platform::io_error;

/** How long a measured database size is reused */
const STORAGE_CHECK: Duration = Duration::from_secs(60);

/** The configured limits
 * Fields
    - requests_per_minute: f64 - signed requests per installation (CRANE_SERVER_REQUESTS_PER_MINUTE)
    - addresses_per_minute: f64 - requests per client address (CRANE_SERVER_ADDRESS_REQUESTS_PER_MINUTE)
    - max_batch_records: usize - records of one kind in one batch (CRANE_SERVER_MAX_BATCH_RECORDS)
    - max_events_per_installation: u64 - stored events (CRANE_SERVER_MAX_EVENTS_PER_INSTALLATION)
    - max_repositories_per_installation: u64 - repositories (CRANE_SERVER_MAX_REPOSITORIES_PER_INSTALLATION)
    - max_storage_bytes: u64 - database data and index size (CRANE_SERVER_MAX_STORAGE_MB)
*/
#[derive(Debug, Clone)]
pub(super) struct Limits {
    pub(super) requests_per_minute: f64,
    pub(super) addresses_per_minute: f64,
    pub(super) max_batch_records: usize,
    pub(super) max_events_per_installation: u64,
    pub(super) max_repositories_per_installation: u64,
    pub(super) max_storage_bytes: u64,
}

impl Limits {
    /** Read the limits from the environment, with defaults sized for a free 512 MB cluster
     * Input
        - None
     * Output
        - Limits
    */
    pub(super) fn from_environment() -> Self {
        let number = |name: &str, default: u64| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(default)
        };
        Self {
            requests_per_minute: number("CRANE_SERVER_REQUESTS_PER_MINUTE", 120) as f64,
            addresses_per_minute: number("CRANE_SERVER_ADDRESS_REQUESTS_PER_MINUTE", 600) as f64,
            max_batch_records: number("CRANE_SERVER_MAX_BATCH_RECORDS", 2_000) as usize,
            max_events_per_installation: number(
                "CRANE_SERVER_MAX_EVENTS_PER_INSTALLATION",
                250_000,
            ),
            max_repositories_per_installation: number(
                "CRANE_SERVER_MAX_REPOSITORIES_PER_INSTALLATION",
                50,
            ),
            max_storage_bytes: number("CRANE_SERVER_MAX_STORAGE_MB", 450) * 1024 * 1024,
        }
    }
}

/** A token bucket per key: each key may make per_minute requests a minute, in bursts of at most
 * per_minute
 * Fields
    - per_minute: f64 - allowed rate
    - buckets: Mutex<HashMap<String, (f64, Instant)>> - tokens left and when they were counted
*/
pub(super) struct Limiter {
    per_minute: f64,
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl Limiter {
    /** Create a limiter
     * Input
        - per_minute: f64 - allowed rate
     * Output
        - Limiter
    */
    pub(super) fn new(per_minute: f64) -> Self {
        Self {
            per_minute,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /** Take one request for a key
     * Input
        - key: &str - installation or client address
     * Output
        - bool, false when the key is over its rate
    */
    pub(super) fn allow(&self, key: &str) -> bool {
        let Ok(mut buckets) = self.buckets.lock() else {
            return true;
        };
        let now = Instant::now();
        if buckets.len() > 100_000 {
            buckets.retain(|_, (_, at)| now.duration_since(*at) < Duration::from_secs(120));
        }
        let bucket = buckets
            .entry(key.to_string())
            .or_insert((self.per_minute, now));
        let refill = now.duration_since(bucket.1).as_secs_f64() * self.per_minute / 60.0;
        *bucket = ((bucket.0 + refill).min(self.per_minute), now);
        if bucket.0 >= 1.0 {
            bucket.0 -= 1.0;
            true
        } else {
            false
        }
    }
}

/** The database size, measured at most once a minute
 * Fields
    - measured: Mutex<Option<(Instant, u64)>> - when it was measured and the size in bytes
*/
pub(super) struct Storage {
    measured: Mutex<Option<(Instant, u64)>>,
}

impl Storage {
    /** Create an unmeasured storage gauge
     * Input
        - None
     * Output
        - Storage
    */
    pub(super) fn new() -> Self {
        Self {
            measured: Mutex::new(None),
        }
    }

    /** Return the data and index size of the database
     * Input
        - database: &Database - database
     * Output
        - Result<u64, String> bytes
    */
    pub(super) fn used(&self, database: &Database) -> Result<u64, String> {
        if let Ok(measured) = self.measured.lock() {
            if let Some((at, bytes)) = *measured {
                if at.elapsed() < STORAGE_CHECK {
                    return Ok(bytes);
                }
            }
        }
        let stats = database
            .run_command(doc! {"dbStats": 1})
            .run()
            .map_err(io_error)?;
        let bytes = number(stats.get("dataSize")) + number(stats.get("indexSize"));
        if let Ok(mut measured) = self.measured.lock() {
            *measured = Some((Instant::now(), bytes));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::Limiter;

    /** Check that a key is limited to its burst while other keys are not affected
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn limiter_caps_each_key() {
        let limiter = Limiter::new(3.0);
        assert!((0..3).all(|_| limiter.allow("a")));
        assert!(!limiter.allow("a"));
        assert!(limiter.allow("b"));
    }
}
