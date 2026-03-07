# Restricted Rooms with Invitation Management — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `restricted` access level to room creation with full invitation management (user search, add/remove members) at creation time and in-call, across Desktop, Android, and iOS.

**Architecture:** New `AccessService` in visio-core handles all Meet API calls for user search and resource-access CRUD. Exposed through UniFFI to mobile platforms and directly to Desktop. Each platform adds a 3rd radio option ("Restricted") to the create-room dialog, an autocomplete user-search field, and a members section in Room Info tab.

**Tech Stack:** Rust (visio-core, reqwest, serde), UniFFI (UDL), Tauri 2.x (Desktop), Kotlin/Jetpack Compose (Android), SwiftUI (iOS), i18n JSON

---

## Task 1: i18n — Add restricted rooms translation keys

**Files:**
- Modify: `i18n/en.json`
- Modify: `i18n/fr.json`
- Modify: `i18n/de.json`
- Modify: `i18n/es.json`
- Modify: `i18n/it.json`
- Modify: `i18n/nl.json`

**Step 1: Add keys to all 6 language files**

Add these keys to each file (shown here in English — translate for each language):

```json
"home.createRoom.restricted": "Restricted",
"home.createRoom.restrictedDesc": "Invitation only — only invited members can join",
"restricted.searchUsers": "Search users by email",
"restricted.invite": "Invite members",
"restricted.members": "Members",
"restricted.remove": "Remove",
"restricted.owner": "Owner",
"restricted.admin": "Admin",
"restricted.member": "Member",
"restricted.alreadyInvited": "Already invited",
"restricted.searchUnavailable": "User search not available on this server",
"restricted.noResults": "No users found"
```

**Step 2: Copy i18n to Android assets**

Run: `cp i18n/*.json android/app/src/main/assets/i18n/`

**Step 3: Commit**

```bash
git add -f i18n/*.json android/app/src/main/assets/i18n/*.json
git commit -m "feat(i18n): add restricted rooms translation keys"
```

---

## Task 2: visio-core — AccessService types and deserialization tests

**Files:**
- Create: `crates/visio-core/src/access.rs`

**Step 1: Write types and deserialization tests**

Create `crates/visio-core/src/access.rs` with types + tests:

```rust
use serde::Deserialize;

use crate::auth::AuthService;
use crate::errors::VisioError;

/// A user returned by the Meet user search API.
#[derive(Debug, Clone, Deserialize)]
pub struct UserSearchResult {
    pub id: String,
    pub email: String,
    pub full_name: Option<String>,
    pub short_name: Option<String>,
}

/// A resource access entry (user linked to a room with a role).
#[derive(Debug, Clone, Deserialize)]
pub struct RoomAccess {
    pub id: String,
    pub user: UserSearchResult,
    pub resource: String,
    pub role: String,
}

/// Paginated response wrapper from Meet API.
#[derive(Debug, Deserialize)]
struct PaginatedResponse<T> {
    results: Vec<T>,
}

/// Service for managing room access (restricted rooms).
pub struct AccessService;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_user_search_result() {
        let json = r#"{"id": "abc-123", "email": "alice@example.com", "full_name": "Alice Doe", "short_name": "Alice"}"#;
        let user: UserSearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(user.id, "abc-123");
        assert_eq!(user.email, "alice@example.com");
        assert_eq!(user.full_name, Some("Alice Doe".to_string()));
    }

    #[test]
    fn parse_user_search_result_minimal() {
        let json = r#"{"id": "abc-123", "email": "alice@example.com"}"#;
        let user: UserSearchResult = serde_json::from_str(json).unwrap();
        assert!(user.full_name.is_none());
        assert!(user.short_name.is_none());
    }

    #[test]
    fn parse_room_access() {
        let json = r#"{
            "id": "ra-1",
            "user": {"id": "u-1", "email": "bob@example.com", "full_name": "Bob", "short_name": null},
            "resource": "room-123",
            "role": "member"
        }"#;
        let access: RoomAccess = serde_json::from_str(json).unwrap();
        assert_eq!(access.id, "ra-1");
        assert_eq!(access.user.email, "bob@example.com");
        assert_eq!(access.role, "member");
    }

    #[test]
    fn parse_paginated_users() {
        let json = r#"{"count": 2, "next": null, "previous": null, "results": [
            {"id": "u-1", "email": "a@b.com", "full_name": "A", "short_name": null},
            {"id": "u-2", "email": "c@d.com", "full_name": "C", "short_name": null}
        ]}"#;
        let page: PaginatedResponse<UserSearchResult> = serde_json::from_str(json).unwrap();
        assert_eq!(page.results.len(), 2);
    }

    #[test]
    fn parse_paginated_accesses() {
        let json = r#"{"count": 1, "next": null, "previous": null, "results": [
            {"id": "ra-1", "user": {"id": "u-1", "email": "a@b.com", "full_name": null, "short_name": null}, "resource": "room-id", "role": "owner"}
        ]}"#;
        let page: PaginatedResponse<RoomAccess> = serde_json::from_str(json).unwrap();
        assert_eq!(page.results.len(), 1);
        assert_eq!(page.results[0].role, "owner");
    }
}
```

**Step 2: Run tests to verify they pass**

Run: `cargo test -p visio-core -- access`
Expected: 5 tests PASS

**Step 3: Commit**

```bash
git add crates/visio-core/src/access.rs
git commit -m "feat(core): add AccessService types with deserialization tests"
```

---

## Task 3: visio-core — AccessService API methods

**Files:**
- Modify: `crates/visio-core/src/access.rs`

**Step 1: Implement the 4 API methods**

Add to `AccessService` impl block in `access.rs`:

