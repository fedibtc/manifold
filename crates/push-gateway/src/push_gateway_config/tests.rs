use std::sync::Mutex;

use super::*;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn default_provider_is_noop_even_when_credentials_are_present() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let env_json = service_account_json("env-project");
    with_env(
        &[
            ("FCM_SERVICE_ACCOUNT_JSON", Some(env_json.as_str())),
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
        ],
        || {
            let config = PushGatewayConfig::from_env().expect("config");
            assert!(matches!(config.provider(), PushProviderConfig::Noop));
        },
    );
}

#[test]
fn production_mode_rejects_zero_for_every_mandatory_admission_cap_family() {
    let credentials =
        FirebaseCredentials::from_json(&service_account_json("prod-project")).unwrap();
    let base = PushGatewayConfig::new(None, "postgres://push.example.test/push", Some(credentials))
        .try_with_public_base_url("https://push.example.test")
        .unwrap()
        .with_production_mode(true)
        .with_open_self_registration_enabled(true)
        .with_legacy_notification_hook_enabled(false);
    let valid_limits = RateLimitConfig {
        max_global_outbox_backlog: 1000,
        max_recipient_outbox_backlog: 20,
        ..RateLimitConfig::default()
    };

    type ZeroCap = (&'static str, fn(&mut RateLimitConfig));
    let zeroers: &[ZeroCap] = &[
        ("auth source", |v| v.auth_events_per_source_prefix = 0),
        ("auth window", |v| v.auth_event_window_seconds = 0),
        ("hook source", |v| v.hook_invocations_per_source_prefix = 0),
        ("hook token", |v| v.hook_invocations_per_hook = 0),
        ("hook invocation window", |v| {
            v.hook_invocation_window_seconds = 0
        }),
        ("hook creation", |v| v.hook_creations_per_recipient = 0),
        ("hook creation window", |v| {
            v.hook_creation_window_seconds = 0
        }),
        ("registration source", |v| {
            v.registration_changes_per_source_prefix = 0
        }),
        ("registration recipient/source", |v| {
            v.registration_changes_per_recipient_source = 0;
        }),
        ("registration window", |v| {
            v.registration_change_window_seconds = 0
        }),
        ("active hooks recipient", |v| {
            v.max_active_hooks_per_recipient = 0
        }),
        ("active registrations recipient", |v| {
            v.max_active_installations_per_recipient = 0;
        }),
        ("active hooks global", |v| v.max_active_hooks_global = 0),
        ("active registrations global", |v| {
            v.max_active_installations_global = 0;
        }),
        ("physical hooks global", |v| v.max_hook_rows_global = 0),
        ("physical registrations global", |v| {
            v.max_registration_rows_global = 0;
        }),
        ("admission GC batch", |v| v.admission_gc_batch_size = 0),
    ];

    for (name, zero) in zeroers {
        let mut limits = valid_limits.clone();
        zero(&mut limits);
        let err =
            validate_production_safety(&base.clone().with_rate_limits(limits)).expect_err(name);
        assert!(matches!(err, PushGatewayConfigError::ProductionSafety(_)));
    }
}

#[test]
fn unknown_provider_mode_is_rejected() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(&[("PUSH_GATEWAY_PROVIDER", Some("fcmm"))], || {
        let err = PushGatewayConfig::from_env().expect_err("unknown mode");
        assert!(matches!(
            err,
            PushGatewayConfigError::UnknownProviderMode(_)
        ));
        assert_eq!(err.to_string(), "unknown push provider mode");
    });
}

#[test]
fn fcm_env_config_uses_file_precedence_and_overrides() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let credentials_path = tempdir.path().join("service-account.json");
    std::fs::write(&credentials_path, service_account_json("file-project"))
        .expect("write credentials");
    let env_json = service_account_json("env-project");
    with_env(
        &[
            ("PUSH_GATEWAY_PROVIDER", Some(" FCM ")),
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
            (
                "FCM_SERVICE_ACCOUNT_FILE",
                Some(credentials_path.to_str().expect("path")),
            ),
            ("FCM_SERVICE_ACCOUNT_JSON", Some(env_json.as_str())),
            ("FCM_SEND_ENDPOINT_BASE", Some("http://127.0.0.1:9")),
            ("FCM_MAX_CONCURRENCY", Some("3")),
        ],
        || {
            let config = PushGatewayConfig::from_env().expect("config");
            let PushProviderConfig::Fcm(fcm) = config.provider() else {
                panic!("expected fcm provider");
            };
            assert_eq!(fcm.credentials().project_id(), "file-project");
            assert_eq!(fcm.send_endpoint_base(), "http://127.0.0.1:9");
            assert_eq!(fcm.max_concurrency(), 3);
        },
    );
}

