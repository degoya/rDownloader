//! The planting requests of the canary suite, apart so the suite stays readable: one row per
//! credential the service keeps, sent through the route the interface uses for it.

/// The planting requests, in order: the name the answer's `id` is kept under (`-` for none),
/// method, route and body. `{name}` is a canary, a kept id, `{refusing}` (an HTTP fixture that
/// refuses every request) or `{ftp-port}` (an FTP fixture that refuses every login).
pub(super) const PLANTINGS: &[(&str, &str, &str, &str)] = &[
    (
        "proxy",
        "POST",
        "/api/v1/proxy-profiles",
        r#"{"name": "canary-proxy", "kind": "socks5", "endpoint": "socks5h://127.0.0.1:1080",
            "username": "canary-proxy-user", "password": "{proxy-password}"}"#,
    ),
    (
        "key-account",
        "POST",
        "/api/v1/accounts",
        r#"{"provider": "canarykey", "label": "canary-key-account", "username": null,
            "credential_mode": null, "secret": "{account-api-key}",
            "cookies": "{account-cookies}", "proxy_profile_id": "{proxy}", "enabled": true}"#,
    ),
    (
        "login-account",
        "POST",
        "/api/v1/accounts",
        r#"{"provider": "canarylogin", "label": "canary-login-account",
            "username": "canary-login-user", "credential_mode": null,
            "secret": "{account-password}", "cookies": null, "proxy_profile_id": null,
            "enabled": true}"#,
    ),
    (
        "bucket",
        "POST",
        "/api/v1/object-storage/profiles",
        r#"{"name": "canary-bucket-profile", "provider": "s3", "endpoint": "{refusing}",
            "region": "us-east-1", "bucket": "canary-bucket", "credential_source": "static",
            "access_key_id": "AKIACANARYFIXTURE", "secret_access_key": "{s3-secret-key}",
            "session_token": "{s3-session-token}"}"#,
    ),
    // RD-1190-20: an Azure shared access signature, the most exposed of the three because it
    // travels in the request address, and a Google service account key.
    (
        "blob",
        "POST",
        "/api/v1/object-storage/profiles",
        r#"{"name": "canary-blob-profile", "provider": "azure", "endpoint": "{refusing}",
            "account": "canaryaccount", "bucket": "canary-container",
            "credential_source": "shared_access_signature",
            "secret_access_key": "sv=2024-11-04&ss=b&srt=co&sp=rl&sig={azure-sas-signature}"}"#,
    ),
    (
        "gcs",
        "POST",
        "/api/v1/object-storage/profiles",
        r#"{"name": "canary-gcs-profile", "provider": "gcs", "endpoint": "{refusing}",
            "bucket": "canary-bucket", "credential_source": "static",
            "secret_access_key": "{\"type\": \"service_account\", \"private_key_id\": \"canary-key-id\", \"client_email\": \"canary@canary.iam.gserviceaccount.com\", \"private_key\": \"-----BEGIN PRIVATE KEY-----\\n{gcs-private-key}\\n-----END PRIVATE KEY-----\\n\"}"}"#,
    ),
    (
        "ftp",
        "POST",
        "/api/v1/remote-credentials",
        r#"{"name": "canary-ftp", "protocol": "ftp", "host": "127.0.0.1", "port": {ftp-port},
            "username": "canary-ftp-user", "auth_mode": "password", "secret": "{ftp-password}"}"#,
    ),
    (
        "webhook",
        "POST",
        "/api/v1/notifications/targets",
        r#"{"name": "canary-webhook", "kind": "webhook", "endpoint": "{refusing}/hook",
            "secret": "{webhook-secret}"}"#,
    ),
    (
        "-",
        "POST",
        "/api/v1/notifications/targets",
        r#"{"name": "canary-apprise", "kind": "apprise", "endpoint": "ntfys",
            "secret": "ntfys://{ntfy-token}@ntfy.canary.test/canaries"}"#,
    ),
    (
        "-",
        "POST",
        "/api/v1/usenet/servers",
        r#"{"name": "canary-news", "host": "news.canary.test", "port": 563, "tls": true,
            "username": "canary-reader", "password": "{nntp-password}", "proxy_profile_id": null,
            "priority": 0, "max_connections": 2, "enabled": false}"#,
    ),
    (
        "-",
        "POST",
        "/api/v1/auth-profiles",
        r#"{"name": "canary-auth-profile", "scope": "files.canary.test",
            "include_subdomains": false, "method": "bearer", "username": null,
            "secret": "{auth-profile-token}", "certificate_pem": null, "expires_at": null,
            "enabled": true}"#,
    ),
    (
        "-",
        "PUT",
        "/api/v1/captcha-config",
        r#"{"solver": "two_captcha_compatible", "endpoint": "https://captcha.canary.test",
            "api_key": "{captcha-api-key}"}"#,
    ),
    (
        "-",
        "PUT",
        "/api/v1/backups/passphrase",
        r#"{"passphrase": "{backup-passphrase}"}"#,
    ),
];
