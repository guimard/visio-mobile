# Room Creation — Phase 2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add room creation (name + access level) for authenticated users, auto-join created rooms, and display shareable room links (HTTPS + visio://) in the in-call settings panel on all 3 platforms.

**Architecture:** New `create_room` function in visio-core `session.rs` handles CSRF token generation and POST to the Meet API. The response includes LiveKit credentials, so the app can join immediately. The in-call settings panels (Android, iOS, Desktop) gain a new "Room info" tab showing copyable HTTPS and visio:// links with native share.

**Tech Stack:** Rust (reqwest, serde, hex, rand), UniFFI, Kotlin (Jetpack Compose), Swift (SwiftUI), React/TypeScript (Tauri)

**Design doc:** `docs/plans/2026-03-07-oidc-authentication-design.md` (Phase 2 section)

---

### Task 1: Add `create_room` to visio-core session.rs

**Files:**
- Modify: `crates/visio-core/src/session.rs:1-146`
- Modify: `crates/visio-core/Cargo.toml` (add `rand` dependency)

**Step 1: Write the failing test**

Add to `crates/visio-core/src/session.rs` tests module:

```rust
    #[test]
    fn test_create_room_response_deserialization() {
        let json = r#"{
            "id": "cc9950db-cf78-4bf0-84b2-4d906148c849",
            "name": "Test Room",
            "slug": "test-room",
            "access_level": "public",
            "livekit": {
                "url": "https://livekit.example.com",
                "room": "cc9950db-cf78-4bf0-84b2-4d906148c849",
                "token": "eyJhbGciOiJIUzI1NiJ9.test"
            }
        }"#;
        let resp: CreateRoomResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.slug, "test-room");
        assert_eq!(resp.name, "Test Room");
        assert_eq!(resp.access_level, "public");
        assert_eq!(resp.livekit.url, "https://livekit.example.com");
        assert!(!resp.livekit.token.is_empty());
    }

    #[tokio::test]
    async fn test_create_room_without_auth_returns_error() {
        let result = SessionManager::create_room(
            "https://meet.example.com",
            "invalid_cookie",
            "Test Room",
            "public",
        ).await;
        assert!(result.is_err());
    }
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p visio-core --lib session::tests::test_create_room`
Expected: FAIL — `CreateRoomResponse` and `create_room` not found

**Step 3: Write implementation**

Add `rand` to `crates/visio-core/Cargo.toml` dependencies:

```toml
rand = "0.8"
```

Add to `crates/visio-core/src/session.rs` after the `UserInfo` impl block (after line 26):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoomLiveKit {
    pub url: String,
    pub room: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoomResponse {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub access_level: String,
    pub livekit: CreateRoomLiveKit,
}
```

Add to `SessionManager` impl block (after `logout`, before closing `}`):

```rust
    /// Create a room on the Meet API.
    /// Handles Django CSRF protection: generates a random 64-hex-char token,
    /// sends it as both a `csrftoken` cookie and `X-CSRFToken` header.
    pub async fn create_room(
        meet_url: &str,
        cookie: &str,
        name: &str,
        access_level: &str,
    ) -> Result<CreateRoomResponse, VisioError> {
        use rand::Rng;

        let url = format!("{}/api/v1.0/rooms/", meet_url.trim_end_matches('/'));

        // Generate a random 64-char hex CSRF token (Django expects this length)
        let csrf_bytes: [u8; 32] = rand::thread_rng().gen();
        let csrf_token: String = csrf_bytes.iter().map(|b| format!("{:02x}", b)).collect();

        let cookie_header = format!("sessionid={}; csrftoken={}", cookie, csrf_token);

        let body = serde_json::json!({
            "name": name,
            "access_level": access_level,
        });

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header(COOKIE, &cookie_header)
            .header("X-CSRFToken", &csrf_token)
            .header("Referer", format!("{}/", meet_url.trim_end_matches('/')))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| VisioError::Http(e.to_string()))?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(VisioError::Session(
                "Authentication required to create a room".to_string(),
            ));
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(VisioError::Session(format!(
                "Room creation failed ({}): {}",
                status, body
            )));
        }

        let room: CreateRoomResponse = response
            .json()
            .await
            .map_err(|e| VisioError::Session(format!("Invalid room response: {}", e)))?;

        Ok(room)
    }
```

Add to `crates/visio-core/src/lib.rs` exports (line 30):

```rust
pub use session::{CreateRoomResponse, CreateRoomLiveKit, SessionManager, SessionState, UserInfo};
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p visio-core --lib session`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add crates/visio-core/src/session.rs crates/visio-core/src/lib.rs crates/visio-core/Cargo.toml
git commit -m "feat(core): add create_room with CSRF handling for Meet API"
```

---

### Task 2: Expose room creation via UniFFI

**Files:**
- Modify: `crates/visio-ffi/src/lib.rs` (add FFI types + method)
- Modify: `crates/visio-ffi/src/visio.udl:108-198` (add types + method)

**Step 1: Add FFI types to `crates/visio-ffi/src/lib.rs`**

Add after the existing SessionState enum (search for `pub enum SessionState`):

```rust
#[derive(uniffi::Record)]
pub struct CreateRoomResult {
    pub slug: String,
    pub name: String,
    pub access_level: String,
    pub livekit_url: String,
    pub livekit_token: String,
}
```