#[test]
fn fcm_mode_requires_credentials() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(&[("PUSH_GATEWAY_PROVIDER", Some("fcm"))], || {
        let err = PushGatewayConfig::from_env().expect_err("missing credentials");
        assert!(matches!(err, PushGatewayConfigError::MissingFcmCredentials));
    });
}

#[test]
fn cli_arguments_override_environment_configuration() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(
        &[
            ("PUSH_GATEWAY_BIND", Some("127.0.0.1:3001")),
            ("PUSH_GATEWAY_APP_ID", Some("env-app")),
            (
                "PUSH_GATEWAY_DATABASE_URL",
                Some("sqlite://env.sqlite?mode=rwc"),
            ),
            ("PUSH_GATEWAY_PUBLIC_BASE_URL", Some("https://env.test")),
            ("PUSH_GATEWAY_OUTBOX_WORKER_CONCURRENCY", Some("2")),
        ],
        || {
            let args = PushGatewayArgs::try_parse_from([
                "push-gateway",
                "--bind",
                "127.0.0.1:3002",
                "--app-id",
                "cli-app",
                "--database-url",
                "sqlite://cli.sqlite?mode=rwc",
                "--public-base-url",
                "https://cli.test",
                "--outbox-worker-concurrency",
                "5",
            ])
            .expect("args");
            assert_eq!(args.bind().to_string(), "127.0.0.1:3002");
            let config = args.into_config().expect("config");
            assert_eq!(config.app_id(), Some(&AppId("cli-app".to_owned())));
            assert_eq!(config.database_url(), "sqlite://cli.sqlite?mode=rwc");
            assert_eq!(config.public_base_url(), "https://cli.test");
            assert!(!config.unsafe_allow_any_app_id_for_tests());
            assert_eq!(config.outbox_worker_concurrency(), 5);
        },
    );
}

#[test]
fn rate_limit_configuration_defaults_and_overrides_parse() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(
        &[(
            "PUSH_GATEWAY_PUBLIC_BASE_URL",
            Some("https://push.example.test"),
        )],
        || {
            let default = PushGatewayConfig::from_env().expect("config");
            assert_eq!(
                default.rate_limits().max_active_installations_per_recipient,
                8
            );
            assert_eq!(default.rate_limits().max_active_hooks_per_recipient, 20);
            assert_eq!(default.rate_limits().hook_creations_per_recipient, 5);
            assert_eq!(
                default
                    .rate_limits()
                    .registration_changes_per_recipient_source,
                10
            );
            assert_eq!(default.rate_limits().hook_invocations_per_source_prefix, 60);
        },
    );
    with_env(
        &[
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
            ("PUSH_GATEWAY_HOOK_INVOCATIONS_PER_SOURCE_PREFIX", Some("7")),
            ("PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG", Some("99")),
            (
                "PUSH_GATEWAY_TRUSTED_PROXY_CIDRS",
                Some("10.0.0.0/8,2001:db8::/32"),
            ),
        ],
        || {
            let config = PushGatewayConfig::from_env().expect("config");
            assert_eq!(config.rate_limits().hook_invocations_per_source_prefix, 7);
            assert_eq!(config.rate_limits().max_global_outbox_backlog, 99);
            assert!(config.rate_limits().trusted_proxy_cidrs.is_some());
        },
    );
}

#[test]
fn invalid_trusted_proxy_cidr_is_rejected() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(
        &[
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
            ("PUSH_GATEWAY_TRUSTED_PROXY_CIDRS", Some("10.0.0.0/99")),
        ],
        || {
            let err = PushGatewayConfig::from_env().expect_err("invalid cidr");
            assert!(matches!(
                err,
                PushGatewayConfigError::InvalidTrustedProxyCidrs(_)
            ));
        },
    );
}

#[test]
fn operator_endpoint_configuration_defaults_and_overrides_parse() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(
        &[(
            "PUSH_GATEWAY_PUBLIC_BASE_URL",
            Some("https://push.example.test"),
        )],
        || {
            let default = PushGatewayConfig::from_env().expect("config");
            assert_eq!(default.operator_bind(), None);
            assert_eq!(default.operator_token(), None);
            assert!(!default.public_metrics_enabled());
        },
    );
    with_env(
        &[
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
            ("PUSH_GATEWAY_OPERATOR_BIND", Some("127.0.0.1:9100")),
            ("PUSH_GATEWAY_OPERATOR_TOKEN", Some(" secret-token ")),
            ("PUSH_GATEWAY_PUBLIC_METRICS_ENABLED", Some("true")),
        ],
        || {
            let config = PushGatewayConfig::from_env().expect("config");
            assert_eq!(
                config.operator_bind(),
                Some("127.0.0.1:9100".parse().expect("addr"))
            );
            assert_eq!(
                config.operator_token().map(OperatorToken::as_str),
                Some("secret-token")
            );
            assert!(config.public_metrics_enabled());
        },
    );
}

