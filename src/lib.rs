//! Periodic and throttled logging macros for the tracing ecosystem.
//!
//! This crate provides macros that extend `tracing` with rate-limited logging capabilities:
//!
//! - `*_once!` - Log only the first time the macro is reached
//! - `*_every!(duration, ...)` - Log at most once per specified duration
//! - `*_every_n!(n, ...)` - Log every N occurrences
//! - `*_first_n!(n, ...)` - Log only the first N occurrences
//! - `*_backoff!(initial, max, ...)` - Log with exponential backoff
//! - `*_on_change!(value, ...)` - Log only when the tracked value changes
//! - `*_once_per_value!(key, ...)` - Log once per unique key value
//! - `*_sample!(probability, ...)` - Log with probability sampling
//!
//! # Examples
//!
//! ```rust
//! use throttled_tracing::{info_once, debug_every, warn_every_n};
//! use std::time::Duration;
//!
//! fn process_item(item: u32) {
//!     // Only logs the first time this line is reached
//!     info_once!("Processing started");
//!
//!     // Logs at most once per second
//!     debug_every!(Duration::from_secs(1), "Processing item {}", item);
//!
//!     // Logs every 100th call
//!     warn_every_n!(100, "Processed {} items so far", item);
//! }
//! ```
//!
//! ```rust
//! use throttled_tracing::{error_backoff, info_on_change, debug_once_per_value, trace_sample};
//! use std::time::Duration;
//!
//! fn handle_error(err: &str) {
//!     // Exponential backoff: logs at 1s, 2s, 4s, 8s... up to 60s
//!     error_backoff!(Duration::from_secs(1), Duration::from_secs(60), "Error: {}", err);
//! }
//!
//! fn monitor_status(status: u32) {
//!     // Only logs when status changes
//!     info_on_change!(status, "Status changed to {}", status);
//! }
//!
//! fn process_user(user_id: u64) {
//!     // Logs once per unique user_id
//!     debug_once_per_value!(user_id, "First time seeing user {}", user_id);
//! }
//!
//! fn high_volume_operation() {
//!     // Logs ~1% of calls
//!     trace_sample!(0.01, "Sampled operation");
//! }
//! ```

pub use parking_lot;
pub use tracing;

pub use fastrand;

use parking_lot::Mutex;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Computes a hash of a value for change detection and deduplication.
#[doc(hidden)]
pub fn compute_hash<T: Hash>(value: &T) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// State for duration-based throttling.
pub struct ThrottleState {
    last_log: Mutex<Option<Instant>>,
}

impl Default for ThrottleState {
    fn default() -> Self {
        Self::new()
    }
}

impl ThrottleState {
    pub const fn new() -> Self {
        Self {
            last_log: Mutex::new(None),
        }
    }

    /// Returns true if enough time has passed since the last log.
    pub fn should_log(&self, interval: Duration) -> bool {
        let mut last = self.last_log.lock();
        let now = Instant::now();

        match *last {
            None => {
                *last = Some(now);
                true
            }
            Some(prev) if now.duration_since(prev) >= interval => {
                *last = Some(now);
                true
            }
            _ => false,
        }
    }
}

/// State for count-based throttling.
pub struct CountState {
    count: AtomicUsize,
}

impl Default for CountState {
    fn default() -> Self {
        Self::new()
    }
}

impl CountState {
    pub const fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
        }
    }

    /// Returns true every N calls (1st call, N+1th call, 2N+1th call, etc.)
    pub fn should_log(&self, n: usize) -> bool {
        let count = self.count.fetch_add(1, Ordering::Relaxed);
        count.is_multiple_of(n)
    }
}

/// State for one-time logging.
pub struct OnceState {
    logged: AtomicBool,
}

impl Default for OnceState {
    fn default() -> Self {
        Self::new()
    }
}

impl OnceState {
    pub const fn new() -> Self {
        Self {
            logged: AtomicBool::new(false),
        }
    }

    /// Returns true only on the first call.
    pub fn should_log(&self) -> bool {
        !self.logged.swap(true, Ordering::Relaxed)
    }
}