**Step 2: Add `create_room` method to VisioClient impl**

Add in the VisioClient impl block, after the `logout` method:

```rust
    pub fn create_room(
        &self,
        meet_url: String,
        name: String,
        access_level: String,
    ) -> Result<CreateRoomResult, VisioError> {
        let cookie = {
            let session = self.session_manager.lock().unwrap();
            session.cookie().ok_or_else(|| {
                VisioError::Session("Not authenticated".to_string())
            })?
        };

        let result = self
            .rt
            .block_on(visio_core::SessionManager::create_room(
                &meet_url,
                &cookie,
                &name,
                &access_level,
            ))
            .map_err(VisioError::from)?;

        let livekit_url = result
            .livekit
            .url
            .replace("https://", "wss://")
            .replace("http://", "ws://");

        Ok(CreateRoomResult {
            slug: result.slug,
            name: result.name,
            access_level: result.access_level,
            livekit_url,
            livekit_token: result.livekit.token,
        })
    }
```

**Step 3: Update UDL**

Add to `crates/visio-ffi/src/visio.udl` after `SessionState` (after line 112):

```
dictionary CreateRoomResult {
    string slug;
    string name;
    string access_level;
    string livekit_url;
    string livekit_token;
};
```

Add to the `VisioClient` interface block (after `validate_session`, before `start_video_renderer`):

```
    [Throws=VisioError]
    CreateRoomResult create_room(string meet_url, string name, string access_level);
```

**Step 4: Build to verify**

Run: `cargo build -p visio-ffi`
Expected: Build succeeds

**Step 5: Commit**

```bash
git add crates/visio-ffi/src/lib.rs crates/visio-ffi/src/visio.udl
git commit -m "feat(ffi): expose create_room via UniFFI"
```

---

### Task 3: Add i18n keys for room creation and share links (all 6 languages)

**Files:**
- Modify: `i18n/en.json`, `i18n/fr.json`, `i18n/de.json`, `i18n/es.json`, `i18n/it.json`, `i18n/nl.json`

**Step 1: Add keys to all 6 JSON files**

Add the following keys to each file (before the closing `}`):

English (`en.json`):
```json
"home.createRoom": "Create a room",
"home.createRoom.name": "Room name",
"home.createRoom.namePlaceholder": "My meeting",
"home.createRoom.access": "Access level",
"home.createRoom.public": "Public",
"home.createRoom.publicDesc": "Anyone can join",
"home.createRoom.trusted": "Trusted",
"home.createRoom.trustedDesc": "Only authenticated users can join",
"home.createRoom.create": "Create",
"home.createRoom.creating": "Creating...",
"home.createRoom.error": "Room creation failed",
"settings.incall.roomInfo": "Room info",
"settings.incall.roomLink": "Room link",
"settings.incall.deepLink": "App link",
"settings.incall.share": "Share link",
"settings.incall.copied": "Copied!"
```

French (`fr.json`):
```json
"home.createRoom": "Créer une salle",
"home.createRoom.name": "Nom de la salle",
"home.createRoom.namePlaceholder": "Ma réunion",
"home.createRoom.access": "Niveau d'accès",
"home.createRoom.public": "Public",
"home.createRoom.publicDesc": "Tout le monde peut rejoindre",
"home.createRoom.trusted": "Trusted",
"home.createRoom.trustedDesc": "Seuls les utilisateurs connectés peuvent rejoindre",
"home.createRoom.create": "Créer",
"home.createRoom.creating": "Création...",
"home.createRoom.error": "Échec de la création",
"settings.incall.roomInfo": "Infos salle",
"settings.incall.roomLink": "Lien de la salle",
"settings.incall.deepLink": "Lien application",
"settings.incall.share": "Partager le lien",
"settings.incall.copied": "Copié !"
```

German (`de.json`):
```json
"home.createRoom": "Raum erstellen",
"home.createRoom.name": "Raumname",
"home.createRoom.namePlaceholder": "Mein Meeting",
"home.createRoom.access": "Zugangsstufe",
"home.createRoom.public": "Öffentlich",
"home.createRoom.publicDesc": "Jeder kann beitreten",
"home.createRoom.trusted": "Vertrauenswürdig",
"home.createRoom.trustedDesc": "Nur authentifizierte Benutzer können beitreten",
"home.createRoom.create": "Erstellen",
"home.createRoom.creating": "Wird erstellt...",
"home.createRoom.error": "Raumerstellung fehlgeschlagen",
"settings.incall.roomInfo": "Rauminfo",
"settings.incall.roomLink": "Raumlink",
"settings.incall.deepLink": "App-Link",
"settings.incall.share": "Link teilen",
"settings.incall.copied": "Kopiert!"
```

Spanish (`es.json`):
```json
"home.createRoom": "Crear una sala",
"home.createRoom.name": "Nombre de la sala",
"home.createRoom.namePlaceholder": "Mi reunión",
"home.createRoom.access": "Nivel de acceso",
"home.createRoom.public": "Público",
"home.createRoom.publicDesc": "Cualquiera puede unirse",
"home.createRoom.trusted": "De confianza",
"home.createRoom.trustedDesc": "Solo usuarios autenticados pueden unirse",
"home.createRoom.create": "Crear",
"home.createRoom.creating": "Creando...",
"home.createRoom.error": "Error al crear la sala",
"settings.incall.roomInfo": "Info de sala",
"settings.incall.roomLink": "Enlace de la sala",
"settings.incall.deepLink": "Enlace de app",
"settings.incall.share": "Compartir enlace",
"settings.incall.copied": "¡Copiado!"
```