#[test]
fn unsafe_any_app_id_escape_hatch_is_explicit() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(
        &[
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
            (
                "PUSH_GATEWAY_UNSAFE_ALLOW_ANY_APP_ID_FOR_TESTS",
                Some("true"),
            ),
        ],
        || {
            let config = PushGatewayConfig::from_env().expect("config");
            assert!(config.app_id().is_none());
            assert!(config.unsafe_allow_any_app_id_for_tests());
        },
    );
}

#[test]
fn debug_redacts_app_id_database_credentials_and_fcm_credentials() {
    let config = PushGatewayConfig::new(
        Some(AppId("secret-app".to_owned())),
        "postgres://db_user:secret-db-password@db.example.test/push",
        Some(
            FirebaseCredentials::from_json(&service_account_json("secret-project"))
                .expect("credentials"),
        ),
    )
    .try_with_public_base_url("https://push.example.test")
    .expect("public base URL");

    let debug = format!("{config:?}");
    assert!(debug.contains("PushGatewayConfig"));
    assert!(debug.contains("app_id: Some(\"<redacted>\")"));
    assert!(debug.contains("postgres://<redacted>@db.example.test/push"));
    assert!(debug.contains("FirebaseCredentials(<redacted>)"));
    assert!(!debug.contains("secret-app"));
    assert!(!debug.contains("secret-db-password"));
    assert!(!debug.contains("db_user"));
    assert!(!debug.contains("secret-project"));
    assert!(!debug.contains("dummy-private-key"));
}

#[test]
fn public_base_url_must_be_https_origin_unless_local_escape_hatch_is_set() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    for invalid in [
        "",
        "http://push.example.test",
        "https://user:pass@push.example.test",
        "https://:443",
        "https://push.example.test:bad",
        "https://push.example.test/path",
        "https://push.example.test?x=1",
        "https://push.example.test#fragment",
    ] {
        with_env(&[("PUSH_GATEWAY_PUBLIC_BASE_URL", Some(invalid))], || {
            let err = PushGatewayConfig::from_env().expect_err("invalid public base url");
            assert!(matches!(err, PushGatewayConfigError::InvalidPublicBaseUrl));
        });
    }

    with_env(
        &[(
            "PUSH_GATEWAY_PUBLIC_BASE_URL",
            Some(" https://push.example.test/ "),
        )],
        || {
            let config = PushGatewayConfig::from_env().expect("trimmed https config");
            assert_eq!(config.public_base_url(), "https://push.example.test");
        },
    );

    with_env(
        &[
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("http://localhost:3000"),
            ),
            ("PUSH_GATEWAY_ALLOW_INSECURE_PUBLIC_BASE_URL", Some("true")),
        ],
        || {
            let config = PushGatewayConfig::from_env().expect("local config");
            assert_eq!(config.public_base_url(), "http://localhost:3000");
        },
    );

    with_env(
        &[("PUSH_GATEWAY_PUBLIC_BASE_URL", Some("http://[::1]:3000"))],
        || {
            let err = PushGatewayConfig::from_env().expect_err("missing escape hatch");
            assert!(matches!(err, PushGatewayConfigError::InvalidPublicBaseUrl));
        },
    );

    with_env(
        &[
            ("PUSH_GATEWAY_PUBLIC_BASE_URL", Some("")),
            ("PUSH_GATEWAY_ALLOW_INSECURE_PUBLIC_BASE_URL", Some("true")),
        ],
        || {
            let config = PushGatewayConfig::from_env().expect("test config");
            assert_eq!(config.public_base_url(), "");
        },
    );
}