```rust
impl AccessService {
    /// Search users by email (trigram similarity).
    /// Calls `GET /api/v1.0/users/?q={query}`.
    /// Requires `ALLOW_UNSECURE_USER_LISTING` on server.
    pub async fn search_users(
        meet_url: &str,
        session_cookie: &str,
        query: &str,
    ) -> Result<Vec<UserSearchResult>, VisioError> {
        let (instance, _slug) = AuthService::parse_meet_url(meet_url)?;

        let api_url = format!(
            "https://{}/api/v1.0/users/?q={}",
            instance,
            urlencoding::encode(query)
        );

        let client = reqwest::Client::new();
        let resp = client
            .get(&api_url)
            .header(
                reqwest::header::COOKIE,
                format!("sessionid={}", session_cookie),
            )
            .send()
            .await
            .map_err(|e| VisioError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(VisioError::Auth(format!(
                "user search returned status {}",
                resp.status()
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| VisioError::Http(e.to_string()))?;

        let page: PaginatedResponse<UserSearchResult> = serde_json::from_str(&body)
            .map_err(|e| VisioError::Auth(format!("invalid user search response: {e}")))?;

        Ok(page.results)
    }

    /// List accesses for a room.
    /// Calls `GET /api/v1.0/resource-accesses/?resource={room_id}`.
    pub async fn list_accesses(
        meet_url: &str,
        session_cookie: &str,
        room_id: &str,
    ) -> Result<Vec<RoomAccess>, VisioError> {
        let (instance, _slug) = AuthService::parse_meet_url(meet_url)?;

        let api_url = format!(
            "https://{}/api/v1.0/resource-accesses/?resource={}",
            instance,
            urlencoding::encode(room_id)
        );

        let client = reqwest::Client::new();
        let resp = client
            .get(&api_url)
            .header(
                reqwest::header::COOKIE,
                format!("sessionid={}", session_cookie),
            )
            .send()
            .await
            .map_err(|e| VisioError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(VisioError::Auth(format!(
                "list-accesses returned status {}",
                resp.status()
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| VisioError::Http(e.to_string()))?;

        let page: PaginatedResponse<RoomAccess> = serde_json::from_str(&body)
            .map_err(|e| VisioError::Auth(format!("invalid accesses response: {e}")))?;

        Ok(page.results)
    }

    /// Add a user as member of a room.
    /// Calls `POST /api/v1.0/resource-accesses/`.
    pub async fn add_access(
        meet_url: &str,
        session_cookie: &str,
        user_id: &str,
        room_id: &str,
    ) -> Result<RoomAccess, VisioError> {
        use rand::Rng;

        let (instance, _slug) = AuthService::parse_meet_url(meet_url)?;

        let api_url = format!("https://{}/api/v1.0/resource-accesses/", instance);

        let csrf_bytes: [u8; 32] = rand::thread_rng().r#gen();
        let csrf_token: String = csrf_bytes.iter().map(|b| format!("{:02x}", b)).collect();

        let cookie_header = format!(
            "sessionid={}; csrftoken={}",
            session_cookie, csrf_token
        );

        let body = serde_json::json!({
            "user": user_id,
            "resource": room_id,
            "role": "member",
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(&api_url)
            .header(reqwest::header::COOKIE, &cookie_header)
            .header("X-CSRFToken", &csrf_token)
            .header("Referer", format!("https://{}/", instance))
            .json(&body)
            .send()
            .await
            .map_err(|e| VisioError::Http(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::BAD_REQUEST {
            return Err(VisioError::Session("Already invited".to_string()));
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(VisioError::Auth(format!(
                "add-access returned status {}: {}",
                status, body
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| VisioError::Http(e.to_string()))?;

        serde_json::from_str(&body)
            .map_err(|e| VisioError::Auth(format!("invalid add-access response: {e}")))
    }

    /// Remove an access (revoke membership).
    /// Calls `DELETE /api/v1.0/resource-accesses/{access_id}/`.
    pub async fn remove_access(
        meet_url: &str,
        session_cookie: &str,
        access_id: &str,
    ) -> Result<(), VisioError> {
        use rand::Rng;

        let (instance, _slug) = AuthService::parse_meet_url(meet_url)?;

        let api_url = format!(
            "https://{}/api/v1.0/resource-accesses/{}/",
            instance, access_id
        );

        let csrf_bytes: [u8; 32] = rand::thread_rng().r#gen();
        let csrf_token: String = csrf_bytes.iter().map(|b| format!("{:02x}", b)).collect();

        let cookie_header = format!(
            "sessionid={}; csrftoken={}",
            session_cookie, csrf_token
        );

        let client = reqwest::Client::new();
        let resp = client
            .delete(&api_url)
            .header(reqwest::header::COOKIE, &cookie_header)
            .header("X-CSRFToken", &csrf_token)
            .header("Referer", format!("https://{}/", instance))
            .send()
            .await
            .map_err(|e| VisioError::Http(e.to_string()))?;

        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::NO_CONTENT {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(VisioError::Auth(format!(
                "remove-access returned status {}: {}",
                status, body
            )));
        }

        Ok(())
    }
}
```

**Step 2: Add a test for invalid URL error**

Add to the `tests` module:

```rust
#[tokio::test]
async fn search_users_invalid_url_returns_error() {
    assert!(AccessService::search_users("invalid", "cookie", "query").await.is_err());
}
```

**Step 3: Run tests**

Run: `cargo test -p visio-core -- access`
Expected: 6 tests PASS

**Step 4: Commit**

```bash
git add crates/visio-core/src/access.rs
git commit -m "feat(core): implement AccessService API methods"
```

