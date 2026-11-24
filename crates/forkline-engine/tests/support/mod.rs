//! The leak check every async engine test ends with.
#![allow(unreachable_pub)]

use std::time::Duration;

/// Fail unless every task the test's requests started has ended within 1 s:
/// spawned fork paths, detached paths, anything a module spawned. A paused
/// test clock advances by itself while this waits.
pub async fn assert_settled() {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        let alive = tokio::runtime::Handle::current().metrics().num_alive_tasks();
        if alive == 0 {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "leak check: {alive} task(s) still alive 1 s after the test's requests ended"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
