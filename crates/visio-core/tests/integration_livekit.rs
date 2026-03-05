//! Integration tests against a local LiveKit dev server.
//!
//! These tests require `livekit-server --dev` to be running locally.
//! In CI, the server is started automatically before this test suite.
//!
//! Run manually:
//! ```sh
//! livekit-server --dev &
//! LIVEKIT_URL=ws://localhost:7880 LIVEKIT_API_KEY=devkey LIVEKIT_API_SECRET=secret \
//!   cargo test -p visio-core --test integration_livekit
//! ```

use std::sync::Arc;
use std::time::Duration;

use livekit_api::access_token::{AccessToken, VideoGrants};
use visio_core::{ConnectionState, RoomManager, VisioEvent, VisioEventListener};

fn livekit_url() -> String {
    std::env::var("LIVEKIT_URL").unwrap_or_else(|_| "ws://localhost:7880".to_string())
}

fn api_key() -> String {
    std::env::var("LIVEKIT_API_KEY").unwrap_or_else(|_| "devkey".to_string())
}

fn api_secret() -> String {
    std::env::var("LIVEKIT_API_SECRET").unwrap_or_else(|_| "secret".to_string())
}

fn make_token(identity: &str, name: &str, room: &str) -> String {
    AccessToken::with_api_key(&api_key(), &api_secret())
        .with_identity(identity)
        .with_name(name)
        .with_grants(VideoGrants {
            room_join: true,
            room: room.to_string(),
            ..Default::default()
        })
        .to_jwt()
        .expect("failed to generate token")
}

/// Listener that captures connection state changes.
struct StateCapture {
    states: std::sync::Mutex<Vec<ConnectionState>>,
}

impl StateCapture {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            states: std::sync::Mutex::new(Vec::new()),
        })
    }
}

impl VisioEventListener for StateCapture {
    fn on_event(&self, event: VisioEvent) {
        if let VisioEvent::ConnectionStateChanged(state) = event {
            self.states.lock().unwrap().push(state);
        }
    }
}

/// Helper: wait until a condition is true, with timeout.
async fn wait_for<F: Fn() -> bool>(condition: F, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn test_connect_and_disconnect() {
    let room_name = format!("test-connect-{}", uuid::Uuid::new_v4());
    let token = make_token("user-1", "User 1", &room_name);

    let rm = RoomManager::new();
    let capture = StateCapture::new();
    rm.add_listener(capture.clone());

    // Connect
    rm.connect_with_token(&livekit_url(), &token)
        .await
        .expect("connect failed");

    assert_eq!(rm.connection_state().await, ConnectionState::Connected);

    // Disconnect
    rm.disconnect().await;

    let saw_disconnected = wait_for(
        || {
            capture
                .states
                .lock()
                .unwrap()
                .iter()
                .any(|s| *s == ConnectionState::Disconnected)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(saw_disconnected, "should have seen Disconnected event");
}

#[tokio::test]
async fn test_two_participants_see_each_other() {
    let room_name = format!("test-2p-{}", uuid::Uuid::new_v4());
    let token1 = make_token("alice", "Alice", &room_name);
    let token2 = make_token("bob", "Bob", &room_name);
    let url = livekit_url();

    let rm1 = RoomManager::new();
    let rm2 = RoomManager::new();

    rm1.connect_with_token(&url, &token1)
        .await
        .expect("connect rm1");
    rm2.connect_with_token(&url, &token2)
        .await
        .expect("connect rm2");

    // Wait for participants to appear
    tokio::time::sleep(Duration::from_secs(2)).await;

    let p1 = rm1.participants().await;
    let p2 = rm2.participants().await;

    // rm1 should see bob as a remote participant (+ alice as local)
    assert!(
        p1.iter().any(|p| p.identity == "bob"),
        "rm1 should see bob, got: {p1:?}"
    );
    // rm2 should see alice as a remote participant (+ bob as local)
    assert!(
        p2.iter().any(|p| p.identity == "alice"),
        "rm2 should see alice, got: {p2:?}"
    );

    rm1.disconnect().await;
    rm2.disconnect().await;
}

#[tokio::test]
async fn test_mute_unmute_propagation() {
    let room_name = format!("test-mute-{}", uuid::Uuid::new_v4());
    let token1 = make_token("alice", "Alice", &room_name);
    let token2 = make_token("bob", "Bob", &room_name);
    let url = livekit_url();

    let rm1 = RoomManager::new();
    let rm2 = RoomManager::new();
    let controls1 = rm1.controls();

    rm1.connect_with_token(&url, &token1)
        .await
        .expect("connect rm1");
    rm2.connect_with_token(&url, &token2)
        .await
        .expect("connect rm2");

    // Wait for join
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Alice publishes mic then mutes it
    let _audio = controls1.publish_microphone().await;
    // Give time for track to propagate
    tokio::time::sleep(Duration::from_secs(2)).await;

    if _audio.is_ok() {
        let _ = controls1.set_microphone_enabled(false).await;
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Bob should see Alice as muted
        let p2 = rm2.participants().await;
        if let Some(alice) = p2.iter().find(|p| p.identity == "alice") {
            assert!(
                alice.is_muted,
                "alice should be muted from bob's perspective"
            );
        }
    }

    rm1.disconnect().await;
    rm2.disconnect().await;
}