---

## Task 4: visio-core — Register access module and add re-exports

**Files:**
- Modify: `crates/visio-core/src/lib.rs`

**Step 1: Add module and re-exports**

Add `pub mod access;` after `pub mod auth;` (line 8) and add re-exports:

```rust
pub mod access;
```

Add to the re-exports section:

```rust
pub use access::{AccessService, RoomAccess, UserSearchResult};
```

**Step 2: Add `room_id` field to `CreateRoomResponse`**

The `CreateRoomResponse` in `session.rs` needs an `id` field so we can pass it to the access APIs.

Check current `CreateRoomResponse` struct and add `id: String` field if not already present.

In `crates/visio-core/src/session.rs`, find `CreateRoomResponse` and ensure it has:

```rust
pub struct CreateRoomResponse {
    pub id: String,  // <-- Add this if missing
    pub slug: String,
    pub name: String,
    pub access_level: String,
    pub livekit: Option<CreateRoomLiveKit>,
}
```

**Step 3: Verify build**

Run: `cargo build -p visio-core`
Expected: Success

**Step 4: Commit**

```bash
git add crates/visio-core/src/lib.rs crates/visio-core/src/session.rs
git commit -m "feat(core): register access module with re-exports"
```

---

## Task 5: visio-ffi — Expose access types and methods via UniFFI

**Files:**
- Modify: `crates/visio-ffi/src/visio.udl`
- Modify: `crates/visio-ffi/src/lib.rs`

**Step 1: Add types to visio.udl**

After the `WaitingParticipant` dictionary (line 121), add:

```
dictionary UserSearchResult {
    string id;
    string email;
    string? full_name;
    string? short_name;
};

dictionary RoomAccess {
    string id;
    UserSearchResult user;
    string resource;
    string role;
};
```

Update `CreateRoomResult` to include `id`:

```
dictionary CreateRoomResult {
    string id;
    string slug;
    string name;
    string access_level;
    string livekit_url;
    string livekit_token;
};
```

Add methods to the `VisioClient` interface (after `deny_participant`):

```
[Throws=VisioError]
sequence<UserSearchResult> search_users(string query);

[Throws=VisioError]
sequence<RoomAccess> list_accesses(string room_id);

[Throws=VisioError]
RoomAccess add_access(string user_id, string room_id);

[Throws=VisioError]
void remove_access(string access_id);
```

**Step 2: Add FFI types and From impls to lib.rs**

In `crates/visio-ffi/src/lib.rs`, add the FFI struct definitions:

```rust
pub struct UserSearchResult {
    pub id: String,
    pub email: String,
    pub full_name: Option<String>,
    pub short_name: Option<String>,
}

pub struct RoomAccess {
    pub id: String,
    pub user: UserSearchResult,
    pub resource: String,
    pub role: String,
}

impl From<visio_core::UserSearchResult> for UserSearchResult {
    fn from(u: visio_core::UserSearchResult) -> Self {
        Self {
            id: u.id,
            email: u.email,
            full_name: u.full_name,
            short_name: u.short_name,
        }
    }
}

impl From<visio_core::RoomAccess> for RoomAccess {
    fn from(a: visio_core::RoomAccess) -> Self {
        Self {
            id: a.id,
            user: a.user.into(),
            resource: a.resource,
            role: a.role,
        }
    }
}
```

**Step 3: Add methods to VisioClient impl**

Add to the VisioClient FFI impl (follow same pattern as lobby methods):

```rust
pub fn search_users(&self, query: String) -> Result<Vec<UserSearchResult>, VisioError> {
    let session = self.session_manager.lock().unwrap();
    let cookie = session.cookie().ok_or_else(|| {
        VisioError::Session { msg: "Not authenticated".to_string() }
    })?;
    let meet_instance = session.meet_instance().ok_or_else(|| {
        VisioError::Session { msg: "No meet instance".to_string() }
    })?;
    let meet_url = format!("https://{}/room", meet_instance); // just need instance for URL parsing
    drop(session);

    let results = self.rt.block_on(
        visio_core::AccessService::search_users(&meet_url, &cookie, &query)
    ).map_err(VisioError::from)?;

    Ok(results.into_iter().map(|u| u.into()).collect())
}

pub fn list_accesses(&self, room_id: String) -> Result<Vec<RoomAccess>, VisioError> {
    let session = self.session_manager.lock().unwrap();
    let cookie = session.cookie().ok_or_else(|| {
        VisioError::Session { msg: "Not authenticated".to_string() }
    })?;
    let meet_instance = session.meet_instance().ok_or_else(|| {
        VisioError::Session { msg: "No meet instance".to_string() }
    })?;
    let meet_url = format!("https://{}/room", meet_instance);
    drop(session);

    let results = self.rt.block_on(
        visio_core::AccessService::list_accesses(&meet_url, &cookie, &room_id)
    ).map_err(VisioError::from)?;

    Ok(results.into_iter().map(|a| a.into()).collect())
}

pub fn add_access(&self, user_id: String, room_id: String) -> Result<RoomAccess, VisioError> {
    let session = self.session_manager.lock().unwrap();
    let cookie = session.cookie().ok_or_else(|| {
        VisioError::Session { msg: "Not authenticated".to_string() }
    })?;
    let meet_instance = session.meet_instance().ok_or_else(|| {
        VisioError::Session { msg: "No meet instance".to_string() }
    })?;
    let meet_url = format!("https://{}/room", meet_instance);
    drop(session);

    let result = self.rt.block_on(
        visio_core::AccessService::add_access(&meet_url, &cookie, &user_id, &room_id)
    ).map_err(VisioError::from)?;

    Ok(result.into())
}

pub fn remove_access(&self, access_id: String) -> Result<(), VisioError> {
    let session = self.session_manager.lock().unwrap();
    let cookie = session.cookie().ok_or_else(|| {
        VisioError::Session { msg: "Not authenticated".to_string() }
    })?;
    let meet_instance = session.meet_instance().ok_or_else(|| {
        VisioError::Session { msg: "No meet instance".to_string() }
    })?;
    let meet_url = format!("https://{}/room", meet_instance);
    drop(session);

    self.rt.block_on(
        visio_core::AccessService::remove_access(&meet_url, &cookie, &access_id)
    ).map_err(VisioError::from)?;

    Ok(())
}
```