#[test]
fn programmatic_public_base_url_constructors_validate_origins() {
    let production = PushGatewayConfig::new(None, DEFAULT_DATABASE_URL, None)
        .try_with_public_base_url(" https://push.example.test:8443/ ")
        .expect("https origin");
    assert_eq!(
        production.public_base_url(),
        "https://push.example.test:8443"
    );

    assert!(
        PushGatewayConfig::new(None, DEFAULT_DATABASE_URL, None)
            .try_with_public_base_url("http://push.example.test")
            .is_err()
    );
    assert!(
        PushGatewayConfig::new(None, DEFAULT_DATABASE_URL, None)
            .try_with_public_base_url("https://push.example.test/path")
            .is_err()
    );

    let local = PushGatewayConfig::new(None, DEFAULT_DATABASE_URL, None)
        .try_with_local_test_public_base_url("http://[::1]:3000")
        .expect("loopback origin");
    assert_eq!(local.public_base_url(), "http://[::1]:3000");
    assert!(
        PushGatewayConfig::new(None, DEFAULT_DATABASE_URL, None)
            .try_with_local_test_public_base_url("http://example.test")
            .is_err()
    );
}

#[test]
fn args_debug_redacts_pre_config_secret_values() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(&[], || {
        let args = PushGatewayArgs::try_parse_from([
            "push-gateway",
            "--app-id",
            "secret-app",
            "--database-url",
            "postgres://db_user:secret-pass@db.example.test/push",
            "--public-base-url",
            "https://push.example.test",
            "--provider",
            "fcm",
            "--fcm-service-account-json",
            service_account_json("secret-project").as_str(),
        ])
        .expect("args");
        let debug = format!("{args:?}");

        assert!(debug.contains("app_id: Some(\"<redacted>\")"));
        assert!(debug.contains("postgres://<redacted>@db.example.test/push"));
        assert!(debug.contains("fcm_service_account_json: Some(\"<redacted>\")"));
        assert!(!debug.contains("secret-app"));
        assert!(!debug.contains("secret-pass"));
        assert!(!debug.contains("db_user"));
        assert!(!debug.contains("secret-project"));
        assert!(!debug.contains("dummy-private-key"));
    });
}

#[test]
fn cli_arguments_configure_fcm_provider() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(&[], || {
        let args = PushGatewayArgs::try_parse_from([
            "push-gateway",
            "--public-base-url",
            "https://push.example.test",
            "--provider",
            "fcm",
            "--fcm-service-account-json",
            service_account_json("cli-project").as_str(),
            "--fcm-send-endpoint-base",
            "http://127.0.0.1:9",
            "--fcm-max-concurrency",
            "7",
        ])
        .expect("args");
        let config = args.into_config().expect("config");
        let PushProviderConfig::Fcm(fcm) = config.provider() else {
            panic!("expected fcm provider");
        };
        assert_eq!(fcm.credentials().project_id(), "cli-project");
        assert_eq!(fcm.send_endpoint_base(), "http://127.0.0.1:9");
        assert_eq!(fcm.max_concurrency(), 7);
    });
}

#[test]
fn bind_env_configures_cli_when_flag_is_absent() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(&[("PUSH_GATEWAY_BIND", Some("127.0.0.1:3010"))], || {
        let args = PushGatewayArgs::try_parse_from([
            "push-gateway",
            "--public-base-url",
            "https://push.example.test",
        ])
        .expect("args");
        assert_eq!(args.bind().to_string(), "127.0.0.1:3010");
    });
}

#[test]
fn cli_fcm_json_overrides_env_fcm_file() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let credentials_path = tempdir.path().join("service-account.json");
    std::fs::write(&credentials_path, service_account_json("env-file-project"))
        .expect("write credentials");
    with_env(
        &[(
            "FCM_SERVICE_ACCOUNT_FILE",
            Some(credentials_path.to_str().expect("path")),
        )],
        || {
            let args = PushGatewayArgs::try_parse_from([
                "push-gateway",
                "--public-base-url",
                "https://push.example.test",
                "--provider",
                "fcm",
                "--fcm-service-account-json",
                service_account_json("cli-json-project").as_str(),
            ])
            .expect("args");
            let config = args.into_config().expect("config");
            let PushProviderConfig::Fcm(fcm) = config.provider() else {
                panic!("expected fcm provider");
            };
            assert_eq!(fcm.credentials().project_id(), "cli-json-project");
        },
    );
}

#[test]
fn cli_fcm_file_overrides_env_fcm_json() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let credentials_path = tempdir.path().join("service-account.json");
    std::fs::write(&credentials_path, service_account_json("cli-file-project"))
        .expect("write credentials");
    let env_json = service_account_json("env-json-project");
    with_env(
        &[("FCM_SERVICE_ACCOUNT_JSON", Some(env_json.as_str()))],
        || {
            let args = PushGatewayArgs::try_parse_from([
                "push-gateway",
                "--public-base-url",
                "https://push.example.test",
                "--provider",
                "fcm",
                "--fcm-service-account-file",
                credentials_path.to_str().expect("path"),
            ])
            .expect("args");
            let config = args.into_config().expect("config");
            let PushProviderConfig::Fcm(fcm) = config.provider() else {
                panic!("expected fcm provider");
            };
            assert_eq!(fcm.credentials().project_id(), "cli-file-project");
        },
    );
}