/// State for logging only the first N occurrences.
pub struct FirstNState {
    count: AtomicUsize,
}

impl Default for FirstNState {
    fn default() -> Self {
        Self::new()
    }
}

impl FirstNState {
    pub const fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
        }
    }

    /// Returns true for the first N calls, then false forever.
    pub fn should_log(&self, n: usize) -> bool {
        let count = self.count.fetch_add(1, Ordering::Relaxed);
        count < n
    }
}

/// State for exponential backoff logging.
pub struct BackoffState {
    state: Mutex<Option<BackoffInner>>,
}

struct BackoffInner {
    last_log: Instant,
    current_interval: Duration,
}

impl Default for BackoffState {
    fn default() -> Self {
        Self::new()
    }
}

impl BackoffState {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(None),
        }
    }

    /// Returns true with exponential backoff timing.
    /// Starts at `initial` interval, doubles after each log, caps at `max`.
    pub fn should_log(&self, initial: Duration, max: Duration) -> bool {
        let mut state = self.state.lock();
        let now = Instant::now();

        match state.as_mut() {
            None => {
                *state = Some(BackoffInner {
                    last_log: now,
                    current_interval: initial,
                });
                true
            }
            Some(inner) if now.duration_since(inner.last_log) >= inner.current_interval => {
                inner.last_log = now;
                inner.current_interval = (inner.current_interval * 2).min(max);
                true
            }
            _ => false,
        }
    }
}

/// State for logging only when a value changes.
/// Uses hash comparison, so values must implement `Hash`.
pub struct OnChangeState {
    prev_hash: AtomicU64,
    initialized: AtomicBool,
}

impl Default for OnChangeState {
    fn default() -> Self {
        Self::new()
    }
}

impl OnChangeState {
    pub const fn new() -> Self {
        Self {
            prev_hash: AtomicU64::new(0),
            initialized: AtomicBool::new(false),
        }
    }

    /// Returns true if the value's hash differs from the previous call.
    /// Always returns true on the first call.
    pub fn should_log<T: Hash>(&self, value: &T) -> bool {
        let new_hash = compute_hash(value);

        // First call: initialize and log
        if !self.initialized.swap(true, Ordering::AcqRel) {
            self.prev_hash.store(new_hash, Ordering::Release);
            return true;
        }

        // Subsequent calls: compare and swap
        let prev = self.prev_hash.swap(new_hash, Ordering::AcqRel);
        prev != new_hash
    }
}

/// State for logging once per unique value.
/// Uses hash-based deduplication, so values must implement `Hash`.
/// Note: Memory grows with each unique value seen.
pub struct OncePerValueState {
    seen: Mutex<HashSet<u64>>,
}

impl Default for OncePerValueState {
    fn default() -> Self {
        Self::new()
    }
}

impl OncePerValueState {
    pub fn new() -> Self {
        Self {
            seen: Mutex::new(HashSet::new()),
        }
    }

    /// Returns true only the first time a particular value's hash is seen.
    pub fn should_log<T: Hash>(&self, value: &T) -> bool {
        let hash = compute_hash(value);
        let mut seen = self.seen.lock();
        seen.insert(hash)
    }
}

/// State for probabilistic sampling.
pub struct SampleState {
    _private: (),
}

impl Default for SampleState {
    fn default() -> Self {
        Self::new()
    }
}

impl SampleState {
    pub const fn new() -> Self {
        Self { _private: () }
    }

    /// Returns true with the given probability (0.0 to 1.0).
    pub fn should_log(&self, probability: f64) -> bool {
        fastrand::f64() < probability
    }
}

// ============================================================================
// *_once! macros - log only the first time
// ============================================================================

/// Logs a message at the TRACE level only once.
#[macro_export]
macro_rules! trace_once {
    ($($arg:tt)*) => {{
        static STATE: $crate::OnceState = $crate::OnceState::new();
        if STATE.should_log() {
            $crate::tracing::trace!($($arg)*);
        }
    }};
}