Also update the `create_room` return type to include `id` in the `CreateRoomResult`:

```rust
// In the create_room method, add id to the result:
Ok(CreateRoomResult {
    id: result.id,  // <-- add this
    slug: result.slug,
    ...
})
```

**Step 4: Verify build**

Run: `cargo build -p visio-ffi`
Expected: Success

**Step 5: Commit**

```bash
git add crates/visio-ffi/src/visio.udl crates/visio-ffi/src/lib.rs
git commit -m "feat(ffi): expose access types and methods via UniFFI"
```

---

## Task 6: Desktop — Tauri commands for access management

**Files:**
- Modify: `crates/visio-desktop/src/lib.rs`

**Step 1: Add 4 Tauri commands**

Add after the `cancel_lobby` command:

```rust
#[tauri::command]
async fn search_users(
    state: tauri::State<'_, VisioState>,
    query: String,
) -> Result<serde_json::Value, String> {
    let session = state.session.lock().await;
    let cookie = session.cookie().ok_or("Not authenticated")?;
    let meet_instance = session.meet_instance().ok_or("No meet instance")?;
    let meet_url = format!("https://{}/room", meet_instance);
    drop(session);

    let results = visio_core::AccessService::search_users(&meet_url, &cookie, &query)
        .await
        .map_err(|e| e.to_string())?;

    serde_json::to_value(&results).map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_accesses(
    state: tauri::State<'_, VisioState>,
    room_id: String,
) -> Result<serde_json::Value, String> {
    let session = state.session.lock().await;
    let cookie = session.cookie().ok_or("Not authenticated")?;
    let meet_instance = session.meet_instance().ok_or("No meet instance")?;
    let meet_url = format!("https://{}/room", meet_instance);
    drop(session);

    let results = visio_core::AccessService::list_accesses(&meet_url, &cookie, &room_id)
        .await
        .map_err(|e| e.to_string())?;

    serde_json::to_value(&results).map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_access(
    state: tauri::State<'_, VisioState>,
    user_id: String,
    room_id: String,
) -> Result<serde_json::Value, String> {
    let session = state.session.lock().await;
    let cookie = session.cookie().ok_or("Not authenticated")?;
    let meet_instance = session.meet_instance().ok_or("No meet instance")?;
    let meet_url = format!("https://{}/room", meet_instance);
    drop(session);

    let result = visio_core::AccessService::add_access(&meet_url, &cookie, &user_id, &room_id)
        .await
        .map_err(|e| e.to_string())?;

    serde_json::to_value(&result).map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_access(
    state: tauri::State<'_, VisioState>,
    access_id: String,
) -> Result<(), String> {
    let session = state.session.lock().await;
    let cookie = session.cookie().ok_or("Not authenticated")?;
    let meet_instance = session.meet_instance().ok_or("No meet instance")?;
    let meet_url = format!("https://{}/room", meet_instance);
    drop(session);

    visio_core::AccessService::remove_access(&meet_url, &cookie, &access_id)
        .await
        .map_err(|e| e.to_string())
}
```

**Step 2: Register commands in invoke_handler**

Add `search_users`, `list_accesses`, `add_access`, `remove_access` to the `invoke_handler![]` macro list.

**Step 3: Verify build**

Run: `cargo build -p visio-desktop`
Expected: Success

**Step 4: Commit**

```bash
git add crates/visio-desktop/src/lib.rs
git commit -m "feat(desktop): add Tauri commands for access management"
```

---

## Task 7: Desktop — Restricted option + autocomplete in CreateRoomDialog

**Files:**
- Modify: `crates/visio-desktop/frontend/src/App.tsx`
- Modify: `crates/visio-desktop/frontend/src/App.css`

**Step 1: Add "Restricted" radio option**

In the `CreateRoomDialog` component, find the access level radio group (after the "Trusted" option) and add a 3rd option:

```tsx
<label className="radio-option">
  <input
    type="radio"
    name="access"
    value="restricted"
    checked={accessLevel === "restricted"}
    onChange={() => setAccessLevel("restricted")}
  />
  <div>
    <strong>{t("home.createRoom.restricted")}</strong>
    <div className="radio-desc">{t("home.createRoom.restrictedDesc")}</div>
  </div>
</label>
```

**Step 2: Add autocomplete user search**

When `accessLevel === "restricted"`, show an invite section below the radio buttons:

```tsx
// State for user search
const [searchQuery, setSearchQuery] = useState("");
const [searchResults, setSearchResults] = useState<any[]>([]);
const [invitedUsers, setInvitedUsers] = useState<any[]>([]);
const [searching, setSearching] = useState(false);

// Debounced search (300ms, min 3 chars)
useEffect(() => {
  if (searchQuery.length < 3) {
    setSearchResults([]);
    return;
  }
  const timer = setTimeout(async () => {
    setSearching(true);
    try {
      const results = await invoke("search_users", { query: searchQuery });
      setSearchResults(
        (results as any[]).filter(
          (u) => !invitedUsers.some((inv) => inv.id === u.id)
        )
      );
    } catch {
      setSearchResults([]);
    }
    setSearching(false);
  }, 300);
  return () => clearTimeout(timer);
}, [searchQuery, invitedUsers]);
```