Italian (`it.json`):
```json
"home.createRoom": "Crea una stanza",
"home.createRoom.name": "Nome della stanza",
"home.createRoom.namePlaceholder": "La mia riunione",
"home.createRoom.access": "Livello di accesso",
"home.createRoom.public": "Pubblico",
"home.createRoom.publicDesc": "Chiunque può partecipare",
"home.createRoom.trusted": "Affidabile",
"home.createRoom.trustedDesc": "Solo utenti autenticati possono partecipare",
"home.createRoom.create": "Crea",
"home.createRoom.creating": "Creazione...",
"home.createRoom.error": "Creazione stanza fallita",
"settings.incall.roomInfo": "Info stanza",
"settings.incall.roomLink": "Link della stanza",
"settings.incall.deepLink": "Link app",
"settings.incall.share": "Condividi link",
"settings.incall.copied": "Copiato!"
```

Dutch (`nl.json`):
```json
"home.createRoom": "Kamer aanmaken",
"home.createRoom.name": "Kamernaam",
"home.createRoom.namePlaceholder": "Mijn vergadering",
"home.createRoom.access": "Toegangsniveau",
"home.createRoom.public": "Openbaar",
"home.createRoom.publicDesc": "Iedereen kan deelnemen",
"home.createRoom.trusted": "Vertrouwd",
"home.createRoom.trustedDesc": "Alleen geauthenticeerde gebruikers kunnen deelnemen",
"home.createRoom.create": "Aanmaken",
"home.createRoom.creating": "Aanmaken...",
"home.createRoom.error": "Kamer aanmaken mislukt",
"settings.incall.roomInfo": "Kamerinfo",
"settings.incall.roomLink": "Kamerlink",
"settings.incall.deepLink": "App-link",
"settings.incall.share": "Link delen",
"settings.incall.copied": "Gekopieerd!"
```

**Step 2: Verify JSON validity**

Run: `for f in i18n/*.json; do python3 -c "import json; json.load(open('$f'))" && echo "$f OK"; done`
Expected: All 6 files OK

**Step 3: Commit**

```bash
git add i18n/
git commit -m "feat(i18n): add room creation and share link strings in all 6 languages"
```

---

### Task 4: Desktop — Add create_room Tauri command

**Files:**
- Modify: `crates/visio-desktop/src/lib.rs` (add command + register)

**Step 1: Add Tauri command**

Add after the `logout_session` command in `crates/visio-desktop/src/lib.rs`:

```rust
#[tauri::command]
async fn create_room(
    state: tauri::State<'_, VisioState>,
    meet_url: String,
    name: String,
    access_level: String,
) -> Result<serde_json::Value, String> {
    use rand::Rng;

    let session = state.session.lock().await;
    let cookie = session
        .cookie()
        .ok_or("Not authenticated")?;
    drop(session);

    let result = visio_core::SessionManager::create_room(
        &meet_url,
        &cookie,
        &name,
        &access_level,
    )
    .await
    .map_err(|e| e.to_string())?;

    let livekit_url = result
        .livekit
        .url
        .replace("https://", "wss://")
        .replace("http://", "ws://");

    Ok(serde_json::json!({
        "slug": result.slug,
        "name": result.name,
        "access_level": result.access_level,
        "livekit_url": livekit_url,
        "livekit_token": result.livekit.token,
    }))
}
```

**Step 2: Register the command**

Find the `.invoke_handler(tauri::generate_handler![...])` call and add `create_room` to the list.

**Step 3: Add `rand` to desktop Cargo.toml if not already present**

Check `crates/visio-desktop/Cargo.toml` for `rand`. If missing, add:
```toml
rand = "0.8"
```

**Step 4: Build to verify**

Run: `cargo build -p visio-desktop`
Expected: Build succeeds

**Step 5: Commit**

```bash
git add crates/visio-desktop/src/lib.rs crates/visio-desktop/Cargo.toml
git commit -m "feat(desktop): add create_room Tauri command"
```

---

### Task 5: Desktop — Add Create Room dialog in frontend

**Files:**
- Modify: `crates/visio-desktop/frontend/src/App.tsx` (add dialog component + button)
- Modify: `crates/visio-desktop/frontend/src/App.css` (add dialog styles)

**Step 1: Add CreateRoomDialog component**

Add a new component in `App.tsx` (after `HomeView` function, before `CallView`):