/// Logs a message at the DEBUG level only once.
#[macro_export]
macro_rules! debug_once {
    ($($arg:tt)*) => {{
        static STATE: $crate::OnceState = $crate::OnceState::new();
        if STATE.should_log() {
            $crate::tracing::debug!($($arg)*);
        }
    }};
}

/// Logs a message at the INFO level only once.
#[macro_export]
macro_rules! info_once {
    ($($arg:tt)*) => {{
        static STATE: $crate::OnceState = $crate::OnceState::new();
        if STATE.should_log() {
            $crate::tracing::info!($($arg)*);
        }
    }};
}

/// Logs a message at the WARN level only once.
#[macro_export]
macro_rules! warn_once {
    ($($arg:tt)*) => {{
        static STATE: $crate::OnceState = $crate::OnceState::new();
        if STATE.should_log() {
            $crate::tracing::warn!($($arg)*);
        }
    }};
}

/// Logs a message at the ERROR level only once.
#[macro_export]
macro_rules! error_once {
    ($($arg:tt)*) => {{
        static STATE: $crate::OnceState = $crate::OnceState::new();
        if STATE.should_log() {
            $crate::tracing::error!($($arg)*);
        }
    }};
}

// ============================================================================
// *_every! macros - log at most once per duration
// ============================================================================

/// Logs a message at the TRACE level at most once per specified duration.
#[macro_export]
macro_rules! trace_every {
    ($duration:expr, $($arg:tt)*) => {{
        static STATE: $crate::ThrottleState = $crate::ThrottleState::new();
        if STATE.should_log($duration) {
            $crate::tracing::trace!($($arg)*);
        }
    }};
}

/// Logs a message at the DEBUG level at most once per specified duration.
#[macro_export]
macro_rules! debug_every {
    ($duration:expr, $($arg:tt)*) => {{
        static STATE: $crate::ThrottleState = $crate::ThrottleState::new();
        if STATE.should_log($duration) {
            $crate::tracing::debug!($($arg)*);
        }
    }};
}

/// Logs a message at the INFO level at most once per specified duration.
#[macro_export]
macro_rules! info_every {
    ($duration:expr, $($arg:tt)*) => {{
        static STATE: $crate::ThrottleState = $crate::ThrottleState::new();
        if STATE.should_log($duration) {
            $crate::tracing::info!($($arg)*);
        }
    }};
}

/// Logs a message at the WARN level at most once per specified duration.
#[macro_export]
macro_rules! warn_every {
    ($duration:expr, $($arg:tt)*) => {{
        static STATE: $crate::ThrottleState = $crate::ThrottleState::new();
        if STATE.should_log($duration) {
            $crate::tracing::warn!($($arg)*);
        }
    }};
}

/// Logs a message at the ERROR level at most once per specified duration.
#[macro_export]
macro_rules! error_every {
    ($duration:expr, $($arg:tt)*) => {{
        static STATE: $crate::ThrottleState = $crate::ThrottleState::new();
        if STATE.should_log($duration) {
            $crate::tracing::error!($($arg)*);
        }
    }};
}

// ============================================================================
// *_every_n! macros - log every N occurrences
// ============================================================================

/// Logs a message at the TRACE level every N occurrences.
#[macro_export]
macro_rules! trace_every_n {
    ($n:expr, $($arg:tt)*) => {{
        static STATE: $crate::CountState = $crate::CountState::new();
        if STATE.should_log($n) {
            $crate::tracing::trace!($($arg)*);
        }
    }};
}

/// Logs a message at the DEBUG level every N occurrences.
#[macro_export]
macro_rules! debug_every_n {
    ($n:expr, $($arg:tt)*) => {{
        static STATE: $crate::CountState = $crate::CountState::new();
        if STATE.should_log($n) {
            $crate::tracing::debug!($($arg)*);
        }
    }};
}

