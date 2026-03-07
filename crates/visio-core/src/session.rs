use reqwest::header::{HeaderMap, HeaderValue, COOKIE};
use serde::{Deserialize, Serialize};

use crate::errors::VisioError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub display_name: String,
}

#[derive(Debug, Clone)]
pub enum SessionState {
    Anonymous,
    Authenticated { user: UserInfo, cookie: String },
}

pub struct SessionManager {
    state: SessionState,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            state: SessionState::Anonymous,
        }
    }

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    pub fn set_authenticated(&mut self, user: UserInfo, cookie: String) {
        self.state = SessionState::Authenticated { user, cookie };
    }

    pub fn clear(&mut self) {
        self.state = SessionState::Anonymous;
    }

    pub fn cookie(&self) -> Option<String> {
        match &self.state {
            SessionState::Authenticated { cookie, .. } => Some(cookie.clone()),
            SessionState::Anonymous => None,
        }
    }

    pub fn user(&self) -> Option<&UserInfo> {
        match &self.state {
            SessionState::Authenticated { user, .. } => Some(user),
            SessionState::Anonymous => None,
        }
    }

    pub async fn fetch_user(meet_url: &str, cookie: &str) -> Result<UserInfo, VisioError> {
        let url = format!("{}/api/v1.0/users/me/", meet_url);

        let mut headers = HeaderMap::new();
        let cookie_value = format!("sessionid={}", cookie);
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&cookie_value)
                .map_err(|e| VisioError::Http(e.to_string()))?,
        );

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| VisioError::Http(e.to_string()))?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(VisioError::Session(
                "Session expired or invalid".to_string(),
            ));
        }

        let body = response
            .text()
            .await
            .map_err(|e| VisioError::Http(e.to_string()))?;

        serde_json::from_str::<UserInfo>(&body)
            .map_err(|e| VisioError::Session(format!("Failed to parse user info: {}", e)))
    }

    pub async fn validate_session(&mut self, meet_url: &str) -> Result<bool, VisioError> {
        let cookie = match self.cookie() {
            Some(c) => c,
            None => return Ok(false),
        };

        match Self::fetch_user(meet_url, &cookie).await {
            Ok(user) => {
                self.state = SessionState::Authenticated { user, cookie };
                Ok(true)
            }
            Err(_) => {
                self.clear();
                Ok(false)
            }
        }
    }

    pub async fn logout(&mut self, meet_url: &str) -> Result<(), VisioError> {
        if let Some(cookie) = self.cookie() {
            let url = format!("{}/logout", meet_url);
            let mut headers = HeaderMap::new();
            let cookie_value = format!("sessionid={}", cookie);
            if let Ok(val) = HeaderValue::from_str(&cookie_value) {
                headers.insert(COOKIE, val);
                let client = reqwest::Client::new();
                let _ = client.get(&url).headers(headers).send().await;
            }
        }
        self.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_state_default_is_anonymous() {
        let session = SessionManager::new();
        assert!(matches!(session.state(), SessionState::Anonymous));
    }

    #[test]
    fn test_set_cookie_changes_state() {
        let mut session = SessionManager::new();
        let user = UserInfo {
            id: "123".to_string(),
            email: "test@example.com".to_string(),
            display_name: "Test User".to_string(),
        };
        session.set_authenticated(user.clone(), "abc123".to_string());
        match session.state() {
            SessionState::Authenticated { user: u, .. } => {
                assert_eq!(u.display_name, "Test User");
            }
            _ => panic!("Expected Authenticated state"),
        }
    }

    #[test]
    fn test_clear_session_returns_to_anonymous() {
        let mut session = SessionManager::new();
        let user = UserInfo {
            id: "123".to_string(),
            email: "test@example.com".to_string(),
            display_name: "Test".to_string(),
        };
        session.set_authenticated(user, "abc123".to_string());
        session.clear();
        assert!(matches!(session.state(), SessionState::Anonymous));
    }

    #[test]
    fn test_cookie_returns_none_when_anonymous() {
        let session = SessionManager::new();
        assert!(session.cookie().is_none());
    }

    #[test]
    fn test_cookie_returns_value_when_authenticated() {
        let mut session = SessionManager::new();
        let user = UserInfo {
            id: "1".to_string(),
            email: "a@b.com".to_string(),
            display_name: "A".to_string(),
        };
        session.set_authenticated(user, "mycookie".to_string());
        assert_eq!(session.cookie(), Some("mycookie".to_string()));
    }

    #[tokio::test]
    async fn test_fetch_user_with_invalid_cookie_returns_error() {
        let result =
            SessionManager::fetch_user("https://meet.example.com", "invalid_cookie").await;
        assert!(result.is_err());
    }
}
