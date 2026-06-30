use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_manager(dir: &std::path::Path) -> OAuthManager {
        OAuthManager {
            client: reqwest::Client::new(),
            config: AuthConfig::default(),
            credentials_path: dir.join("auth.json"),
        }
    }

    fn make_credentials(hours_from_now: i64) -> OAuthCredentials {
        OAuthCredentials {
            access_token: "test-access-token".to_string(),
            refresh_token: Some("test-refresh-token".to_string()),
            expires_at: Utc::now() + chrono::Duration::hours(hours_from_now),
            token_type: "Bearer".to_string(),
        }
    }

    #[tokio::test]
    async fn test_save_and_load_credentials() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path());
        let creds = make_credentials(1);

        manager.save_credentials(&creds).unwrap();
        let loaded = manager.load_credentials().unwrap();

        assert_eq!(loaded.access_token, "test-access-token");
        assert_eq!(loaded.refresh_token, Some("test-refresh-token".to_string()));
        assert_eq!(loaded.token_type, "Bearer");
        // Allow 1 second tolerance for serialization round-trip
        let diff = (loaded.expires_at - creds.expires_at).num_seconds().abs();
        assert!(diff <= 1, "expires_at mismatch: diff={diff}s");
    }

    #[tokio::test]
    async fn test_has_credentials_false_when_empty() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path());

        assert!(!manager.has_credentials());
    }

    #[tokio::test]
    async fn test_logout_deletes_credentials() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path());
        let creds = make_credentials(1);

        manager.save_credentials(&creds).unwrap();
        assert!(manager.has_credentials());

        manager.logout().unwrap();
        assert!(!manager.has_credentials());
        assert!(!manager.credentials_path.exists());
    }

    #[tokio::test]
    async fn test_get_token_returns_valid_token() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path());
        let creds = make_credentials(1);

        manager.save_credentials(&creds).unwrap();

        let token = manager.get_token().await.unwrap();
        assert_eq!(token, "test-access-token");
    }

    #[tokio::test]
    async fn test_get_token_refreshes_expired() {
        let tmp = TempDir::new().unwrap();
        let mock_server = MockServer::start().await;

        let manager = OAuthManager {
            client: reqwest::Client::new(),
            config: AuthConfig {
                auth_url: mock_server.uri(),
                token_url: format!("{}/token", mock_server.uri()),
                client_id: "test".to_string(),
            },
            credentials_path: tmp.path().join("auth.json"),
        };

        // Save expired credentials
        let expired_creds = make_credentials(-1);
        manager.save_credentials(&expired_creds).unwrap();

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-token",
                "refresh_token": "new-refresh",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&mock_server)
            .await;

        let token = manager.get_token().await.unwrap();
        assert_eq!(token, "new-token");

        // Verify new credentials were persisted
        let reloaded = manager.load_credentials().unwrap();
        assert_eq!(reloaded.access_token, "new-token");
        assert_eq!(reloaded.refresh_token, Some("new-refresh".to_string()));
    }
}