/// Logs a message at the INFO level every N occurrences.
#[macro_export]
macro_rules! info_every_n {
    ($n:expr, $($arg:tt)*) => {{
        static STATE: $crate::CountState = $crate::CountState::new();
        if STATE.should_log($n) {
            $crate::tracing::info!($($arg)*);
        }
    }};
}

/// Logs a message at the WARN level every N occurrences.
#[macro_export]
macro_rules! warn_every_n {
    ($n:expr, $($arg:tt)*) => {{
        static STATE: $crate::CountState = $crate::CountState::new();
        if STATE.should_log($n) {
            $crate::tracing::warn!($($arg)*);
        }
    }};
}

/// Logs a message at the ERROR level every N occurrences.
#[macro_export]
macro_rules! error_every_n {
    ($n:expr, $($arg:tt)*) => {{
        static STATE: $crate::CountState = $crate::CountState::new();
        if STATE.should_log($n) {
            $crate::tracing::error!($($arg)*);
        }
    }};
}

// ============================================================================
// *_first_n! macros - log only the first N occurrences
// ============================================================================

/// Logs a message at the TRACE level only for the first N occurrences.
#[macro_export]
macro_rules! trace_first_n {
    ($n:expr, $($arg:tt)*) => {{
        static STATE: $crate::FirstNState = $crate::FirstNState::new();
        if STATE.should_log($n) {
            $crate::tracing::trace!($($arg)*);
        }
    }};
}

/// Logs a message at the DEBUG level only for the first N occurrences.
#[macro_export]
macro_rules! debug_first_n {
    ($n:expr, $($arg:tt)*) => {{
        static STATE: $crate::FirstNState = $crate::FirstNState::new();
        if STATE.should_log($n) {
            $crate::tracing::debug!($($arg)*);
        }
    }};
}

/// Logs a message at the INFO level only for the first N occurrences.
#[macro_export]
macro_rules! info_first_n {
    ($n:expr, $($arg:tt)*) => {{
        static STATE: $crate::FirstNState = $crate::FirstNState::new();
        if STATE.should_log($n) {
            $crate::tracing::info!($($arg)*);
        }
    }};
}

/// Logs a message at the WARN level only for the first N occurrences.
#[macro_export]
macro_rules! warn_first_n {
    ($n:expr, $($arg:tt)*) => {{
        static STATE: $crate::FirstNState = $crate::FirstNState::new();
        if STATE.should_log($n) {
            $crate::tracing::warn!($($arg)*);
        }
    }};
}

/// Logs a message at the ERROR level only for the first N occurrences.
#[macro_export]
macro_rules! error_first_n {
    ($n:expr, $($arg:tt)*) => {{
        static STATE: $crate::FirstNState = $crate::FirstNState::new();
        if STATE.should_log($n) {
            $crate::tracing::error!($($arg)*);
        }
    }};
}

// ============================================================================
// *_backoff! macros - log with exponential backoff
// ============================================================================

/// Logs a message at the TRACE level with exponential backoff.
/// Starts at `initial` interval, doubles after each log, caps at `max`.
#[macro_export]
macro_rules! trace_backoff {
    ($initial:expr, $max:expr, $($arg:tt)*) => {{
        static STATE: $crate::BackoffState = $crate::BackoffState::new();
        if STATE.should_log($initial, $max) {
            $crate::tracing::trace!($($arg)*);
        }
    }};
}

/// Logs a message at the DEBUG level with exponential backoff.
/// Starts at `initial` interval, doubles after each log, caps at `max`.
#[macro_export]
macro_rules! debug_backoff {
    ($initial:expr, $max:expr, $($arg:tt)*) => {{
        static STATE: $crate::BackoffState = $crate::BackoffState::new();
        if STATE.should_log($initial, $max) {
            $crate::tracing::debug!($($arg)*);
        }
    }};
}

/// Logs a message at the INFO level with exponential backoff.
/// Starts at `initial` interval, doubles after each log, caps at `max`.
#[macro_export]
macro_rules! info_backoff {
    ($initial:expr, $max:expr, $($arg:tt)*) => {{
        static STATE: $crate::BackoffState = $crate::BackoffState::new();
        if STATE.should_log($initial, $max) {
            $crate::tracing::info!($($arg)*);
        }
    }};
}