#[test]
fn help_displays_env_names_without_env_values() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let secret_json = service_account_json("secret-help-project");
    with_env(
        &[
            ("PUSH_GATEWAY_DATABASE_URL", Some("postgres://secret-db")),
            ("FCM_SERVICE_ACCOUNT_JSON", Some(secret_json.as_str())),
        ],
        || {
            let help = PushGatewayCli::command().render_long_help().to_string();
            assert!(help.contains("PUSH_GATEWAY_DATABASE_URL"));
            assert!(help.contains("FCM_SERVICE_ACCOUNT_JSON"));
            assert!(!help.contains("postgres://secret-db"));
            assert!(!help.contains("secret-help-project"));
            assert!(!help.contains(secret_json.as_str()));
        },
    );
}

#[test]
fn outbox_command_uses_database_url_without_requiring_provider_config() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(
        &[
            ("PUSH_GATEWAY_PROVIDER", Some("fcm")),
            ("PUSH_GATEWAY_PUBLIC_BASE_URL", Some("not a valid url")),
        ],
        || {
            let command = PushGatewayCommand::try_parse_from([
                "push-gateway",
                "--database-url",
                "sqlite://admin.sqlite?mode=rwc",
                "outbox",
                "list-dead-letter",
            ])
            .expect("outbox command parses");
            let PushGatewayCommand::Outbox(command) = command else {
                panic!("expected outbox command");
            };
            let (database_url, action) = command.into_parts();
            assert_eq!(database_url, "sqlite://admin.sqlite?mode=rwc");
            assert!(matches!(
                action,
                PushGatewayOutboxAction::ListDeadLetter {
                    limit: 50,
                    json: false
                }
            ));
        },
    );
}

#[test]
fn outbox_command_debug_redacts_database_url_credentials() {
    let command = PushGatewayCommand::try_parse_from([
        "push-gateway",
        "--database-url",
        "postgres://db_user:secret-db-password@db.example.test/push",
        "outbox",
        "dead-letter-reasons",
    ])
    .expect("outbox command parses");

    let debug = format!("{command:?}");
    assert!(debug.contains("postgres://<redacted>@db.example.test/push"));
    assert!(!debug.contains("db_user"));
    assert!(!debug.contains("secret-db-password"));
}

#[test]
fn server_args_reject_outbox_subcommands() {
    let err = PushGatewayArgs::try_parse_from(["push-gateway", "outbox", "dead-letter-reasons"])
        .expect_err("server-only parser rejects subcommands");
    assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
}

#[test]
fn production_mode_requires_explicit_admission_fcm_and_backlog_caps() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let recipient_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let recipient_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let recipients = format!("{recipient_a}, {recipient_b}");
    let credentials = service_account_json("prod-project");
    with_env(
        &[
            ("PUSH_GATEWAY_PRODUCTION_MODE", Some("true")),
            ("PUSH_GATEWAY_PROVIDER", Some("fcm")),
            ("FCM_SERVICE_ACCOUNT_JSON", Some(credentials.as_str())),
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
            (
                "PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS",
                Some(recipients.as_str()),
            ),
            ("PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED", Some("true")),
            ("PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG", Some("1000")),
            ("PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG", Some("20")),
            (
                "PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED",
                Some("false"),
            ),
        ],
        || {
            let err = PushGatewayConfig::from_env().expect_err("admission modes are exclusive");
            assert!(err.to_string().contains("exactly one admission mode"));
        },
    );
    with_env(
        &[
            ("PUSH_GATEWAY_PRODUCTION_MODE", Some("true")),
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
        ],
        || {
            let err = PushGatewayConfig::from_env().expect_err("production safety");
            assert!(matches!(err, PushGatewayConfigError::ProductionSafety(_)));
            assert!(err.to_string().contains("PUSH_GATEWAY_PROVIDER=fcm"));
        },
    );

    with_env(
        &[
            ("PUSH_GATEWAY_PRODUCTION_MODE", Some("true")),
            ("PUSH_GATEWAY_PROVIDER", Some("fcm")),
            ("FCM_SERVICE_ACCOUNT_JSON", Some(credentials.as_str())),
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
            ("PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS", None),
            ("PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED", Some("true")),
            ("PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG", Some("1000")),
            ("PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG", Some("20")),
            (
                "PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED",
                Some("false"),
            ),
        ],
        || {
            let config = PushGatewayConfig::from_env().expect("open production admission");
            assert!(config.recipient_admitted(&crate::RecipientId(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_owned()
            )));
        },
    );

    with_env(
        &[
            ("PUSH_GATEWAY_PRODUCTION_MODE", Some("true")),
            ("PUSH_GATEWAY_PROVIDER", Some("fcm")),
            ("FCM_SERVICE_ACCOUNT_JSON", Some(credentials.as_str())),
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
            (
                "PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS",
                Some(recipients.as_str()),
            ),
            ("PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG", Some("1000")),
            ("PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG", Some("20")),
            (
                "PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED",
                Some("false"),
            ),
        ],
        || {
            let config = PushGatewayConfig::from_env().expect("production config");
            assert!(config.production_mode());
            assert!(matches!(config.provider(), PushProviderConfig::Fcm(_)));
            assert!(!config.legacy_notification_hook_enabled());
            assert!(config.recipient_admitted(&crate::RecipientId(recipient_a.to_owned())));
            assert!(!config.recipient_admitted(&crate::RecipientId(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_owned()
            )));
        },
    );
}