```tsx
function CreateRoomDialog({
  meetInstances,
  onCreated,
  onCancel,
}: {
  meetInstances: string[];
  onCreated: (meetUrl: string) => void;
  onCancel: () => void;
}) {
  const t = useT();
  const [name, setName] = useState("");
  const [accessLevel, setAccessLevel] = useState("public");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState("");
  const meetInstance = meetInstances[0] || "";

  const handleCreate = async () => {
    if (!name.trim() || !meetInstance) return;
    setCreating(true);
    setError("");
    try {
      const result = await invoke<{
        slug: string;
        name: string;
        livekit_url: string;
        livekit_token: string;
      }>("create_room", {
        meetUrl: `https://${meetInstance}`,
        name: name.trim(),
        accessLevel,
      });
      onCreated(`https://${meetInstance}/${result.slug}`);
    } catch (e) {
      setError(String(e));
      setCreating(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content create-room-dialog" onClick={(e) => e.stopPropagation()}>
        <h2>{t("home.createRoom")}</h2>

        <div className="form-field">
          <label>{t("home.createRoom.name")}</label>
          <input
            type="text"
            placeholder={t("home.createRoom.namePlaceholder")}
            value={name}
            onChange={(e) => setName(e.target.value)}
            autoFocus
            maxLength={500}
          />
        </div>

        <div className="form-field">
          <label>{t("home.createRoom.access")}</label>
          <div className="access-level-options">
            <label className={`access-option ${accessLevel === "public" ? "selected" : ""}`}>
              <input
                type="radio"
                name="access"
                value="public"
                checked={accessLevel === "public"}
                onChange={() => setAccessLevel("public")}
              />
              <div>
                <strong>{t("home.createRoom.public")}</strong>
                <span>{t("home.createRoom.publicDesc")}</span>
              </div>
            </label>
            <label className={`access-option ${accessLevel === "trusted" ? "selected" : ""}`}>
              <input
                type="radio"
                name="access"
                value="trusted"
                checked={accessLevel === "trusted"}
                onChange={() => setAccessLevel("trusted")}
              />
              <div>
                <strong>{t("home.createRoom.trusted")}</strong>
                <span>{t("home.createRoom.trustedDesc")}</span>
              </div>
            </label>
          </div>
        </div>

        {error && <div className="create-room-error">{t("home.createRoom.error")}: {error}</div>}

        <div className="modal-buttons">
          <button className="btn-secondary" onClick={onCancel}>{t("settings.cancel")}</button>
          <button
            className="btn-primary"
            onClick={handleCreate}
            disabled={!name.trim() || creating}
          >
            {creating ? t("home.createRoom.creating") : t("home.createRoom.create")}
          </button>
        </div>
      </div>
    </div>
  );
}
```

**Step 2: Add "Create room" button in HomeView**

In `HomeView`, add state and button. Find the Join button area and add below it (visible only when authenticated):

```tsx
const [showCreateRoom, setShowCreateRoom] = useState(false);

// In JSX, after the Join button, inside the authenticated check:
{isAuthenticated && (
  <button
    className="btn-secondary create-room-btn"
    onClick={() => setShowCreateRoom(true)}
  >
    {t("home.createRoom")}
  </button>
)}