/// Logs a message at the WARN level with exponential backoff.
/// Starts at `initial` interval, doubles after each log, caps at `max`.
#[macro_export]
macro_rules! warn_backoff {
    ($initial:expr, $max:expr, $($arg:tt)*) => {{
        static STATE: $crate::BackoffState = $crate::BackoffState::new();
        if STATE.should_log($initial, $max) {
            $crate::tracing::warn!($($arg)*);
        }
    }};
}

/// Logs a message at the ERROR level with exponential backoff.
/// Starts at `initial` interval, doubles after each log, caps at `max`.
#[macro_export]
macro_rules! error_backoff {
    ($initial:expr, $max:expr, $($arg:tt)*) => {{
        static STATE: $crate::BackoffState = $crate::BackoffState::new();
        if STATE.should_log($initial, $max) {
            $crate::tracing::error!($($arg)*);
        }
    }};
}

// ============================================================================
// *_on_change! macros - log only when value changes
// ============================================================================

/// Logs a message at the TRACE level only when the tracked value changes.
#[macro_export]
macro_rules! trace_on_change {
    ($value:expr, $($arg:tt)*) => {{
        static STATE: $crate::OnChangeState = $crate::OnChangeState::new();
        if STATE.should_log(&$value) {
            $crate::tracing::trace!($($arg)*);
        }
    }};
}

/// Logs a message at the DEBUG level only when the tracked value changes.
#[macro_export]
macro_rules! debug_on_change {
    ($value:expr, $($arg:tt)*) => {{
        static STATE: $crate::OnChangeState = $crate::OnChangeState::new();
        if STATE.should_log(&$value) {
            $crate::tracing::debug!($($arg)*);
        }
    }};
}

/// Logs a message at the INFO level only when the tracked value changes.
#[macro_export]
macro_rules! info_on_change {
    ($value:expr, $($arg:tt)*) => {{
        static STATE: $crate::OnChangeState = $crate::OnChangeState::new();
        if STATE.should_log(&$value) {
            $crate::tracing::info!($($arg)*);
        }
    }};
}

/// Logs a message at the WARN level only when the tracked value changes.
#[macro_export]
macro_rules! warn_on_change {
    ($value:expr, $($arg:tt)*) => {{
        static STATE: $crate::OnChangeState = $crate::OnChangeState::new();
        if STATE.should_log(&$value) {
            $crate::tracing::warn!($($arg)*);
        }
    }};
}

/// Logs a message at the ERROR level only when the tracked value changes.
#[macro_export]
macro_rules! error_on_change {
    ($value:expr, $($arg:tt)*) => {{
        static STATE: $crate::OnChangeState = $crate::OnChangeState::new();
        if STATE.should_log(&$value) {
            $crate::tracing::error!($($arg)*);
        }
    }};
}

// ============================================================================
// *_once_per_value! macros - log once per unique value
// ============================================================================

/// Logs a message at the TRACE level once per unique value.
#[macro_export]
macro_rules! trace_once_per_value {
    ($value:expr, $($arg:tt)*) => {{
        static STATE: std::sync::OnceLock<$crate::OncePerValueState> = std::sync::OnceLock::new();
        if STATE.get_or_init($crate::OncePerValueState::new).should_log(&$value) {
            $crate::tracing::trace!($($arg)*);
        }
    }};
}

/// Logs a message at the DEBUG level once per unique value.
#[macro_export]
macro_rules! debug_once_per_value {
    ($value:expr, $($arg:tt)*) => {{
        static STATE: std::sync::OnceLock<$crate::OncePerValueState> = std::sync::OnceLock::new();
        if STATE.get_or_init($crate::OncePerValueState::new).should_log(&$value) {
            $crate::tracing::debug!($($arg)*);
        }
    }};
}