UI for the invite section (shown when `accessLevel === "restricted"` and `createdUrl === null`):

```tsx
{accessLevel === "restricted" && (
  <div className="invite-section">
    <label>{t("restricted.invite")}</label>
    <input
      type="text"
      placeholder={t("restricted.searchUsers")}
      value={searchQuery}
      onChange={(e) => setSearchQuery(e.target.value)}
      className="search-input"
    />
    {searchResults.length > 0 && (
      <div className="search-dropdown">
        {searchResults.map((user) => (
          <div
            key={user.id}
            className="search-result"
            onClick={() => {
              setInvitedUsers([...invitedUsers, user]);
              setSearchQuery("");
              setSearchResults([]);
            }}
          >
            <span className="search-name">{user.full_name || user.email}</span>
            <span className="search-email">{user.email}</span>
          </div>
        ))}
      </div>
    )}
    {invitedUsers.length > 0 && (
      <div className="invited-chips">
        {invitedUsers.map((user) => (
          <span key={user.id} className="user-chip">
            {user.full_name || user.email}
            <button
              className="chip-remove"
              onClick={() =>
                setInvitedUsers(invitedUsers.filter((u) => u.id !== user.id))
              }
            >
              ×
            </button>
          </span>
        ))}
      </div>
    )}
  </div>
)}
```

**Step 3: Add access invitations after room creation**

In the create button's `onClick` handler, after the room is created successfully, add accesses for each invited user:

```tsx
// After successful room creation:
if (accessLevel === "restricted") {
  for (const user of invitedUsers) {
    try {
      await invoke("add_access", { userId: user.id, roomId: result.id });
    } catch (e) {
      console.warn("Failed to add access for", user.email, e);
    }
  }
}
```

**Step 4: Add CSS styles**

Add to `App.css`:

```css
.invite-section { display: flex; flex-direction: column; gap: 8px; }
.search-input { padding: 8px; border: 1px solid var(--border-color); border-radius: 6px; background: var(--input-bg); color: var(--text-color); }
.search-dropdown { background: var(--surface-color); border: 1px solid var(--border-color); border-radius: 6px; max-height: 150px; overflow-y: auto; }
.search-result { padding: 8px 12px; cursor: pointer; display: flex; flex-direction: column; }
.search-result:hover { background: var(--hover-color); }
.search-name { font-weight: 500; }
.search-email { font-size: 0.85em; opacity: 0.7; }
.invited-chips { display: flex; flex-wrap: wrap; gap: 6px; }
.user-chip { display: inline-flex; align-items: center; gap: 4px; padding: 4px 10px; background: var(--primary-color); color: white; border-radius: 16px; font-size: 0.85em; }
.chip-remove { background: none; border: none; color: white; cursor: pointer; font-size: 1.1em; padding: 0; margin-left: 2px; }
```

**Step 5: Verify build**

Run: `cd crates/visio-desktop && cargo tauri dev` (visual check)

**Step 6: Commit**

```bash
git add crates/visio-desktop/frontend/src/App.tsx crates/visio-desktop/frontend/src/App.css
git commit -m "feat(desktop): add restricted access level with user invite in create dialog"
```

---

## Task 8: Desktop — Members section in Room Info sidebar

**Files:**
- Modify: `crates/visio-desktop/frontend/src/App.tsx`
- Modify: `crates/visio-desktop/frontend/src/App.css`

**Step 1: Add members state and fetch logic**

In the main `App` component, add state for room accesses:

```tsx
const [roomId, setRoomId] = useState<string | null>(null);
const [roomAccesses, setRoomAccesses] = useState<any[]>([]);
const [memberSearchQuery, setMemberSearchQuery] = useState("");
const [memberSearchResults, setMemberSearchResults] = useState<any[]>([]);
```

Store `roomId` from the create room result and from join responses.

Add a function to refresh accesses:

```tsx
const refreshAccesses = async () => {
  if (!roomId) return;
  try {
    const results = await invoke("list_accesses", { roomId });
    setRoomAccesses(results as any[]);
  } catch (e) {
    console.warn("Failed to fetch accesses", e);
  }
};
```

**Step 2: Add members section to Room Info tab in sidebar**

In the Room Info display within the participants sidebar (or a separate Room Info section), add a members panel for restricted rooms:

```tsx
{roomId && accessLevel === "restricted" && (
  <div className="members-section">
    <h4>{t("restricted.members")}</h4>
    <div className="member-search">
      <input
        type="text"
        placeholder={t("restricted.searchUsers")}
        value={memberSearchQuery}
        onChange={(e) => setMemberSearchQuery(e.target.value)}
      />
      {/* Search dropdown similar to create dialog */}
    </div>
    <div className="members-list">
      {roomAccesses.map((access) => (
        <div key={access.id} className="member-row">
          <span>{access.user.full_name || access.user.email}</span>
          <span className="member-role">{t(`restricted.${access.role}`)}</span>
          {access.role === "member" && (
            <button
              className="btn-remove"
              onClick={async () => {
                await invoke("remove_access", { accessId: access.id });
                refreshAccesses();
              }}
            >
              {t("restricted.remove")}
            </button>
          )}
        </div>
      ))}
    </div>
  </div>
)}
```

**Step 3: Add CSS styles for members**