// At the end of HomeView, before closing div:
{showCreateRoom && (
  <CreateRoomDialog
    meetInstances={meetInstances}
    onCreated={(meetUrl) => {
      setShowCreateRoom(false);
      onJoin(meetUrl, displayName.trim() || null);
    }}
    onCancel={() => setShowCreateRoom(false)}
  />
)}
```

**Step 3: Add CSS styles**

Add to `App.css`:

```css
.create-room-dialog {
  max-width: 440px;
}
.create-room-dialog h2 {
  margin: 0 0 16px;
  font-size: 1.2rem;
}
.form-field {
  margin-bottom: 16px;
}
.form-field label {
  display: block;
  margin-bottom: 4px;
  font-weight: 500;
  font-size: 0.9rem;
}
.access-level-options {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.access-option {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 8px;
  cursor: pointer;
}
.access-option.selected {
  border-color: var(--primary);
  background: var(--primary-bg);
}
.access-option div {
  display: flex;
  flex-direction: column;
}
.access-option strong {
  font-size: 0.9rem;
}
.access-option span {
  font-size: 0.8rem;
  opacity: 0.7;
}
.access-option input[type="radio"] {
  margin-top: 3px;
}
.create-room-error {
  color: var(--error);
  font-size: 0.85rem;
  margin-bottom: 8px;
}
.create-room-btn {
  width: 100%;
}
```

**Step 4: Build and verify**

Run: `cd crates/visio-desktop && cargo tauri dev`
Expected: "Create a room" button visible when authenticated, dialog opens, creation works

**Step 5: Commit**

```bash
git add crates/visio-desktop/frontend/src/App.tsx crates/visio-desktop/frontend/src/App.css
git commit -m "feat(desktop): add room creation dialog with auto-join"
```

---

### Task 6: Desktop — Add room links to InfoSidebar

**Files:**
- Modify: `crates/visio-desktop/frontend/src/App.tsx:507-540` (InfoSidebar component)
- Modify: `crates/visio-desktop/frontend/src/App.css` (add link styles)

**Step 1: Update InfoSidebar to show HTTPS and visio:// links**

Replace the `InfoSidebar` component at line 507:

```tsx
function InfoSidebar({ meetUrl, onClose }: { meetUrl: string; onClose: () => void }) {
  const t = useT();
  const [copiedHttp, setCopiedHttp] = useState(false);
  const [copiedDeep, setCopiedDeep] = useState(false);

  // Parse URL to build deep link: visio://instance/slug
  const displayUrl = meetUrl.replace(/^https?:\/\//, "");
  const deepLink = `visio://${displayUrl}`;

  const handleCopy = async (text: string, type: "http" | "deep") => {
    try {
      await navigator.clipboard.writeText(text);
      if (type === "http") {
        setCopiedHttp(true);
        setTimeout(() => setCopiedHttp(false), 2000);
      } else {
        setCopiedDeep(true);
        setTimeout(() => setCopiedDeep(false), 2000);
      }
    } catch { /* ignore */ }
  };

  return (
    <div className="info-sidebar">
      <div className="participants-header">
        <span>{t("info.title")}</span>
        <button className="chat-close" onClick={onClose}><RiCloseLine size={20} /></button>
      </div>
      <div className="info-body">
        <div className="info-section">
          <div className="info-section-title">{t("settings.incall.roomLink")}</div>
          <div className="info-link-row">
            <RiGlobalLine size={16} />
            <span className="info-url">{displayUrl}</span>
            <button className="info-copy-btn" onClick={() => handleCopy(meetUrl, "http")}>
              {copiedHttp ? <RiCheckLine size={14} /> : <RiFileCopyLine size={14} />}
              {copiedHttp ? t("settings.incall.copied") : t("info.copy")}
            </button>
          </div>
        </div>

        <div className="info-section">
          <div className="info-section-title">{t("settings.incall.deepLink")}</div>
          <div className="info-link-row">
            <RiSmartphoneLine size={16} />
            <span className="info-url">{deepLink}</span>
            <button className="info-copy-btn" onClick={() => handleCopy(deepLink, "deep")}>
              {copiedDeep ? <RiCheckLine size={14} /> : <RiFileCopyLine size={14} />}
              {copiedDeep ? t("settings.incall.copied") : t("info.copy")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
```

**Step 2: Add missing icon imports**

At the top of `App.tsx`, ensure these Remix Icon imports are present:

```tsx
import { RiGlobalLine, RiSmartphoneLine } from "@remixicon/react";
```

**Step 3: Add CSS for link rows**

Add to `App.css`:

```css
.info-link-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 0;
}
.info-link-row .info-url {
  flex: 1;
  word-break: break-all;
  font-size: 0.85rem;
}
.info-link-row .info-copy-btn {
  flex-shrink: 0;
}
```

**Step 4: Build and verify**

Run: `cd crates/visio-desktop && cargo tauri dev`
Expected: Info sidebar shows both HTTPS and visio:// links with copy buttons

**Step 5: Commit**

```bash
git add crates/visio-desktop/frontend/src/App.tsx crates/visio-desktop/frontend/src/App.css
git commit -m "feat(desktop): add HTTPS and visio:// links in info sidebar"
```

---

### Task 7: Android — Add Create Room dialog on HomeScreen

**Files:**
- Modify: `android/app/src/main/kotlin/io/visio/mobile/ui/HomeScreen.kt`

**Step 1: Add CreateRoomDialog composable**

Add a new composable function at the bottom of `HomeScreen.kt` (before the file ends):

```kotlin
@Composable
private fun CreateRoomDialog(
    meetInstances: List<String>,
    lang: String,
    onCreated: (roomUrl: String) -> Unit,
    onDismiss: () -> Unit,
) {
    val meetInstance = meetInstances.firstOrNull() ?: return
    var name by remember { mutableStateOf("") }
    var accessLevel by remember { mutableStateOf("public") }
    var creating by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val coroutineScope = rememberCoroutineScope()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(Strings.t("home.createRoom", lang)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    label = { Text(Strings.t("home.createRoom.name", lang)) },
                    placeholder = { Text(Strings.t("home.createRoom.namePlaceholder", lang)) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )

                Text(
                    text = Strings.t("home.createRoom.access", lang),
                    style = MaterialTheme.typography.labelMedium,
                )

                Row(verticalAlignment = Alignment.CenterVertically) {
                    RadioButton(
                        selected = accessLevel == "public",
                        onClick = { accessLevel = "public" },
                    )
                    Column {
                        Text(Strings.t("home.createRoom.public", lang), style = MaterialTheme.typography.bodyMedium)
                        Text(Strings.t("home.createRoom.publicDesc", lang), style = MaterialTheme.typography.bodySmall)
                    }
                }

                Row(verticalAlignment = Alignment.CenterVertically) {
                    RadioButton(
                        selected = accessLevel == "trusted",
                        onClick = { accessLevel = "trusted" },
                    )
                    Column {
                        Text(Strings.t("home.createRoom.trusted", lang), style = MaterialTheme.typography.bodyMedium)
                        Text(Strings.t("home.createRoom.trustedDesc", lang), style = MaterialTheme.typography.bodySmall)
                    }
                }

                if (error != null) {
                    Text(
                        text = error!!,
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        },
        confirmButton = {
            Button(
                onClick = {
                    creating = true
                    error = null
                    coroutineScope.launch(Dispatchers.IO) {
                        try {
                            val result = VisioManager.client.createRoom(
                                "https://$meetInstance",
                                name.trim(),
                                accessLevel,
                            )
                            withContext(Dispatchers.Main) {
                                onCreated("https://$meetInstance/${result.slug}")
                            }
                        } catch (e: Exception) {
                            withContext(Dispatchers.Main) {
                                error = e.message ?: Strings.t("home.createRoom.error", lang)
                                creating = false
                            }
                        }
                    }
                },
                enabled = name.trim().isNotEmpty() && !creating,
            ) {
                Text(
                    if (creating) Strings.t("home.createRoom.creating", lang)
                    else Strings.t("home.createRoom.create", lang)
                )
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(Strings.t("settings.cancel", lang))
            }
        },
    )
}
```

**Step 2: Add state and button in HomeScreen**

In the `HomeScreen` composable, add state:

```kotlin
var showCreateRoom by remember { mutableStateOf(false) }
```

After the Join button, add (inside the authenticated check):

```kotlin
if (VisioManager.isAuthenticated) {
    Button(
        onClick = { showCreateRoom = true },
        modifier = Modifier.fillMaxWidth().padding(horizontal = 32.dp),
        colors = ButtonDefaults.outlinedButtonColors(),
        border = BorderStroke(1.dp, VisioColors.Primary500),
    ) {
        Text(Strings.t("home.createRoom", lang))
    }
}
```

Add the dialog at the end of the composable (before the closing `}`):

```kotlin
if (showCreateRoom) {
    CreateRoomDialog(
        meetInstances = meetInstances,
        lang = lang,
        onCreated = { roomUrl ->
            showCreateRoom = false
            onJoin(roomUrl, username)
        },
        onDismiss = { showCreateRoom = false },
    )
}
```

**Step 3: Build to verify**

Run: `cd android && ./gradlew assembleDebug`
Expected: Build succeeds

**Step 4: Commit**

```bash
git add android/app/src/main/kotlin/io/visio/mobile/ui/HomeScreen.kt
git commit -m "feat(android): add room creation dialog with auto-join"
```

---

### Task 8: Android — Add Room Info tab in InCallSettingsSheet

**Files:**
- Modify: `android/app/src/main/kotlin/io/visio/mobile/ui/InCallSettingsSheet.kt:55-154`
- Modify: `android/app/src/main/kotlin/io/visio/mobile/ui/CallScreen.kt:300-310` (pass roomUrl)

**Step 1: Add `roomUrl` parameter to InCallSettingsSheet**

Update the function signature at line 55:

```kotlin
fun InCallSettingsSheet(
    roomUrl: String,              // NEW
    initialTab: Int = 0,
    onDismiss: () -> Unit,
    onSelectAudioInput: (AudioDeviceInfo) -> Unit,
    onSelectAudioOutput: (AudioDeviceInfo) -> Unit,
    onSwitchCamera: (Boolean) -> Unit,
    isFrontCamera: Boolean,
)
```

**Step 2: Add 4th tab icon (link/info)**

After the Notifications tab icon (line 118), add:

```kotlin
                TabIcon(
                    icon = Icons.Outlined.Info,
                    label = Strings.t("settings.incall.roomInfo", lang),
                    selected = selectedTab == 3,
                    onClick = { selectedTab = 3 },
                )
```

Add the import: `import androidx.compose.material.icons.outlined.Info`

**Step 3: Add tab content**

In the `when (selectedTab)` block (line 128), add case 3:

```kotlin
                    3 -> RoomInfoTab(roomUrl, lang)
```

**Step 4: Add RoomInfoTab composable**

Add at the bottom of the file:

```kotlin
@Composable
private fun RoomInfoTab(roomUrl: String, lang: String) {
    val context = LocalContext.current
    val displayUrl = roomUrl.removePrefix("https://").removePrefix("http://")
    val deepLink = "visio://$displayUrl"
    var copiedHttp by remember { mutableStateOf(false) }
    var copiedDeep by remember { mutableStateOf(false) }

    Column(
        modifier = Modifier.padding(8.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        // HTTPS link
        SectionHeader(Strings.t("settings.incall.roomLink", lang))
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .background(VisioColors.PrimaryDark50, RoundedCornerShape(8.dp))
                .padding(12.dp),
        ) {
            Icon(
                painter = painterResource(R.drawable.ri_global_line),
                contentDescription = null,
                tint = VisioColors.White,
                modifier = Modifier.size(20.dp),
            )
            Text(
                text = displayUrl,
                color = VisioColors.White,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
            )
            IconButton(onClick = {
                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                clipboard.setPrimaryClip(android.content.ClipData.newPlainText("Room URL", roomUrl))
                copiedHttp = true
            }) {
                Icon(
                    painter = painterResource(
                        if (copiedHttp) R.drawable.ri_check_line else R.drawable.ri_file_copy_line
                    ),
                    contentDescription = Strings.t("info.copy", lang),
                    tint = VisioColors.White,
                    modifier = Modifier.size(18.dp),
                )
            }
        }

        // Deep link
        SectionHeader(Strings.t("settings.incall.deepLink", lang))
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .background(VisioColors.PrimaryDark50, RoundedCornerShape(8.dp))
                .padding(12.dp),
        ) {
            Icon(
                painter = painterResource(R.drawable.ri_smartphone_line),
                contentDescription = null,
                tint = VisioColors.White,
                modifier = Modifier.size(20.dp),
            )
            Text(
                text = deepLink,
                color = VisioColors.White,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
            )
            IconButton(onClick = {
                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                clipboard.setPrimaryClip(android.content.ClipData.newPlainText("Deep Link", deepLink))
                copiedDeep = true
            }) {
                Icon(
                    painter = painterResource(
                        if (copiedDeep) R.drawable.ri_check_line else R.drawable.ri_file_copy_line
                    ),
                    contentDescription = Strings.t("info.copy", lang),
                    tint = VisioColors.White,
                    modifier = Modifier.size(18.dp),
                )
            }
        }

        // Share button
        Button(
            onClick = {
                val shareIntent = android.content.Intent(android.content.Intent.ACTION_SEND).apply {
                    type = "text/plain"
                    putExtra(android.content.Intent.EXTRA_TEXT, roomUrl)
                }
                context.startActivity(android.content.Intent.createChooser(shareIntent, null))
            },
            modifier = Modifier.fillMaxWidth(),
        ) {
            Icon(
                painter = painterResource(R.drawable.ri_share_line),
                contentDescription = null,
                modifier = Modifier.size(18.dp),
            )
            Spacer(Modifier.width(8.dp))
            Text(Strings.t("settings.incall.share", lang))
        }
    }
}
```

**Step 5: Pass roomUrl from CallScreen**

In `CallScreen.kt` at line 301, update the `InCallSettingsSheet` call:

```kotlin
        InCallSettingsSheet(
            roomUrl = roomUrl,        // NEW — roomUrl is already a parameter of CallScreen
            initialTab = inCallSettingsTab,
            ...
```

**Step 6: Check for missing drawable resources**

Verify these drawables exist: `ri_global_line`, `ri_smartphone_line`, `ri_share_line`, `ri_check_line`, `ri_file_copy_line`. If any are missing, download from Remix Icon SVGs or use Material icons as fallback.

**Step 7: Build to verify**

Run: `cd android && ./gradlew assembleDebug`
Expected: Build succeeds

**Step 8: Commit**

```bash
git add android/app/src/main/kotlin/io/visio/mobile/ui/InCallSettingsSheet.kt android/app/src/main/kotlin/io/visio/mobile/ui/CallScreen.kt
git commit -m "feat(android): add room info tab with share links in in-call settings"
```

---

### Task 9: iOS — Add Create Room dialog on HomeView

**Files:**
- Modify: `ios/VisioMobile/Views/HomeView.swift`

**Step 1: Add state and button**

In `HomeView`, add state:

```swift
@State private var showCreateRoom: Bool = false
```

After the Join button (around line 150), add (inside the authenticated check):

```swift
if manager.isAuthenticated {
    Button {
        showCreateRoom = true
    } label: {
        Label(Strings.t("home.createRoom", lang: lang), systemImage: "plus.rectangle")
            .font(.headline)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 12)
    }
    .buttonStyle(.bordered)
    .tint(VisioColors.primary500)
    .padding(.horizontal, 32)
}
```

**Step 2: Add sheet**

Add a `.sheet` modifier (after the existing settings sheet):

```swift
.sheet(isPresented: $showCreateRoom) {
    CreateRoomSheet(
        meetInstances: meetInstances,
        lang: lang,
        onCreated: { roomUrl in
            showCreateRoom = false
            roomURL = roomUrl
            // Auto-join: trigger navigation
            navigateToCall = true
        },
        onCancel: { showCreateRoom = false }
    )
    .environmentObject(manager)
}
```

**Step 3: Add CreateRoomSheet view**

Add at the bottom of `HomeView.swift` (before `#Preview`):

```swift
private struct CreateRoomSheet: View {
    @EnvironmentObject private var manager: VisioManager
    let meetInstances: [String]
    let lang: String
    let onCreated: (String) -> Void
    let onCancel: () -> Void

    @State private var name: String = ""
    @State private var accessLevel: String = "public"
    @State private var creating: Bool = false
    @State private var error: String? = nil

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField(Strings.t("home.createRoom.namePlaceholder", lang: lang), text: $name)
                } header: {
                    Text(Strings.t("home.createRoom.name", lang: lang))
                }

                Section {
                    Picker(Strings.t("home.createRoom.access", lang: lang), selection: $accessLevel) {
                        Text(Strings.t("home.createRoom.public", lang: lang)).tag("public")
                        Text(Strings.t("home.createRoom.trusted", lang: lang)).tag("trusted")
                    }
                    .pickerStyle(.inline)

                    if accessLevel == "public" {
                        Text(Strings.t("home.createRoom.publicDesc", lang: lang))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    } else {
                        Text(Strings.t("home.createRoom.trustedDesc", lang: lang))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                } header: {
                    Text(Strings.t("home.createRoom.access", lang: lang))
                }

                if let error {
                    Section {
                        Text(error)
                            .foregroundStyle(.red)
                            .font(.caption)
                    }
                }

                Section {
                    Button {
                        guard let meetInstance = meetInstances.first else { return }
                        creating = true
                        error = nil
                        DispatchQueue.global(qos: .userInitiated).async {
                            do {
                                let result = try manager.client.createRoom(
                                    meetUrl: "https://\(meetInstance)",
                                    name: name.trimmingCharacters(in: .whitespacesAndNewlines),
                                    accessLevel: accessLevel
                                )
                                DispatchQueue.main.async {
                                    onCreated("https://\(meetInstance)/\(result.slug)")
                                }
                            } catch {
                                DispatchQueue.main.async {
                                    self.error = error.localizedDescription
                                    creating = false
                                }
                            }
                        }
                    } label: {
                        HStack {
                            Spacer()
                            Text(creating
                                ? Strings.t("home.createRoom.creating", lang: lang)
                                : Strings.t("home.createRoom.create", lang: lang))
                                .fontWeight(.semibold)
                            Spacer()
                        }
                    }
                    .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || creating)
                }
            }
            .navigationTitle(Strings.t("home.createRoom", lang: lang))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(Strings.t("settings.cancel", lang: lang)) { onCancel() }
                }
            }
        }
    }
}
```

**Step 4: Build to verify**

Run: `scripts/build-ios.sh sim`
Expected: Build succeeds

**Step 5: Commit**

```bash
git add ios/VisioMobile/Views/HomeView.swift
git commit -m "feat(ios): add room creation dialog with auto-join"
```

---

### Task 10: iOS — Add Room Info tab in InCallSettingsSheet

**Files:**
- Modify: `ios/VisioMobile/Views/InCallSettingsSheet.swift`
- Modify: `ios/VisioMobile/Views/CallView.swift` (pass roomURL)

**Step 1: Add `roomURL` property to InCallSettingsSheet**

At line 4, update the struct:

```swift
struct InCallSettingsSheet: View {
    @EnvironmentObject private var manager: VisioManager
    @Environment(\.dismiss) private var dismiss

    let roomURL: String           // NEW
    @State var selectedTab: Int
```

**Step 2: Add 4th tab button**

After the bell.fill tab button (line 20), add:

```swift
                    tabButton(icon: "info.circle.fill", tab: 3, label: Strings.t("settings.incall.roomInfo", lang: lang))
```

**Step 3: Add tab content**

In the `switch selectedTab` (line 31), add:

```swift
                    case 3: roomInfoTab
```

**Step 4: Add roomInfoTab computed property**

Add after the `notificationsTab` property:

```swift
    // MARK: - Room Info Tab

    private var roomInfoTab: some View {
        let displayUrl = roomURL.replacingOccurrences(of: "https://", with: "")
                                .replacingOccurrences(of: "http://", with: "")
        let deepLink = "visio://\(displayUrl)"

        return ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                // HTTPS link
                VStack(alignment: .leading, spacing: 8) {
                    Text(Strings.t("settings.incall.roomLink", lang: lang))
                        .font(.subheadline)
                        .fontWeight(.semibold)
                        .foregroundStyle(VisioColors.onBackground(dark: isDark))

                    HStack {
                        Image(systemName: "globe")
                            .foregroundStyle(VisioColors.primary500)
                        Text(displayUrl)
                            .font(.caption)
                            .foregroundStyle(VisioColors.onBackground(dark: isDark))
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Spacer()
                        Button {
                            UIPasteboard.general.string = roomURL
                        } label: {
                            Image(systemName: "doc.on.doc")
                                .font(.caption)
                        }
                    }
                    .padding(12)
                    .background(VisioColors.surface(dark: isDark))
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                }

                // Deep link
                VStack(alignment: .leading, spacing: 8) {
                    Text(Strings.t("settings.incall.deepLink", lang: lang))
                        .font(.subheadline)
                        .fontWeight(.semibold)
                        .foregroundStyle(VisioColors.onBackground(dark: isDark))

                    HStack {
                        Image(systemName: "apps.iphone")
                            .foregroundStyle(VisioColors.primary500)
                        Text(deepLink)
                            .font(.caption)
                            .foregroundStyle(VisioColors.onBackground(dark: isDark))
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Spacer()
                        Button {
                            UIPasteboard.general.string = deepLink
                        } label: {
                            Image(systemName: "doc.on.doc")
                                .font(.caption)
                        }
                    }
                    .padding(12)
                    .background(VisioColors.surface(dark: isDark))
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                }

                // Share button
                ShareLink(item: roomURL) {
                    Label(Strings.t("settings.incall.share", lang: lang), systemImage: "square.and.arrow.up")
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                }
                .buttonStyle(.borderedProminent)
                .tint(VisioColors.primary500)
            }
            .padding()
        }
    }
```

**Step 5: Pass roomURL from CallView**

In `CallView.swift`, find where `InCallSettingsSheet` is presented and add the `roomURL` parameter:

```swift
InCallSettingsSheet(roomURL: roomURL, selectedTab: 0)
```

`roomURL` is already a property of `CallView`.

**Step 6: Build to verify**

Run: `scripts/build-ios.sh sim`
Expected: Build succeeds

**Step 7: Commit**

```bash
git add ios/VisioMobile/Views/InCallSettingsSheet.swift ios/VisioMobile/Views/CallView.swift
git commit -m "feat(ios): add room info tab with share links in in-call settings"
```

---

### Task 11: End-to-end manual testing

**Prerequisites:** Authenticated session on meet.linagora.com

**Test matrix:**

| Test Case | Android | iOS | Desktop |
|-----------|---------|-----|---------|
| "Create room" button hidden when anonymous | | | |
| "Create room" button visible when authenticated | | | |
| Dialog shows name + access level (Public/Trusted) | | | |
| Room creation succeeds, auto-joins | | | |
| In-call settings shows Room Info tab | | | |
| HTTPS link is correct and copyable | | | |
| visio:// link is correct and copyable | | | |
| Share button opens native share sheet | | | |
| Creating room with empty name is disabled | | | |
| Creating room when not auth shows error | | | |

**Step 1: Test on Desktop first** (fastest iteration)

Run: `cd crates/visio-desktop && cargo tauri dev`

**Step 2: Test on Android**

Run: `cd android && ./gradlew installDebug`

**Step 3: Test on iOS**

Run: Xcode build to simulator

**Step 4: Fix any issues found**

**Step 5: Final commit**

```bash
git commit -m "fix: address issues found during room creation testing"
```