/// Logs a message at the INFO level once per unique value.
#[macro_export]
macro_rules! info_once_per_value {
    ($value:expr, $($arg:tt)*) => {{
        static STATE: std::sync::OnceLock<$crate::OncePerValueState> = std::sync::OnceLock::new();
        if STATE.get_or_init($crate::OncePerValueState::new).should_log(&$value) {
            $crate::tracing::info!($($arg)*);
        }
    }};
}

/// Logs a message at the WARN level once per unique value.
#[macro_export]
macro_rules! warn_once_per_value {
    ($value:expr, $($arg:tt)*) => {{
        static STATE: std::sync::OnceLock<$crate::OncePerValueState> = std::sync::OnceLock::new();
        if STATE.get_or_init($crate::OncePerValueState::new).should_log(&$value) {
            $crate::tracing::warn!($($arg)*);
        }
    }};
}

/// Logs a message at the ERROR level once per unique value.
#[macro_export]
macro_rules! error_once_per_value {
    ($value:expr, $($arg:tt)*) => {{
        static STATE: std::sync::OnceLock<$crate::OncePerValueState> = std::sync::OnceLock::new();
        if STATE.get_or_init($crate::OncePerValueState::new).should_log(&$value) {
            $crate::tracing::error!($($arg)*);
        }
    }};
}

// ============================================================================
// *_sample! macros - log with probability sampling
// ============================================================================

/// Logs a message at the TRACE level with the given probability (0.0 to 1.0).
#[macro_export]
macro_rules! trace_sample {
    ($probability:expr, $($arg:tt)*) => {{
        static STATE: $crate::SampleState = $crate::SampleState::new();
        if STATE.should_log($probability) {
            $crate::tracing::trace!($($arg)*);
        }
    }};
}

/// Logs a message at the DEBUG level with the given probability (0.0 to 1.0).
#[macro_export]
macro_rules! debug_sample {
    ($probability:expr, $($arg:tt)*) => {{
        static STATE: $crate::SampleState = $crate::SampleState::new();
        if STATE.should_log($probability) {
            $crate::tracing::debug!($($arg)*);
        }
    }};
}

/// Logs a message at the INFO level with the given probability (0.0 to 1.0).
#[macro_export]
macro_rules! info_sample {
    ($probability:expr, $($arg:tt)*) => {{
        static STATE: $crate::SampleState = $crate::SampleState::new();
        if STATE.should_log($probability) {
            $crate::tracing::info!($($arg)*);
        }
    }};
}

/// Logs a message at the WARN level with the given probability (0.0 to 1.0).
#[macro_export]
macro_rules! warn_sample {
    ($probability:expr, $($arg:tt)*) => {{
        static STATE: $crate::SampleState = $crate::SampleState::new();
        if STATE.should_log($probability) {
            $crate::tracing::warn!($($arg)*);
        }
    }};
}