```css
.members-section { padding: 12px; }
.members-list { display: flex; flex-direction: column; gap: 4px; }
.member-row { display: flex; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 6px; }
.member-role { font-size: 0.8em; opacity: 0.6; margin-left: auto; }
.btn-remove { background: none; border: 1px solid var(--error-color); color: var(--error-color); padding: 2px 8px; border-radius: 4px; cursor: pointer; font-size: 0.8em; }
.member-search { margin-bottom: 8px; }
.member-search input { width: 100%; padding: 6px 10px; border: 1px solid var(--border-color); border-radius: 6px; background: var(--input-bg); color: var(--text-color); }
```

**Step 4: Commit**

```bash
git add crates/visio-desktop/frontend/src/App.tsx crates/visio-desktop/frontend/src/App.css
git commit -m "feat(desktop): add members section in Room Info for restricted rooms"
```

---

## Task 9: Android — Restricted option + autocomplete in CreateRoomDialog

**Files:**
- Modify: `android/app/src/main/kotlin/io/visio/mobile/ui/HomeScreen.kt`

**Step 1: Add "Restricted" radio button**

In `CreateRoomDialog` composable, after the "Trusted" radio option (around line 554), add:

```kotlin
Row(verticalAlignment = Alignment.CenterVertically) {
    RadioButton(
        selected = accessLevel == "restricted",
        onClick = { accessLevel = "restricted" },
    )
    Column(modifier = Modifier.padding(start = 4.dp)) {
        Text(Strings.t("home.createRoom.restricted", lang), style = MaterialTheme.typography.bodyMedium)
        Text(Strings.t("home.createRoom.restrictedDesc", lang), style = MaterialTheme.typography.bodySmall)
    }
}
```

**Step 2: Add user search state and UI**

Add state variables to `CreateRoomDialog`:

```kotlin
var searchQuery by remember { mutableStateOf("") }
var searchResults by remember { mutableStateOf<List<Any>>(emptyList()) }
var invitedUsers by remember { mutableStateOf<List<Map<String, String>>>(emptyList()) }
```

Use `LaunchedEffect` with debouncing for search:

```kotlin
LaunchedEffect(searchQuery) {
    if (searchQuery.length < 3) {
        searchResults = emptyList()
        return@LaunchedEffect
    }
    delay(300)
    try {
        val results = VisioManager.client.searchUsers(searchQuery)
        searchResults = results.filter { user ->
            invitedUsers.none { it["id"] == user.id }
        }
    } catch (_: Exception) {
        searchResults = emptyList()
    }
}
```

Show invite UI when `accessLevel == "restricted"`:

```kotlin
if (accessLevel == "restricted") {
    Text(
        text = Strings.t("restricted.invite", lang),
        style = MaterialTheme.typography.labelMedium,
    )
    OutlinedTextField(
        value = searchQuery,
        onValueChange = { searchQuery = it },
        placeholder = { Text(Strings.t("restricted.searchUsers", lang)) },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
    // Search results dropdown
    searchResults.forEach { user ->
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable {
                    invitedUsers = invitedUsers + mapOf(
                        "id" to user.id,
                        "email" to user.email,
                        "name" to (user.fullName ?: user.email)
                    )
                    searchQuery = ""
                    searchResults = emptyList()
                }
                .padding(8.dp),
        ) {
            Text(user.fullName ?: user.email, style = MaterialTheme.typography.bodyMedium)
            Spacer(modifier = Modifier.width(8.dp))
            Text(user.email, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    // Invited user chips
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        invitedUsers.forEach { user ->
            AssistChip(
                onClick = { invitedUsers = invitedUsers.filter { it["id"] != user["id"] } },
                label = { Text(user["name"] ?: user["email"] ?: "") },
                trailingIcon = { Icon(Icons.Default.Close, contentDescription = null, modifier = Modifier.size(16.dp)) },
            )
        }
    }
}
```

**Step 3: Add access invitations after room creation**

In the create button's coroutine (after `createRoom` call succeeds), add:

```kotlin
if (accessLevel == "restricted") {
    for (user in invitedUsers) {
        try {
            VisioManager.client.addAccess(user["id"]!!, result.id)
        } catch (_: Exception) { }
    }
}
```

**Step 4: Verify build**

Run: `cd android && ./gradlew assembleDebug`
Expected: Success

**Step 5: Commit**

```bash
git add android/app/src/main/kotlin/io/visio/mobile/ui/HomeScreen.kt
git commit -m "feat(android): add restricted access level with user invite in create dialog"
```

---

## Task 10: Android — Members section in InCallSettingsSheet

**Files:**
- Modify: `android/app/src/main/kotlin/io/visio/mobile/ui/InCallSettingsSheet.kt`
- Modify: `android/app/src/main/kotlin/io/visio/mobile/VisioManager.kt`

**Step 1: Add access management methods to VisioManager**

Add to `VisioManager`:

```kotlin
private val _roomAccesses = MutableStateFlow<List<RoomAccess>>(emptyList())
val roomAccesses: StateFlow<List<RoomAccess>> = _roomAccesses

private var currentRoomId: String? = null

fun setCurrentRoomId(id: String?) {
    currentRoomId = id
}

fun refreshAccesses() {
    val roomId = currentRoomId ?: return
    viewModelScope.launch(Dispatchers.IO) {
        try {
            val accesses = client.listAccesses(roomId)
            _roomAccesses.value = accesses
        } catch (_: Exception) { }
    }
}

fun addAccess(userId: String, onDone: () -> Unit = {}) {
    val roomId = currentRoomId ?: return
    viewModelScope.launch(Dispatchers.IO) {
        try {
            client.addAccess(userId, roomId)
            refreshAccesses()
        } catch (_: Exception) { }
        withContext(Dispatchers.Main) { onDone() }
    }
}

fun removeAccess(accessId: String) {
    viewModelScope.launch(Dispatchers.IO) {
        try {
            client.removeAccess(accessId)
            refreshAccesses()
        } catch (_: Exception) { }
    }
}
```