#[test]
fn production_mode_rejects_default_public_base_url() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let credentials = service_account_json("prod-project");
    let recipient = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    with_env(
        &[
            ("PUSH_GATEWAY_PRODUCTION_MODE", Some("true")),
            ("PUSH_GATEWAY_PROVIDER", Some("fcm")),
            ("FCM_SERVICE_ACCOUNT_JSON", Some(credentials.as_str())),
            ("PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS", Some(recipient)),
            ("PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG", Some("1000")),
            ("PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG", Some("20")),
            (
                "PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED",
                Some("false"),
            ),
        ],
        || {
            let err = PushGatewayConfig::from_env().expect_err("production safety");
            assert!(matches!(err, PushGatewayConfigError::ProductionSafety(_)));
            assert!(
                err.to_string()
                    .contains("explicit HTTPS PUSH_GATEWAY_PUBLIC_BASE_URL")
            );
        },
    );
}

#[test]
fn production_mode_rejects_insecure_public_base_url_escape_hatch() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let credentials = service_account_json("prod-project");
    let recipient = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    with_env(
        &[
            ("PUSH_GATEWAY_PRODUCTION_MODE", Some("true")),
            ("PUSH_GATEWAY_PROVIDER", Some("fcm")),
            ("FCM_SERVICE_ACCOUNT_JSON", Some(credentials.as_str())),
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("http://127.0.0.1:3000"),
            ),
            ("PUSH_GATEWAY_ALLOW_INSECURE_PUBLIC_BASE_URL", Some("true")),
            ("PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS", Some(recipient)),
            ("PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG", Some("1000")),
            ("PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG", Some("20")),
            (
                "PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED",
                Some("false"),
            ),
        ],
        || {
            let err = PushGatewayConfig::from_env().expect_err("production safety");
            assert!(matches!(err, PushGatewayConfigError::ProductionSafety(_)));
            assert!(
                err.to_string()
                    .contains("explicit HTTPS PUSH_GATEWAY_PUBLIC_BASE_URL")
            );
        },
    );
}

#[test]
fn production_mode_rejects_fcm_test_overrides() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let recipient = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let base_env = [
        ("PUSH_GATEWAY_PRODUCTION_MODE", Some("true")),
        ("PUSH_GATEWAY_PROVIDER", Some("fcm")),
        (
            "PUSH_GATEWAY_PUBLIC_BASE_URL",
            Some("https://push.example.test"),
        ),
        ("PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS", Some(recipient)),
        ("PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG", Some("1000")),
        ("PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG", Some("20")),
        (
            "PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED",
            Some("false"),
        ),
    ];

    let credentials = service_account_json("prod-project");
    let mut env = base_env.to_vec();
    env.push(("FCM_SERVICE_ACCOUNT_JSON", Some(credentials.as_str())));
    env.push(("FCM_SEND_ENDPOINT_BASE", Some("http://127.0.0.1:9")));
    with_env(&env, || {
        let err = PushGatewayConfig::from_env().expect_err("production safety");
        assert!(matches!(err, PushGatewayConfigError::ProductionSafety(_)));
        assert!(err.to_string().contains("FCM_SEND_ENDPOINT_BASE"));
    });

    let credentials =
        service_account_json_with_token_uri("prod-project", "https://oauth2.example.test/token");
    let mut env = base_env.to_vec();
    env.push(("FCM_SERVICE_ACCOUNT_JSON", Some(credentials.as_str())));
    with_env(&env, || {
        let err = PushGatewayConfig::from_env().expect_err("production safety");
        assert!(matches!(err, PushGatewayConfigError::ProductionSafety(_)));
        assert!(err.to_string().contains("Google OAuth token URI"));
    });
}