/// Logs a message at the ERROR level with the given probability (0.0 to 1.0).
#[macro_export]
macro_rules! error_sample {
    ($probability:expr, $($arg:tt)*) => {{
        static STATE: $crate::SampleState = $crate::SampleState::new();
        if STATE.should_log($probability) {
            $crate::tracing::error!($($arg)*);
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn once_state_logs_only_first_time() {
        let state = OnceState::new();
        assert!(state.should_log());
        assert!(!state.should_log());
        assert!(!state.should_log());
    }

    #[test]
    fn count_state_logs_every_n() {
        let state = CountState::new();
        // With n=3: logs on 0, 3, 6, 9...
        assert!(state.should_log(3)); // count 0
        assert!(!state.should_log(3)); // count 1
        assert!(!state.should_log(3)); // count 2
        assert!(state.should_log(3)); // count 3
        assert!(!state.should_log(3)); // count 4
        assert!(!state.should_log(3)); // count 5
        assert!(state.should_log(3)); // count 6
    }

    #[test]
    fn throttle_state_respects_duration() {
        let state = ThrottleState::new();
        let short_duration = Duration::from_millis(50);

        // First call always logs
        assert!(state.should_log(short_duration));

        // Immediate second call should not log
        assert!(!state.should_log(short_duration));

        // Wait for duration to pass
        std::thread::sleep(Duration::from_millis(60));

        // Now it should log again
        assert!(state.should_log(short_duration));
    }

    #[test]
    fn once_macro_logs_once() {
        for _ in 0..5 {
            trace_once!("test message");
            // We can't easily verify tracing output, but we can verify the macro compiles
        }
    }

    #[test]
    fn every_macro_compiles() {
        for i in 0..5 {
            debug_every!(Duration::from_secs(1), "iteration {}", i);
        }
    }

    #[test]
    fn every_n_macro_compiles() {
        for i in 0..10 {
            info_every_n!(3, "iteration {}", i);
        }
    }

    #[test]
    fn first_n_state_logs_first_n_only() {
        let state = FirstNState::new();
        // With n=3: logs on 0, 1, 2, then stops
        assert!(state.should_log(3)); // count 0
        assert!(state.should_log(3)); // count 1
        assert!(state.should_log(3)); // count 2
        assert!(!state.should_log(3)); // count 3
        assert!(!state.should_log(3)); // count 4
    }

    #[test]
    fn backoff_state_doubles_interval() {
        let state = BackoffState::new();
        let initial = Duration::from_millis(20);
        let max = Duration::from_millis(100);

        // First call always logs
        assert!(state.should_log(initial, max));

        // Immediate call should not log
        assert!(!state.should_log(initial, max));

        // Wait for initial interval
        std::thread::sleep(Duration::from_millis(25));
        assert!(state.should_log(initial, max));

        // Now interval is 40ms, so 25ms wait shouldn't be enough
        std::thread::sleep(Duration::from_millis(25));
        assert!(!state.should_log(initial, max));

        // Wait more to reach 40ms total
        std::thread::sleep(Duration::from_millis(20));
        assert!(state.should_log(initial, max));
    }

    #[test]
    fn on_change_state_detects_changes() {
        let state = OnChangeState::new();

        // First call always logs
        assert!(state.should_log(&42));

        // Same value should not log
        assert!(!state.should_log(&42));

        // Different value should log
        assert!(state.should_log(&100));

        // Same new value should not log
        assert!(!state.should_log(&100));

        // Back to original value should log (it changed)
        assert!(state.should_log(&42));
    }

    #[test]
    fn once_per_value_state_logs_unique_values() {
        let state = OncePerValueState::new();

        // First time seeing each value
        assert!(state.should_log(&"apple"));
        assert!(state.should_log(&"banana"));
        assert!(state.should_log(&"cherry"));

        // Second time seeing values
        assert!(!state.should_log(&"apple"));
        assert!(!state.should_log(&"banana"));

        // New value still logs
        assert!(state.should_log(&"date"));
    }

    #[test]
    fn sample_state_respects_probability() {
        let state = SampleState::new();

        // With probability 0, should never log
        let mut logged = false;
        for _ in 0..100 {
            if state.should_log(0.0) {
                logged = true;
            }
        }
        assert!(!logged);

        // With probability 1, should always log
        for _ in 0..10 {
            assert!(state.should_log(1.0));
        }
    }

    #[test]
    fn first_n_macro_compiles() {
        for i in 0..10 {
            info_first_n!(3, "iteration {}", i);
        }
    }

    #[test]
    fn backoff_macro_compiles() {
        for i in 0..5 {
            warn_backoff!(Duration::from_secs(1), Duration::from_secs(60), "iteration {}", i);
        }
    }

    #[test]
    fn on_change_macro_compiles() {
        for i in 0..5 {
            let value = i % 2;
            debug_on_change!(value, "value changed to {}", value);
        }
    }

    #[test]
    fn once_per_value_macro_compiles() {
        for i in 0..10 {
            let key = i % 3;
            info_once_per_value!(key, "first time seeing {}", key);
        }
    }

    #[test]
    fn sample_macro_compiles() {
        for i in 0..10 {
            trace_sample!(0.5, "sampled iteration {}", i);
        }
    }
}