**Step 2: Add Members tab to InCallSettingsSheet**

In `InCallSettingsSheet.kt`, add a new tab for members (only shown for restricted rooms). Add a composable function:

```kotlin
@Composable
private fun MembersTab(lang: String) {
    val accesses by VisioManager.roomAccesses.collectAsState()
    var searchQuery by remember { mutableStateOf("") }
    var searchResults by remember { mutableStateOf<List<UserSearchResult>>(emptyList()) }

    LaunchedEffect(Unit) { VisioManager.refreshAccesses() }

    LaunchedEffect(searchQuery) {
        if (searchQuery.length < 3) { searchResults = emptyList(); return@LaunchedEffect }
        delay(300)
        try {
            searchResults = VisioManager.client.searchUsers(searchQuery)
        } catch (_: Exception) { searchResults = emptyList() }
    }

    Column(modifier = Modifier.padding(16.dp).verticalScroll(rememberScrollState())) {
        Text(Strings.t("restricted.members", lang), style = MaterialTheme.typography.titleMedium)
        Spacer(modifier = Modifier.height(8.dp))

        // Search field
        OutlinedTextField(
            value = searchQuery,
            onValueChange = { searchQuery = it },
            placeholder = { Text(Strings.t("restricted.searchUsers", lang)) },
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
        )

        // Search results
        searchResults.forEach { user ->
            Row(
                modifier = Modifier.fillMaxWidth().clickable {
                    VisioManager.addAccess(user.id) { searchQuery = "" }
                }.padding(vertical = 8.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(user.fullName ?: user.email)
                Text(user.email, style = MaterialTheme.typography.bodySmall)
            }
        }

        Spacer(modifier = Modifier.height(12.dp))

        // Current members
        accesses.forEach { access ->
            Row(
                modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text(access.user.fullName ?: access.user.email)
                    Text(Strings.t("restricted.${access.role}", lang), style = MaterialTheme.typography.bodySmall)
                }
                if (access.role == "member") {
                    TextButton(onClick = { VisioManager.removeAccess(access.id) }) {
                        Text(Strings.t("restricted.remove", lang), color = MaterialTheme.colorScheme.error)
                    }
                }
            }
        }
    }
}
```

**Step 3: Add the tab icon to the sidebar**

In the tab icons section, add a "Members" tab conditionally:

```kotlin
// Show members tab only for restricted rooms
TabIcon(
    icon = Icons.Outlined.People,
    label = Strings.t("restricted.members", lang),
    selected = selectedTab == 4,
    onClick = { selectedTab = 4 },
)
```

And handle `selectedTab == 4` in the content area:

```kotlin
4 -> MembersTab(lang = lang)
```

**Step 4: Commit**

```bash
git add android/app/src/main/kotlin/io/visio/mobile/ui/InCallSettingsSheet.kt android/app/src/main/kotlin/io/visio/mobile/VisioManager.kt
git commit -m "feat(android): add members section in Room Info for restricted rooms"
```

---

## Task 11: iOS — Restricted option + autocomplete in CreateRoomSheet

**Files:**
- Modify: `ios/VisioMobile/Views/HomeView.swift`

**Step 1: Add "Restricted" picker option**

In `CreateRoomSheet`, update the `Picker` to add the restricted option:

```swift
Picker(Strings.t("home.createRoom.access", lang: lang), selection: $accessLevel) {
    Text(Strings.t("home.createRoom.public", lang: lang)).tag("public")
    Text(Strings.t("home.createRoom.trusted", lang: lang)).tag("trusted")
    Text(Strings.t("home.createRoom.restricted", lang: lang)).tag("restricted")
}
```

Update the description text:

```swift
if accessLevel == "public" {
    Text(Strings.t("home.createRoom.publicDesc", lang: lang))
        .font(.caption)
        .foregroundStyle(.secondary)
} else if accessLevel == "trusted" {
    Text(Strings.t("home.createRoom.trustedDesc", lang: lang))
        .font(.caption)
        .foregroundStyle(.secondary)
} else {
    Text(Strings.t("home.createRoom.restrictedDesc", lang: lang))
        .font(.caption)
        .foregroundStyle(.secondary)
}
```

**Step 2: Add user search state and autocomplete UI**

Add state properties to `CreateRoomSheet`:

```swift
@State private var searchQuery: String = ""
@State private var searchResults: [UserSearchResult] = []
@State private var invitedUsers: [UserSearchResult] = []
@State private var searchTask: Task<Void, Never>? = nil
```

Add search logic with debounce:

```swift
.onChange(of: searchQuery) { _, newValue in
    searchTask?.cancel()
    guard newValue.count >= 3 else {
        searchResults = []
        return
    }
    searchTask = Task {
        try? await Task.sleep(nanoseconds: 300_000_000)
        guard !Task.isCancelled else { return }
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let results = try manager.client.searchUsers(query: newValue)
                DispatchQueue.main.async {
                    searchResults = results.filter { user in
                        !invitedUsers.contains { $0.id == user.id }
                    }
                }
            } catch {
                DispatchQueue.main.async { searchResults = [] }
            }
        }
    }
}
```

Add UI section when restricted is selected (inside the Form, after access level section):

