// Targeted regression tests for previously reported bugs.

#[cfg(test)]
mod diagnostics {
    use base64::Engine;
    use encodeur_rsa_rust::app::chat_manager::ChatManager;
    use encodeur_rsa_rust::types::Config;
    use encodeur_rsa_rust::util::sanitize_filename;

    use uuid::Uuid;

    #[tokio::test]
    async fn test_send_file_without_session_fails() {
        let mut manager = ChatManager::new(Config::default());
        let chat_id = Uuid::new_v4();
        manager.create_local_chat_for_test(chat_id, "Test Chat".to_string());

        let file = tempfile::NamedTempFile::new().unwrap();
        let file_path = file.path().to_path_buf();

        let result = manager.send_file(chat_id, file_path).await;
        let err = result.expect_err("send_file should fail without an active session");
        assert!(err.to_string().contains("Session not found"));
    }

    #[tokio::test]
    async fn test_send_file_rejects_directory_with_clear_error() {
        let mut manager = ChatManager::new(Config::default());
        let chat_id = Uuid::new_v4();
        manager.create_local_chat_for_test(chat_id, "Test Chat".to_string());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        manager.add_session_for_test(
            chat_id,
            encodeur_rsa_rust::app::chat_manager::SessionHandle { from_app_tx: tx },
        );

        let dir = tempfile::tempdir().unwrap();
        let result = manager.send_file(chat_id, dir.path().to_path_buf()).await;
        let err = result.expect_err("directory path should be rejected");
        assert!(err.to_string().contains("not a regular file"));
    }

    #[tokio::test]
    async fn test_send_file_rejects_missing_path_with_clear_error() {
        let mut manager = ChatManager::new(Config::default());
        let chat_id = Uuid::new_v4();
        manager.create_local_chat_for_test(chat_id, "Test Chat".to_string());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        manager.add_session_for_test(
            chat_id,
            encodeur_rsa_rust::app::chat_manager::SessionHandle { from_app_tx: tx },
        );

        let missing_path = std::env::temp_dir().join(format!("missing-{}.txt", Uuid::new_v4()));
        let result = manager.send_file(chat_id, missing_path.clone()).await;
        let err = result.expect_err("missing file should be rejected");
        assert!(
            err.to_string().contains("file not found")
                || err.to_string().contains("not available locally")
        );
    }

    #[test]
    fn test_sanitization_robustness() {
        // Verify our assumption that ../ is neutralized
        let bad_input = "../../../windows/system32/cmd.exe";
        let sanitized = sanitize_filename(bad_input);

        // Expect separators to be replaced
        assert!(!sanitized.contains("/"));
        assert!(!sanitized.contains("\\"));

        // It likely looks like ".._.._.._windows_system32_cmd.exe"
        // This is safe for Path::join
        assert!(!sanitized.contains(".."));
        assert!(!sanitized.is_empty());
    }

    #[test]
    fn invite_link_with_address_autofills() {
        let manager = ChatManager::new(Config::default());
        let payload = serde_json::json!({
            "name": "Diag",
            "address": "172.20.0.5:6000",
            "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----"
        });
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_string(&payload).unwrap());
        let link = format!("chat-p2p://invite/{}", encoded);

        let contact = manager.parse_invite_link(&link).unwrap();
        assert_eq!(contact.address.as_deref(), Some("172.20.0.5:6000"));
        assert_eq!(contact.name, "Diag");
    }
}