#[test]
fn production_mode_can_be_configured_from_cli_flags() {
    let credentials = service_account_json("prod-cli-project");
    let recipient = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let args = PushGatewayArgs::try_parse_from([
        "push-gateway",
        "--production-mode",
        "--provider=fcm",
        "--fcm-service-account-json",
        &credentials,
        "--public-base-url=https://push.example.test",
        "--admission-allowed-recipients",
        recipient,
        "--max-global-outbox-backlog=1000",
        "--max-recipient-outbox-backlog=20",
        "--legacy-notification-hook-enabled=false",
    ])
    .expect("args");
    let config = args.into_config().expect("production config");
    assert!(config.production_mode());
    assert!(!config.legacy_notification_hook_enabled());
    assert!(config.recipient_admitted(&crate::RecipientId(recipient.to_owned())));
}

#[test]
fn retention_days_defaults_to_seven_days_and_is_configurable() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(&[], || {
        let config = PushGatewayConfig::from_env().expect("default config");
        assert_eq!(config.retention_seconds(), 7 * 86_400);
        assert_eq!(config.registration_ttl_seconds(), 30 * 86_400);
    });

    with_env(&[("PUSH_GATEWAY_RETENTION_DAYS", Some("3"))], || {
        let config = PushGatewayConfig::from_env().expect("env config");
        assert_eq!(config.retention_seconds(), 3 * 86_400);
    });

    let args =
        PushGatewayArgs::try_parse_from(["push-gateway", "--retention-days=14"]).expect("args");
    let config = args.into_config().expect("cli config");
    assert_eq!(config.retention_seconds(), 14 * 86_400);

    let args = PushGatewayArgs::try_parse_from(["push-gateway", "--registration-ttl-days=45"])
        .expect("args");
    let config = args.into_config().expect("cli config");
    assert_eq!(config.registration_ttl_seconds(), 45 * 86_400);
}

#[test]
fn admission_allowlist_rejects_malformed_recipients() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(
        &[
            (
                "PUSH_GATEWAY_PUBLIC_BASE_URL",
                Some("https://push.example.test"),
            ),
            ("PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS", Some("npub123")),
        ],
        || {
            let err = PushGatewayConfig::from_env().expect_err("invalid recipient");
            assert!(matches!(
                err,
                PushGatewayConfigError::InvalidAdmissionAllowedRecipients(_)
            ));
        },
    );
}

#[test]
fn telemetry_configuration_is_all_or_nothing_and_redacts_bearers() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    with_env(
        &[("PUSH_GATEWAY_TELEMETRY_ENCRYPTION_KEY", Some("11"))],
        || {
            assert!(matches!(
                PushGatewayConfig::from_env().unwrap_err(),
                PushGatewayConfigError::InvalidTelemetryConfiguration
            ));
        },
    );
    with_env(
        &[
            (
                "PUSH_GATEWAY_TELEMETRY_MANIFOLD_ENVIRONMENT",
                Some("development"),
            ),
            (
                "PUSH_GATEWAY_TELEMETRY_ENCRYPTION_KEY",
                Some("1111111111111111111111111111111111111111111111111111111111111111"),
            ),
        ],
        || {
            let config = PushGatewayConfig::from_env().unwrap();
            let telemetry = config.telemetry().unwrap();
            assert_eq!(telemetry.environment(), ManifoldEnvironment::Development);
            let debug = format!("{config:?}");
            assert!(!debug.contains(&"11".repeat(32)));
        },
    );
}

#[test]
fn telemetry_rejects_bad_encryption_keys() {
    for key in [&"11".repeat(31), "not-hex"] {
        assert!(matches!(
            TelemetryReceiverConfig::new(ManifoldEnvironment::Development, key),
            Err(PushGatewayConfigError::InvalidTelemetryConfiguration)
        ));
    }
}