```swift
if accessLevel == "restricted" && createdUrl == nil {
    Section(header: Text(Strings.t("restricted.invite", lang: lang))) {
        TextField(Strings.t("restricted.searchUsers", lang: lang), text: $searchQuery)

        ForEach(searchResults, id: \.id) { user in
            Button {
                invitedUsers.append(user)
                searchQuery = ""
                searchResults = []
            } label: {
                VStack(alignment: .leading) {
                    Text(user.fullName ?? user.email)
                    Text(user.email)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    if !invitedUsers.isEmpty {
        Section(header: Text(Strings.t("restricted.members", lang: lang))) {
            ForEach(invitedUsers, id: \.id) { user in
                HStack {
                    Text(user.fullName ?? user.email)
                    Spacer()
                    Button {
                        invitedUsers.removeAll { $0.id == user.id }
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }
    }
}
```

**Step 3: Add access invitations after room creation**

In the create button's action, after room creation succeeds:

```swift
if accessLevel == "restricted" {
    for user in invitedUsers {
        do {
            _ = try manager.client.addAccess(userId: user.id, roomId: result.id)
        } catch { }
    }
}
```

**Step 4: Verify build**

Run: `scripts/build-ios.sh device` or open in Xcode

**Step 5: Commit**

```bash
git add ios/VisioMobile/Views/HomeView.swift
git commit -m "feat(ios): add restricted access level with user invite in create sheet"
```

---

## Task 12: iOS — Members section in InCallSettingsSheet

**Files:**
- Modify: `ios/VisioMobile/Views/InCallSettingsSheet.swift`
- Modify: `ios/VisioMobile/VisioManager.swift`

**Step 1: Add access management to VisioManager**

Add to `VisioManager.swift`:

```swift
@Published var roomAccesses: [RoomAccess] = []
var currentRoomId: String?

func refreshAccesses() {
    guard let roomId = currentRoomId else { return }
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
        do {
            let accesses = try self?.client.listAccesses(roomId: roomId) ?? []
            DispatchQueue.main.async {
                self?.roomAccesses = accesses
            }
        } catch { }
    }
}

func addAccessMember(userId: String) {
    guard let roomId = currentRoomId else { return }
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
        do {
            _ = try self?.client.addAccess(userId: userId, roomId: roomId)
            self?.refreshAccesses()
        } catch { }
    }
}

func removeAccessMember(accessId: String) {
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
        do {
            try self?.client.removeAccess(accessId: accessId)
            self?.refreshAccesses()
        } catch { }
    }
}
```

**Step 2: Add Members tab to InCallSettingsSheet**

Add a new tab button in the sidebar:

```swift
tabButton(icon: "person.2.fill", tab: 4, label: Strings.t("restricted.members", lang: lang))
```

Add case in the content switch:

```swift
case 4: membersTab
```

Add the membersTab view:

```swift
private var membersTab: some View {
    ScrollView {
        VStack(alignment: .leading, spacing: 12) {
            // Search field
            TextField(Strings.t("restricted.searchUsers", lang: lang), text: $memberSearchQuery)
                .textFieldStyle(.roundedBorder)
                .padding(.horizontal)

            // Search results
            ForEach(memberSearchResults, id: \.id) { user in
                Button {
                    manager.addAccessMember(userId: user.id)
                    memberSearchQuery = ""
                    memberSearchResults = []
                } label: {
                    VStack(alignment: .leading) {
                        Text(user.fullName ?? user.email)
                        Text(user.email)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .padding(.horizontal)
                }
            }

            Divider()

            // Current members
            Text(Strings.t("restricted.members", lang: lang))
                .font(.headline)
                .padding(.horizontal)

            ForEach(manager.roomAccesses, id: \.id) { access in
                HStack {
                    VStack(alignment: .leading) {
                        Text(access.user.fullName ?? access.user.email)
                        Text(Strings.t("restricted.\(access.role)", lang: lang))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    if access.role == "member" {
                        Button {
                            manager.removeAccessMember(accessId: access.id)
                        } label: {
                            Text(Strings.t("restricted.remove", lang: lang))
                                .font(.caption)
                                .foregroundStyle(.red)
                        }
                    }
                }
                .padding(.horizontal)
            }
        }
        .padding(.vertical)
    }
    .onAppear { manager.refreshAccesses() }
}
```

Add state vars:

```swift
@State private var memberSearchQuery: String = ""
@State private var memberSearchResults: [UserSearchResult] = []
```

Add onChange for search with debounce (same pattern as Task 11).

**Step 3: Commit**

```bash
git add ios/VisioMobile/Views/InCallSettingsSheet.swift ios/VisioMobile/VisioManager.swift
git commit -m "feat(ios): add members section in Room Info for restricted rooms"
```

---

## Task 13: Generate UniFFI bindings

**Files:**
- Generated: `android/app/src/main/kotlin/generated/`
- Generated: `ios/VisioMobile/Generated/`

**Step 1: Generate bindings**

Run: `scripts/generate-bindings.sh all`

**Step 2: Verify both platforms build**

Run: `cd android && ./gradlew assembleDebug`
Run: Open Xcode project and build

**Step 3: Commit**

```bash
git add android/app/src/main/kotlin/generated/ ios/VisioMobile/Generated/
git commit -m "build: regenerate UniFFI bindings with access types"
```

---

## Task 14: Full build verification and all-platform test

**Step 1: Run Rust tests**

Run: `cargo test -p visio-core -- access`
Expected: All access tests pass

Run: `cargo test -p visio-core`
Expected: All 45+ tests pass

**Step 2: Build all platforms**

Run: `cargo build -p visio-core -p visio-ffi -p visio-desktop`
Expected: All succeed

**Step 3: Commit any final fixes**

If any compilation fixes were needed, commit them:

```bash
git commit -m "fix: compilation fixes for restricted rooms feature"
```