#[test]
fn production_telemetry_requires_a_protected_operator_surface() {
    let credentials =
        FirebaseCredentials::from_json(&service_account_json("prod-project")).unwrap();
    let telemetry =
        TelemetryReceiverConfig::new(ManifoldEnvironment::Development, &"11".repeat(32)).unwrap();
    let config =
        PushGatewayConfig::new(None, "postgres://push.example.test/push", Some(credentials))
            .try_with_public_base_url("https://push.example.test")
            .unwrap()
            .with_production_mode(true)
            .with_open_self_registration_enabled(true)
            .with_legacy_notification_hook_enabled(false)
            .with_telemetry_receiver(telemetry)
            .with_rate_limits(RateLimitConfig {
                max_global_outbox_backlog: 1_000,
                max_recipient_outbox_backlog: 20,
                ..RateLimitConfig::default()
            });

    assert!(matches!(
        validate_production_safety(&config),
        Err(PushGatewayConfigError::ProductionSafety(_))
    ));
    assert!(
        validate_production_safety(
            &config.with_operator_token(OperatorToken::new("operator-secret"))
        )
        .is_ok()
    );
}

fn service_account_json(project_id: &str) -> String {
    service_account_json_with_token_uri(project_id, "https://oauth2.googleapis.com/token")
}

fn service_account_json_with_token_uri(project_id: &str, token_uri: &str) -> String {
    serde_json::json!({
        "type": "service_account",
        "project_id": project_id,
        "client_email": "svc@example.test",
        "private_key": "dummy-private-key",
        "token_uri": token_uri
    })
    .to_string()
}

fn with_env(vars: &[(&str, Option<&str>)], test: impl FnOnce()) {
    const CLEARED: &[&str] = &[
        "PUSH_GATEWAY_PROVIDER",
        "FCM_SERVICE_ACCOUNT_FILE",
        "FCM_SERVICE_ACCOUNT_JSON",
        "FIREBASE_CREDENTIALS_JSON",
        "FCM_SEND_ENDPOINT_BASE",
        "FCM_MAX_CONCURRENCY",
        "PUSH_GATEWAY_APP_ID",
        "PUSH_GATEWAY_DATABASE_URL",
        "PUSH_GATEWAY_PUBLIC_BASE_URL",
        "PUSH_GATEWAY_BIND",
        "PUSH_GATEWAY_OPERATOR_BIND",
        "PUSH_GATEWAY_OPERATOR_TOKEN",
        "PUSH_GATEWAY_PUBLIC_METRICS_ENABLED",
        "PUSH_GATEWAY_PRODUCTION_MODE",
        "PUSH_GATEWAY_TELEMETRY_MANIFOLD_ENVIRONMENT",
        "PUSH_GATEWAY_TELEMETRY_ENCRYPTION_KEY",
        "PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED",
        "PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS",
        "PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED",
        "PUSH_GATEWAY_OUTBOX_WORKER_CONCURRENCY",
        "PUSH_GATEWAY_HOOK_INVOCATIONS_PER_SOURCE_PREFIX",
        "PUSH_GATEWAY_HOOK_INVOCATIONS_PER_HOOK",
        "PUSH_GATEWAY_HOOK_INVOCATION_WINDOW_SECONDS",
        "PUSH_GATEWAY_HOOK_CREATIONS_PER_RECIPIENT",
        "PUSH_GATEWAY_HOOK_CREATION_WINDOW_SECONDS",
        "PUSH_GATEWAY_REGISTRATION_CHANGES_PER_RECIPIENT_SOURCE",
        "PUSH_GATEWAY_REGISTRATION_CHANGES_PER_SOURCE_PREFIX",
        "PUSH_GATEWAY_REGISTRATION_CHANGE_WINDOW_SECONDS",
        "PUSH_GATEWAY_MAX_ACTIVE_HOOKS_PER_RECIPIENT",
        "PUSH_GATEWAY_MAX_ACTIVE_INSTALLATIONS_PER_RECIPIENT",
        "PUSH_GATEWAY_MAX_ACTIVE_HOOKS_GLOBAL",
        "PUSH_GATEWAY_MAX_ACTIVE_INSTALLATIONS_GLOBAL",
        "PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG",
        "PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG",
        "PUSH_GATEWAY_TRUSTED_PROXY_CIDRS",
        "PUSH_GATEWAY_ALLOW_INSECURE_PUBLIC_BASE_URL",
        "PUSH_GATEWAY_UNSAFE_ALLOW_ANY_APP_ID_FOR_TESTS",
        "PUSH_GATEWAY_REGISTRATION_TTL_DAYS",
    ];
    let saved = CLEARED
        .iter()
        .map(|name| (*name, std::env::var(name).ok()))
        .collect::<Vec<_>>();
    for name in CLEARED {
        unsafe { std::env::remove_var(name) };
    }
    for (name, value) in vars {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }
    test();
    for (name, value) in saved {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }
}
