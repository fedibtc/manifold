{
  description = "Decentralized Federations";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-utils.url = "github:numtide/flake-utils";
    flakebox = {
      url = "git+https://radicle.dpc.pw/z3fAWfQ8mDt5eVELtCN9iMFKr8cYb.git?rev=7214849891debfbcd7f2fcce9de16cad608c9377";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    dpc-public-skills = {
      url = "git+https://radicle.dpc.pw/z2HR882B4c4mTdAgdt4SozpdeTuMf.git";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    selfci = {
      url = "github:dpc/selfci";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
    credential-sdk-src = {
      url = "github:fedibtc/credential-sdk";
      flake = false;
    };
    fedimint.url = "github:fedibtc/fedimint/v0.11.1-fedi18";
    # SP-enabled fedimintd for the live stability-pool E2E. The stability-pool
    # server module lives only in the fedixyz/fedi monorepo; its `fedi-fedimintd`
    # package bundles it (enabled at runtime by FEDI_STABILITY_POOL_V2_MODULE_ENABLE).
    # `fedimint-pkgs` follows our `fedimint` input for Fedi's Nix dependencies.
    # `fedi-fedimintd` still builds its Rust graph from this Fedi revision's
    # Cargo.lock, so that graph is not necessarily identical to FLIP's pin.
    fedi = {
      url = "github:fedixyz/fedi/2f35ea4e3b2516d35b8ed315455718cd3b336758";
      inputs.fedimint-pkgs.follows = "fedimint";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      flakebox,
      dpc-public-skills,
      selfci,
      credential-sdk-src,
      fedimint,
      fedi,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        fedimintPkgs = import nixpkgs {
          inherit system;
          overlays = [ fedimint.overlays.all ];
        };
        projectName = "decentralized-federations";
        pushGatewayVersion = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
        selfciPkg = selfci.packages.${system}.default;
        selfciCheck = pkgs.writeShellApplication {
          name = "selfci-check";
          runtimeInputs = [
            selfciPkg
            pkgs.git
          ];
          text = ''
            exec selfci check "$@"
          '';
        };
        mq = pkgs.writeShellScriptBin "mq" ''
          exec ${selfciPkg}/bin/selfci mq "$@"
        '';
        projectSkillNames = [
          "linked-specs"
          "linked-specs-updating"
          "linked-specs-review"
          "linked-specs-claims"
          "linked-specs-claims-verification"
          "dpc-general-multipart-review"
          "dpc-review-coordination"
          "dpc-review-architecture"
          "dpc-review-judgment"
          "dpc-review-maintainability"
          "dpc-review-rust-style"
          "dpc-review-reliability"
        ];
        linkProjectSkills = pkgs.writeShellScriptBin "link-project-skills" ''
          set -eu

          root=$1
          skills_dir="$root/.agents/skills"
          source_dir="${dpc-public-skills.packages.${system}.skills}/share/agents/skills"
          ${pkgs.coreutils}/bin/mkdir -p "$skills_dir"

          remove_old_link() {
            target="$skills_dir/$1"
            if [ -L "$target" ]; then
              ${pkgs.coreutils}/bin/rm -f "$target"
            elif [ -e "$target" ]; then
              printf 'refusing to replace non-symlink: %s\n' "$target" >&2
              exit 1
            fi
          }

          remove_old_link dpc-public-skills
          remove_old_link maan2003-skills

          for name in ${pkgs.lib.escapeShellArgs projectSkillNames}; do
            target="$skills_dir/$name"
            if [ -e "$target" ] && [ ! -L "$target" ]; then
              printf 'refusing to replace non-symlink: %s\n' "$target" >&2
              exit 1
            fi

            tmp_dir="$(${pkgs.coreutils}/bin/mktemp -d "$skills_dir/.link-project-skills.XXXXXX")"
            trap '${pkgs.coreutils}/bin/rm -rf "$tmp_dir"' EXIT HUP INT TERM
            ${pkgs.coreutils}/bin/ln -s "$source_dir/$name" "$tmp_dir/$name"
            ${pkgs.coreutils}/bin/mv -Tf "$tmp_dir/$name" "$target"
            ${pkgs.coreutils}/bin/rmdir "$tmp_dir"
            trap - EXIT HUP INT TERM
          done

          if exclude_file="$(${pkgs.git}/bin/git -C "$root" rev-parse --path-format=absolute --git-path info/exclude 2>/dev/null)"; then
            ${pkgs.coreutils}/bin/mkdir -p "$(${pkgs.coreutils}/bin/dirname "$exclude_file")"
            ${pkgs.coreutils}/bin/touch "$exclude_file"

            tmp_file="$(${pkgs.coreutils}/bin/mktemp "$exclude_file.XXXXXX")"
            ${pkgs.gnugrep}/bin/grep -Fvx \
              -e '/.agents/skills/dpc-public-skills' \
              -e '/.agents/skills/maan2003-skills' \
              "$exclude_file" >"$tmp_file" || true
            ${pkgs.coreutils}/bin/mv -f "$tmp_file" "$exclude_file"

            for name in ${pkgs.lib.escapeShellArgs projectSkillNames}; do
              pattern="/.agents/skills/$name"
              if ! ${pkgs.gnugrep}/bin/grep -Fxq "$pattern" "$exclude_file"; then
                printf '%s\n' "$pattern" >>"$exclude_file"
              fi
            done
          fi
        '';
        # Cargo links this exact source, rather than a tracing-side filter, so
        # the upstream payment retry site cannot format payment authorization,
        # invoice metadata, gateway metadata, or remote error text. Keep the
        # patch adjacent to the pinned input and make every Cargo consumer use
        # the same patched source.
        fedimintPatched = pkgs.applyPatches {
          name = "fedimint-redacted-lightning-payment-logs";
          src = fedimint;
          patches = [ ./patches/fedimint-redact-lightning-payment-logs.patch ];
        };
        linkExternalDeps = pkgs.writeShellScriptBin "link-external-deps" ''
          set -eu

          root=$1
          parent="$root/.nix-deps"
          mkdir -p "$parent"

          link_dependency() {
            name=$1
            target=$2
            link="$parent/$name"

            if [ -e "$link" ] && [ ! -L "$link" ]; then
              echo "refusing to replace non-symlink: $link" >&2
              exit 1
            fi

            tmp_dir="$(${pkgs.coreutils}/bin/mktemp -d "$parent/.link-external-deps.XXXXXX")"
            trap '${pkgs.coreutils}/bin/rm -rf "$tmp_dir"' EXIT HUP INT TERM
            ln -s "$target" "$tmp_dir/$name"

            # Rename is atomic, so concurrent dev-shell entries converge on the
            # same link instead of racing through an unlink/create window.
            ${pkgs.coreutils}/bin/mv -Tf "$tmp_dir/$name" "$link"
            ${pkgs.coreutils}/bin/rmdir "$tmp_dir"
            trap - EXIT HUP INT TERM
          }

          link_dependency credential-sdk ${credential-sdk-src}
          link_dependency fedimint ${fedimintPatched}
        '';

        flakeboxLib = flakebox.lib.mkLib pkgs {
          config = {
            github.ci.buildOutputs = [ ".#ci.workspace" ];
            just.importPaths = [ "justfile.custom.just" ];
            just.rules.watch.enable = false;
            rootDir.".envrc".text = pkgs.lib.mkAfter ''
              source_env_if_exists .envrc.local
            '';
            # Replace the stock cargo-lock pre-commit check: it runs bare
            # `cargo update --locked`, which dies deep inside cargo when the
            # .nix-deps symlinks are missing (`git clean -fdx`, GC'd store
            # path, commit outside the dev shell). Relink them first, or fail
            # with instructions instead of a cargo path error.
            cargo.pre-commit.cargo-lock.enable = false;
            git.pre-commit.hooks.cargo_lock = ''
              # Cargo resolves fedimint/credential-sdk through .nix-deps
              # symlinks that the dev shell creates; (re)link them so this
              # check doesn't fail deep inside cargo when they are missing.
              if command -v link-external-deps >/dev/null 2>&1; then
                link-external-deps "$(git rev-parse --show-toplevel)"
              else
                for dep in $(grep -oE '"\.nix-deps/[A-Za-z0-9_-]+' Cargo.toml | cut -d'"' -f2 | sort -u); do
                  if [ ! -e "$dep" ]; then
                    >&2 echo "$dep is missing; enter the dev shell once ('nix develop') to create it."
                    return 1
                  fi
                done
              fi

              # https://users.rust-lang.org/t/check-if-the-cargo-lock-is-up-to-date-without-building-anything/91048/5
              flakebox-in-each-cargo-workspace cargo update --workspace --locked -q
            '';
            toolchain.components = [
              "rustc"
              "cargo"
              "clippy"
              "rust-analyzer"
              "rust-src"
              "rustfmt"
            ];
          };
        };

        buildPaths = [
          ".config/nextest.toml"
          "Cargo.toml"
          "Cargo.lock"
          "crates"
          # The cloud telemetry policy checks its reviewed source manifest from
          # Rust tests, so it must be present in the filtered Nix build source.
          "docs/telemetry/fedimint-metrics-v0.11.1-fedi18.tsv"
          # Same arrangement for the captured guardian response those tests
          # replay through the shipped policy. The manifest above records what
          # the pinned source registers; this records what a running producer
          # actually emits, which is the half a source scan cannot check.
          "docs/telemetry/fedimint-metrics-v0.11.1-fedi15-seat-scrape.txt"
          # The contract-fixture test in service-liquidity-manager re-serializes
          # each admin response and asserts it still equals the committed JSON
          # the TypeScript side consumes. That JSON has to be inside the build
          # source or the test cannot see it: it passes against a full checkout
          # and fails in the sandbox with "missing committed fixture".
          "operator-ui/packages/types/fixtures"
          # The same arrangement, one directory over: fman-core's
          # admin_request_ts test re-generates the TypeScript AdminRequest union
          # and asserts it still equals the committed file.
          "operator-ui/packages/types/src/generated"
        ];
        markdownPackageMetadata = map toString [
          ./crates/defe-api/README.md
          ./crates/defe-portalloc/README.md
          ./crates/operator-ui-auth/README.md
          ./crates/tests-e2e/README.md
        ];

        buildSrc = flakeboxLib.source.fromPaths {
          root = ./.;
          paths = buildPaths;
          filter =
            path: type:
            let
              baseName = builtins.baseNameOf path;
            in
            (type != "directory" || baseName != "specs")
            && (
              type != "regular"
              || !pkgs.lib.hasSuffix ".md" baseName
              || builtins.elem (toString path) markdownPackageMetadata
            );
        };

        multiBuild = (flakeboxLib.craneMultiBuild { }) (
          craneLib':
          let
            craneLib = craneLib'.overrideArgs {
              pname = projectName;
              src = buildSrc;
              # aws-lc-sys (via the fedimint client stack) builds with cmake;
              # some existing crates also discover native libs through pkg-config.
              nativeBuildInputs = [
                pkgs.cmake
                pkgs.pkg-config
              ];
              dontUseCmakeConfigure = true;
              env.RUSTDOCFLAGS = "-D warnings";
              # jemalloc bakes in its page size; 64KiB works everywhere. Same
              # value fedimint sets.
              env.JEMALLOC_SYS_WITH_LG_PAGE = "16";
              postPatch = "${linkExternalDeps}/bin/link-external-deps .";
            };
          in
          rec {
            workspaceDeps = craneLib.buildWorkspaceDepsOnly { };

            workspace = craneLib.buildWorkspace {
              cargoArtifacts = workspaceDeps;
            };

            defe = craneLib.buildPackage {
              cargoArtifacts = workspaceDeps;
              cargoExtraArgs = "-p defe";
            };

            pushGateway = craneLib.buildPackage {
              cargoArtifacts = workspaceDeps;
              cargoExtraArgs = "-p fedi-decentralized-push-gateway";
            };

            cloudFmanTelemetry = craneLib.buildPackage {
              cargoArtifacts = workspaceDeps;
              cargoExtraArgs = "-p fedi-decentralized-cloud-fman-telemetry";
            };

            liquidityManagerDaemon = craneLib.buildPackage {
              cargoArtifacts = workspaceDeps;
              cargoExtraArgs = "-p fedi-decentralized-liquidity-manager-daemon --features embedded-operator-ui";
              env.FLIP_OPERATOR_UI_DIST_DIR = "${operatorUi}/srv/flip";
            };

            fleetManager = craneLib.buildPackage {
              cargoArtifacts = workspaceDeps;
              # Shipped daemon builds consume their Vite output. Workspace,
              # test, and developer builds stay UI-free.
              cargoExtraArgs = "-p fman --bin fleet-manager -p fman-cli --bin fman-cli --features fman/embedded-operator-ui";
              env.FMAN_OPERATOR_UI_DIST_DIR = "${operatorUi}/srv/fman";
              # The bundled fedimintd reports this value in `app_start_ts`.
              # Keep it tied to the reviewed Fedimint input, not the Manifold
              # checkout or a source snapshot's missing `.git` fallback.
              env.FEDIMINT_BUILD_FORCE_GIT_HASH = fedimintSourceRev;
            };

            setupPaymentPublisher = craneLib.buildPackage {
              cargoArtifacts = workspaceDeps;
              cargoExtraArgs = "-p setup-payment-publisher";
            };

            testArtifacts = craneLib.mkCargoDerivation {
              pname = "${projectName}-nextest-artifacts";
              cargoArtifacts = workspaceDeps;
              doCheck = false;
              nativeBuildInputs = [
                pkgs.cmake
                pkgs.cargo-nextest
                pkgs.pkg-config
              ];
              buildPhaseCargoCommand = ''
                cargo nextest run --no-run \
                  ''${CARGO_PROFILE:+--cargo-profile $CARGO_PROFILE} \
                  --features fedi-decentralized-cloud-fman-telemetry/defe-test-support \
                  --workspace
              '';
            };

            runtimeBins = craneLib.buildPackage {
              cargoArtifacts = testArtifacts;
              # FMan and FLIP are the only published binaries whose release
              # feature set embeds the dashboards. Enable those features on the
              # runtime bundle SelfCI already builds instead of adding another
              # Cargo derivation or profile solely for the OCI checks.
              cargoExtraArgs = "-p defe --bin defe -p devmon --bin manifold-test-issuer -p fedi-decentralized-cloud-fman-telemetry --bin fedi-decentralized-cloud-fman-telemetry -p fedi-decentralized-push-gateway --bin fedi-decentralized-push-gateway -p fedi-decentralized-liquidity-manager-daemon --bin liquidity-manager-daemon -p fman --bin fleet-manager -p fman-cli --bin fman-cli -p fi-cli --bin fi-cli --features fedi-decentralized-liquidity-manager-daemon/embedded-operator-ui,fman/embedded-operator-ui";
              env.FLIP_OPERATOR_UI_DIST_DIR = "${operatorUi}/srv/flip";
              env.FMAN_OPERATOR_UI_DIST_DIR = "${operatorUi}/srv/fman";
              env.FEDIMINT_BUILD_FORCE_GIT_HASH = fedimintSourceRev;
            };

            tests =
              let

                # Keep every workspace runner on the artifact feature set.
                # A mismatch makes Cargo recompile inside each isolated runner.
                workspaceTestArgs = "--workspace --features fedi-decentralized-cloud-fman-telemetry/defe-test-support";
                # Linux Nix sandboxes isolate each runner's loopback network.
                # Darwin runners share host loopback while keeping independent
                # $TMPDIR-backed defe-portalloc ledgers, so concurrent runners
                # can reserve the same ports before their services bind. Making
                # each Darwin runner consume its predecessor serializes only the
                # platform that lacks network namespaces.
                darwinRunnerDependency =
                  previousRunner:
                  pkgs.lib.optionalString (pkgs.stdenv.isDarwin && previousRunner != null) ''
                    test -e ${previousRunner}
                  '';

                runTests =
                  name: artifacts: bins: cargoArgs: filter: previousRunner:
                  craneLib.mkCargoDerivation {
                    # Darwin includes the derivation name in its build TMPDIR;
                    # keep it short enough for nested Unix-domain sockets.
                    pname = if pkgs.stdenv.isDarwin then "df-nextest-${name}" else "${projectName}-nextest-${name}";
                    cargoArtifacts = artifacts;
                    doCheck = true;
                    doInstallCargoArtifacts = false;
                    env = {
                      NEXTEST_SHOW_PROGRESS = "none";
                      NEXTEST_STATUS_LEVEL = "none";
                    };
                    nativeBuildInputs = [
                      pkgs.cargo-nextest
                      pkgs.nostr-rs-relay
                    ];
                    buildPhaseCargoCommand = "mkdir -p $out";
                    checkPhaseCargoCommand = ''
                      ${darwinRunnerDependency previousRunner}
                      export DEV_DEFE_PORTALLOC_DATA_DIR="$TMPDIR/defe-portalloc"
                      export FI_CLI_TEST_BIN=${bins}/bin/fi-cli
                      export FMAN_E2E=1
                      export FMAN_E2E_FLEET_MANAGER_BIN=${bins}/bin/fleet-manager
                      export FMAN_E2E_FMAN_CLI_BIN=${bins}/bin/fman-cli
                      export FMAN_E2E_FI_CLI_BIN=${bins}/bin/fi-cli
                      export FMAN_E2E_FEDIMINT_CLI_BIN=${fedimint.packages.${system}.fedimint-cli}/bin/fedimint-cli
                      export FMAN_E2E_BITCOIN_CLI_BIN=${pkgs.bitcoind}/bin/bitcoin-cli
                      export FMAN_E2E_ESPLORA_BIN=${fedimintPkgs.esplora-electrs}/bin/esplora
                      export FLIP_E2E_GATEWAYD_BIN=${fedimint.packages.${system}.gatewayd}/bin/gatewayd
                      export FLIP_E2E_GATEWAY_CLI_BIN=${fedimint.packages.${system}.gateway-cli}/bin/gateway-cli
                      export FLIP_E2E_FEDIMINTD_BIN=${fedimint.packages.${system}.fedimintd}/bin/fedimintd
                      export FLIP_E2E_FEDIMINT_CLI_BIN=${fedimint.packages.${system}.fedimint-cli}/bin/fedimint-cli
                      export FLIP_E2E_SP_FEDIMINTD_BIN=${fedi.packages.${system}.fedi-fedimintd}/bin/fedimintd
                      export FLIP_E2E_ESPLORA_BIN=${fedimintPkgs.esplora-electrs}/bin/esplora
                      export FLIP_E2E_BITCOIN_CLI_BIN=${pkgs.bitcoind}/bin/bitcoin-cli
                      export FLIP_E2E_LIQUIDITY_MANAGER_DAEMON_BIN=${bins}/bin/liquidity-manager-daemon
                      mkdir -p "$DEV_DEFE_PORTALLOC_DATA_DIR"
                      ${bins}/bin/defe \
                        --binary-path ${bins}/bin \
                        --nostr-rs-relay-bin ${pkgs.nostr-rs-relay}/bin/nostr-rs-relay \
                        --bitcoind-bin ${pkgs.bitcoind}/bin/bitcoind \
                        --gatewayd-bin ${fedimint.packages.${system}.gatewayd}/bin/gatewayd \
                        --gateway-cli-bin ${fedimint.packages.${system}.gateway-cli}/bin/gateway-cli \
                        exec cargo nextest run \
                        ''${CARGO_PROFILE:+--cargo-profile $CARGO_PROFILE} \
                        ${cargoArgs} -E ${pkgs.lib.escapeShellArg filter}
                    '';
                  };

                ordinaryRunner =
                  runTests "ordinary" testArtifacts runtimeBins workspaceTestArgs
                    "all() - binary(fleet_manager_0_1_formation) - binary(integration_live_liquidity) - binary(integration_flip_operator_e2e)"
                    null;
                fmanFreeRunner =
                  runTests "fman-free" testArtifacts runtimeBins workspaceTestArgs
                    "binary(fleet_manager_0_1_formation) & test(fleet_manager_0_1_forms_seven_guardian_federation_under_defe)"
                    ordinaryRunner;
                fmanRestoreRunner =
                  runTests "fman-restore" testArtifacts runtimeBins workspaceTestArgs
                    "binary(fleet_manager_0_1_formation) & test(fman_restores_a_formed_fleet_from_mnemonic_and_nostr_under_defe)"
                    fmanFreeRunner;
                fiCrashRecoveryRunner =
                  runTests "fi-crash-recovery" testArtifacts runtimeBins workspaceTestArgs
                    "binary(fleet_manager_0_1_formation) & test(fi_client_resumes_real_dkg_after_sigkill_under_defe)"
                    fmanRestoreRunner;
                fmanSeatLifecycleRunner =
                  runTests "fman-seat-lifecycle" testArtifacts runtimeBins workspaceTestArgs
                    "binary(fleet_manager_0_1_formation) & test(fman_recovers_a_real_child_and_terminalizes_data_loss_under_defe)"
                    fiCrashRecoveryRunner;
                fmanPostFormationRunner =
                  runTests "fman-post-formation" testArtifacts runtimeBins workspaceTestArgs
                    "binary(fleet_manager_0_1_formation) & test(fman_remits_collects_and_recovers_guardian_fee_payout_under_defe)"
                    fmanSeatLifecycleRunner;
                fmanPaidRunner =
                  runTests "fman-paid" testArtifacts runtimeBins workspaceTestArgs
                    "binary(fleet_manager_0_1_formation) & test(fleet_manager_0_1_paid_formation_settles_real_ecash_under_defe)"
                    fmanPostFormationRunner;
                fmanMultiRelayRunner =
                  runTests "fman-multi-relay" testArtifacts runtimeBins workspaceTestArgs
                    "binary(fleet_manager_0_1_formation) & test(fman_advertises_and_onboards_with_first_relay_down_under_defe)"
                    fmanPaidRunner;
                flipHappyRunner =
                  runTests "flip-happy" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_liquidity_happy_path_publishes_nostr_and_allocates_over_iroh)"
                    fmanMultiRelayRunner;
                flipDepositRunner =
                  runTests "flip-deposit" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_deposit_monitoring_restart_claims_funded_top_up)"
                    flipHappyRunner;
                flipWithdrawalRunner =
                  runTests "flip-withdrawal" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_gateway_withdrawal_restart_resumes_without_duplicate_funding)"
                    flipDepositRunner;
                flipPegInRunner =
                  runTests "flip-peg-in" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_target_peg_in_restart_completes_after_wallet_operation_finality)"
                    flipWithdrawalRunner;
                flipStabilityRunner =
                  runTests "flip-stability" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_stability_pool_happy_path_provides_into_pool)"
                    flipPegInRunner;
                flipRestoreCredentialRunner =
                  runTests "flip-restore-credential" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_flip_operator_e2e) & test(live_restore_reinstates_the_archived_admin_credential)"
                    flipStabilityRunner;

                flipOperatorRemediationRunner =
                  runTests "flip-operator-remediation" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_flip_operator_e2e) & test(operator_remediation_preserves_send_once_guards_and_releases_abandoned_capacity)"
                    flipRestoreCredentialRunner;
                flipRestoreRollbackRunner =
                  runTests "flip-restore-rollback" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_restore_rejects_allocation_rollback_and_replay_stays_idempotent)"
                    flipOperatorRemediationRunner;
                flipTrustRejectRunner =
                  runTests "flip-trust-reject" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_trust_reject_matrix_maps_each_failure_to_its_code)"
                    flipRestoreRollbackRunner;
                flipStabilityRecoveryRunner =
                  runTests "flip-stability-recovery" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_stability_pre_submit_restart_adopts_the_deposit_already_made)"
                    flipTrustRejectRunner;
                flipWithdrawalIntentRunner =
                  runTests "flip-withdrawal-intent" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_operator_withdrawal_intent_replays_once_and_rejects_conflicts)"
                    flipStabilityRecoveryRunner;
                flipCombinedReplayRunner =
                  runTests "flip-combined-replay" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_combined_request_concurrent_replay_creates_one_allocation)"
                    flipWithdrawalIntentRunner;
                flipCapacityTopUpRunner =
                  runTests "flip-capacity-top-up" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_top_up_admits_the_request_capacity_first_refused)"
                    flipCombinedReplayRunner;
                flipDependencyOutageRunner =
                  runTests "flip-dependency-outage" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_dependency_outage_withdraws_the_advertisement_until_the_operator_republishes)"
                    flipCapacityTopUpRunner;
                flipTwoFederationsRunner =
                  runTests "flip-two-federations" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_second_federation_is_funded_after_the_operator_tops_up)"
                    flipDependencyOutageRunner;
                flipManualReviewRunner =
                  runTests "flip-manual-review" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_unresolvable_send_escalates_to_review_and_waits_for_the_operator)"
                    flipTwoFederationsRunner;
                flipCancelCapacityRunner =
                  runTests "flip-cancel-capacity" testArtifacts runtimeBins workspaceTestArgs
                    "binary(integration_live_liquidity) & test(live_cancelling_a_wedged_allocation_frees_capacity_for_the_next_federation)"
                    flipManualReviewRunner;

                runners = [
                  ordinaryRunner
                  fmanFreeRunner
                  fmanRestoreRunner
                  fiCrashRecoveryRunner
                  fmanSeatLifecycleRunner
                  fmanPostFormationRunner
                  fmanPaidRunner
                  fmanMultiRelayRunner
                  flipHappyRunner
                  flipDepositRunner
                  flipWithdrawalRunner
                  flipPegInRunner
                  flipStabilityRunner
                  flipRestoreCredentialRunner
                  flipOperatorRemediationRunner
                  flipRestoreRollbackRunner
                  flipTrustRejectRunner
                  flipStabilityRecoveryRunner
                  flipWithdrawalIntentRunner
                  flipCombinedReplayRunner
                  flipCapacityTopUpRunner
                  flipDependencyOutageRunner
                  flipTwoFederationsRunner
                  flipManualReviewRunner
                  flipCancelCapacityRunner
                ];
              in
              pkgs.runCommand "${projectName}-nextest-${craneLib.cargoProfile}" { } ''
                for result in ${pkgs.lib.concatMapStringsSep " " toString runners}; do
                  test -e "$result"
                done
                touch "$out"
              '';

            clippy = craneLib.cargoClippy {
              cargoArtifacts = workspaceDeps;
              doInstallCargoArtifacts = false;
            };

            cargoFmt = craneLib.cargoFmt { };

            # Anti-drift for the Fedimint dependency declarations. The pinned
            # Fedi release is named once (the `fedimint` flake input); workspace
            # crates request Fedimint through source-neutral version
            # requirements, and `[patch]` routes them to the Nix-provided
            # `${fedimintPatched}` source (linked as `.nix-deps/fedimint`). This
            # check fences that arrangement so it cannot silently regress:
            #   1. No Cargo manifest reintroduces a Fedimint git tag/revision.
            #   2. Every Fedimint monorepo crate resolves from the pinned Nix
            #      source. This is checked against the *resolved* graph via
            #      `cargo metadata`: each such package's canonicalized
            #      `manifest_path` must live under `${fedimintPatched}`. A bare
            #      "no `source` field in Cargo.lock" test is insufficient —
            #      Cargo omits `source` for every path dependency, so a `[patch]`
            #      redirected to a different local checkout would still pass.
            #   3. Iroh 0.90 and 0.96 stay absent from the resolved graph.
            # Runs as a crane derivation so `cargo metadata` sees the vendored
            # dependency graph and the `link-external-deps` postPatch symlink.
            cargoDependencyHygiene = craneLib.mkCargoDerivation {
              pname = "${projectName}-dependency-hygiene";
              cargoArtifacts = null;
              doInstallCargoArtifacts = false;
              nativeBuildInputs = [
                pkgs.gnutar
                pkgs.gzip
                pkgs.jq
                pkgs.gawk
              ];
              buildPhaseCargoCommand = ''
                set -eu
                mkdir -p "$out"

                # Canonical root of the pinned Nix-provided Fedimint source.
                fedimint_root=$(realpath ${fedimintPatched})

                # 1. No Fedimint git tags/revisions in Cargo manifests. A git
                #    dependency carries a `git =` key; the
                #    `[patch."...fedimint"]` table header has none and maps
                #    crate names to `.nix-deps` paths, so it is not matched.
                if grep -HnE 'git[[:space:]]*=[[:space:]]*"[^"]*github\.com/fedibtc/fedimint' \
                     Cargo.toml $(find crates -name Cargo.toml); then
                  echo "error: Fedimint git dependency found in a Cargo manifest above." >&2
                  echo "Use a source-neutral version requirement (routed to" >&2
                  echo "${fedimintPatched} via [patch]) instead of a git tag/revision." >&2
                  exit 1
                fi

                # Authoritative set of Fedimint *monorepo* crates: every package
                # defined in the pinned source tree. Intra-tree path deps such
                # as `fedimint-derive` / `fedimint-lnv2-common` are included;
                # crates.io crates that merely share the prefix but are absent
                # from the tree (e.g. `fedimint-threshold-crypto`) are excluded.
                monorepo=$(find ${fedimintPatched} -name Cargo.toml -exec awk '
                  /^\[package\]/ { inpkg = 1; next }
                  /^\[/ { inpkg = 0 }
                  inpkg && /^name[[:space:]]*=/ {
                    s = $0; sub(/^[^"]*"/, "", s); sub(/".*/, "", s); print s
                  }
                ' {} + | sort -u | tr '\n' ' ')

                # Print every monorepo crate in a `cargo metadata` document whose
                # canonicalized manifest_path is NOT under the pinned source
                # root. `realpath -m` resolves the `.nix-deps/fedimint` symlink
                # and tolerates non-existent paths (used by the self-test).
                offenders_in() {
                  jq -r '.packages[] | "\(.name)\t\(.manifest_path)"' "$1" \
                  | while IFS="$(printf '\t')" read -r name path; do
                      case " $monorepo " in
                        *" $name "*) ;;
                        *) continue ;;
                      esac
                      canon=$(realpath -m "$path")
                      case "$canon" in
                        "$fedimint_root"/*) ;;
                        *) echo "$name -> $canon" ;;
                      esac
                    done
                }

                # 2. Validate the resolved origin of every Fedimint monorepo
                #    crate directly from `cargo metadata`.
                cargo metadata --frozen --format-version 1 > "$TMPDIR/metadata.json"
                offenders=$(offenders_in "$TMPDIR/metadata.json")
                if [ -n "$offenders" ]; then
                  echo "error: Fedimint monorepo crates not resolved from the pinned" >&2
                  echo "Nix source ($fedimint_root):" >&2
                  echo "$offenders" >&2
                  exit 1
                fi

                # Negative self-test: the same validator must reject a monorepo
                # crate resolved from an alternate local checkout.
                printf '{"packages":[{"name":"fedimint-core","manifest_path":"/tmp/alt-fedimint/fedimint-core/Cargo.toml"}]}\n' \
                  > "$TMPDIR/alt.json"
                if [ -z "$(offenders_in "$TMPDIR/alt.json")" ]; then
                  echo "error: origin validator self-test failed — an alternate-path" >&2
                  echo "fedimint-core was not rejected." >&2
                  exit 1
                fi

                # 3. Iroh 0.90 and 0.96 must stay absent from the resolved graph.
                bad_iroh=$(awk '
                  /^\[\[package\]\]/ { name = ""; version = "" }
                  /^name = "/ { name = $3; gsub(/"/, "", name) }
                  /^version = "/ {
                    version = $3; gsub(/"/, "", version)
                    if (name ~ /^iroh($|-)/ && (version ~ /^0\.90(\.|$)/ || version ~ /^0\.96(\.|$)/))
                      print name " " version
                  }
                ' Cargo.lock)
                if [ -n "$bad_iroh" ]; then
                  echo "error: forbidden iroh version present in Cargo.lock:" >&2
                  echo "$bad_iroh" >&2
                  exit 1
                fi
              '';
            };

            ${projectName} = craneLib.buildPackage {
              cargoArtifacts = workspaceDeps;
            };
          }
        );

        treefmt =
          pkgs.runCommand "treefmt-check"
            {
              nativeBuildInputs = [
                pkgs.treefmt
                pkgs.nixfmt-rfc-style
                pkgs.rustfmt
                pkgs.taplo
              ];
              src = self;
            }
            ''
              cp -r $src work && chmod -R u+w work
              cd work
              treefmt --ci
              touch $out
            '';

        # Published to a registry alongside the Fleet Manager image, so it carries
        # the same versioned tag and OCI provenance labels the other two images
        # do rather than a `latest` that never moves.
        #
        # Parameterized over the package supplying the daemon binary. Publishing
        # supplies the release build; the required runtime-contract check uses
        # the UI-enabled CI-profile build so it exercises the embedded dashboard
        # without compiling a second Cargo profile.
        mkLiquidityManagerDockerImage =
          liquidityManagerDaemon:
          pkgs.dockerTools.buildLayeredImage {
            name = "flip-liquidity-manager";
            tag = pushGatewayVersion;
            contents = [
              liquidityManagerDaemon
              pkgs.cacert
              pkgs.curl
            ];
            config = {
              Entrypoint = [
                "${liquidityManagerDaemon}/bin/liquidity-manager-daemon"
              ];
              Cmd = [
                "run"
                "daemon"
              ];
              Env = [
                "FLIP_DATA_DIR=/var/lib/flip"
                "FLIP_ADMIN_BIND_ADDRESS=0.0.0.0:8173"
                "FLIP_PUBLIC_BIND_ADDRESS=0.0.0.0:8174"
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              ];
              ExposedPorts = {
                "8173/tcp" = { };
                # The public Liquidity API is Iroh over QUIC: a UDP socket
                # (`public_rpc.rs` binds an Iroh endpoint, not an HTTP
                # listener). Deployments that publish it must map UDP; a TCP
                # mapping exposes nothing, silently downgrading the endpoint
                # to relay-only inbound connectivity.
                "8174/udp" = { };
              };
              Volumes = {
                "/var/lib/flip" = { };
              };
              Healthcheck = {
                Test = [
                  "CMD"
                  "${pkgs.curl}/bin/curl"
                  "-fsS"
                  "http://127.0.0.1:8173/health"
                ];
                Interval = 5000000000;
                Timeout = 2000000000;
                Retries = 12;
                StartPeriod = 5000000000;
              };
              WorkingDir = "/var/lib/flip";
              Labels = {
                "org.opencontainers.image.title" = "flip-liquidity-manager";
                "org.opencontainers.image.description" =
                  "Federation Liquidity Provisioner daemon for decentralized federations";
                "org.opencontainers.image.version" = pushGatewayVersion;
                "org.opencontainers.image.source" = "https://github.com/fedibtc/manifold";
                "org.opencontainers.image.licenses" = "MIT";
              };
            };
          };
        liquidityManagerDockerImage = mkLiquidityManagerDockerImage multiBuild.liquidityManagerDaemon;
        liquidityManagerCiDockerImage = mkLiquidityManagerDockerImage ciRuntimeBins;

        # The collector is cloud infrastructure rather than an edge package. Its
        # image is published to ECR, but unlike the Umbrel-consumed images it has
        # no GHCR mirror.
        mkCloudFmanTelemetryContainerImage =
          cloudFmanTelemetry:
          pkgs.dockerTools.buildLayeredImage {
            name = "cloud-fman-telemetry";
            tag = pushGatewayVersion;
            contents = [
              cloudFmanTelemetry
              pkgs.cacert
              pkgs.curl
            ];
            # Preserve the service identity on an image-local path and on Docker
            # named-volume initialization. Kubernetes and host-path mounts replace
            # this inode and need UID/GID 10001 ownership and exact mode 0700 before
            # startup; fsGroup alone does not establish that contract.
            fakeRootCommands = ''
              mkdir -p var/lib/cloud-fman-telemetry
              chmod 0700 var/lib/cloud-fman-telemetry
              chown 10001:10001 var/lib/cloud-fman-telemetry
            '';
            config = {
              Entrypoint = [
                "${cloudFmanTelemetry}/bin/fedi-decentralized-cloud-fman-telemetry"
              ];
              Env = [
                "CLOUD_FMAN_TELEMETRY_PUBLIC_BIND=0.0.0.0:8175"
                "CLOUD_FMAN_TELEMETRY_PRIVATE_BIND=0.0.0.0:8176"
                "CLOUD_FMAN_TELEMETRY_DATA_DIR=/var/lib/cloud-fman-telemetry"
                "CLOUD_FMAN_TELEMETRY_KEY_FILE=/run/secrets/cloud-fman-telemetry-key"
                "CLOUD_FMAN_TELEMETRY_METRICS_POLL_SECONDS=1800"
                "CLOUD_FMAN_TELEMETRY_LOG_POLL_SECONDS=300"
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              ];
              User = "10001:10001";
              ExposedPorts = {
                "8175/tcp" = { };
                "8176/tcp" = { };
              };
              Volumes = {
                "/var/lib/cloud-fman-telemetry" = { };
              };
              Healthcheck = {
                Test = [
                  "CMD"
                  "${pkgs.curl}/bin/curl"
                  "-fsS"
                  "http://127.0.0.1:8176/health"
                ];
                Interval = 10000000000;
                Timeout = 2000000000;
                Retries = 6;
                StartPeriod = 5000000000;
              };
              WorkingDir = "/var/lib/cloud-fman-telemetry";
              Labels = {
                "org.opencontainers.image.title" = "cloud-fman-telemetry";
                "org.opencontainers.image.description" =
                  "Private cloud collector for registered Fleet Manager telemetry";
                "org.opencontainers.image.version" = pushGatewayVersion;
                "org.opencontainers.image.source" = "https://github.com/fedibtc/manifold";
                "org.opencontainers.image.licenses" = "MIT";
              };
            };
          };
        cloudFmanTelemetryContainerImage = mkCloudFmanTelemetryContainerImage multiBuild.cloudFmanTelemetry;
        cloudFmanTelemetryCiContainerImage = mkCloudFmanTelemetryContainerImage ciRuntimeBins;

        cloudFmanTelemetryContainerImageCheck =
          pkgs.runCommand "cloud-fman-telemetry-oci-image-check"
            {
              nativeBuildInputs = [
                pkgs.jq
                pkgs.python3
                pkgs.skopeo
                pkgs.umoci
              ];
            }
            ''
               tmp=$(mktemp -d)
              skopeo --insecure-policy --tmpdir "$tmp" copy \
                docker-archive:${cloudFmanTelemetryContainerImage} \
                oci:"$tmp/image":checked >/dev/null
              umoci raw unpack --rootless \
                --image "$tmp/image:checked" "$tmp/rootfs"

              mkdir "$tmp/archive"
              tar -xzf ${cloudFmanTelemetryContainerImage} -C "$tmp/archive"
              config_path="$tmp/archive/$(jq -r '.[0].Config' "$tmp/archive/manifest.json")"

              manifest_digest=$(
                jq -r '.manifests[0].digest | sub("^sha256:"; "")' \
                  "$tmp/image/index.json"
              )
              manifest_path="$tmp/image/blobs/sha256/$manifest_digest"
              entrypoint=$(jq -r '.config.Entrypoint[0]' "$config_path")
              rootfs="$tmp/rootfs"

              require_world_search() {
                requested=$1
                resolved=$(realpath --canonicalize-existing \
                  "$rootfs/''${requested#/}") \
                  || { echo "configured path absent from final rootfs: $requested" >&2; exit 1; }
                case "$resolved" in
                  "$rootfs" | "$rootfs"/*) ;;
                  *) echo "configured path escapes final rootfs: $requested" >&2; exit 1 ;;
                esac
                test "$(stat -c %F "$resolved")" = directory \
                  || { echo "configured search path is not a directory: $requested" >&2; exit 1; }

                current=$rootfs
                relative="''${resolved#"$rootfs"}"
                relative="''${relative#/}"
                IFS=/ read -r -a components <<<"$relative"
                for component in "" "''${components[@]}"; do
                  if [ -n "$component" ]; then
                    current="$current/$component"
                  fi
                  mode=$(stat -c %a "$current")
                  other="''${mode: -1}"
                  (( (other & 1) == 1 )) \
                    || { echo "UID 10001 cannot search $requested" >&2; exit 1; }
                done
              }

              # Rootless extraction maps image root to the build user. Requiring
              # world search/execute bits proves UID/GID 10001 can traverse and
              # execute these paths without trusting the mapped host ownership.
              require_runtime_executable() {
                requested=$1
                resolved=$(realpath --canonicalize-existing \
                  "$rootfs/''${requested#/}") \
                  || { echo "configured executable absent from final rootfs: $requested" >&2; exit 1; }
                case "$resolved" in
                  "$rootfs"/*) ;;
                  *) echo "configured executable escapes final rootfs: $requested" >&2; exit 1 ;;
                esac
                test "$(stat -c %F "$resolved")" = "regular file" \
                  || { echo "configured executable is not a regular file: $requested" >&2; exit 1; }
                require_world_search "/$(dirname "''${resolved#"$rootfs/"}")"
                mode=$(stat -c %a "$resolved")
                other="''${mode: -1}"
                (( (other & 1) == 1 )) \
                  || { echo "UID 10001 cannot execute $requested" >&2; exit 1; }
              }

              require_runtime_executable "$entrypoint"
              healthcheck_executable=$(jq -r '.config.Healthcheck.Test[1]' "$config_path")
              require_runtime_executable "$healthcheck_executable"
              working_dir=$(jq -r '.config.WorkingDir' "$config_path")
              require_world_search "$(dirname "$working_dir")"

              data_dir="$rootfs/var/lib/cloud-fman-telemetry"
              test "$(stat -c '%F:%a' "$data_dir")" = directory:700 \
                || { echo "final data directory is not a mode-0700 directory" >&2; exit 1; }

              # Rootless filesystems cannot represent every container UID. The
              # image constructor writes the data directory in its final layer,
              # so inspect that layer's one canonical entry for authoritative
              # ownership while umoci checks its final merged type and mode.
              final_layer=$(
                jq -r '.layers[-1].digest | sub("^sha256:"; "")' "$manifest_path"
              )
              python3 - "$tmp/image/blobs/sha256/$final_layer" <<'PY'
              import posixpath
              import sys
              import tarfile

              target = "var/lib/cloud-fman-telemetry"
              with tarfile.open(sys.argv[1], mode="r:*") as layer:
                  matches = [
                      member
                      for member in layer.getmembers()
                      if posixpath.normpath(member.name.lstrip("/")) == target
                  ]

              if len(matches) != 1:
                  raise SystemExit(
                      f"final layer has {len(matches)} canonical data-directory entries"
                  )
              entry = matches[0]
              if not entry.isdir() or (entry.mode & 0o777) != 0o700:
                  raise SystemExit("final layer data path is not a mode-0700 directory")
              if (entry.uid, entry.gid) != (10001, 10001):
                  raise SystemExit("final layer data directory is not UID/GID 10001")
              PY

               jq -e \
                 --arg entrypoint "${multiBuild.cloudFmanTelemetry}/bin/fedi-decentralized-cloud-fman-telemetry" \
                 --arg version "${pushGatewayVersion}" \
                 '
                   .config.Entrypoint == [$entrypoint]
                   and .config.Cmd == null
                   and .config.User == "10001:10001"
                   and .config.WorkingDir == "/var/lib/cloud-fman-telemetry"
                   and ((.config.Env | sort) == ([
                     "CLOUD_FMAN_TELEMETRY_PUBLIC_BIND=0.0.0.0:8175",
                     "CLOUD_FMAN_TELEMETRY_PRIVATE_BIND=0.0.0.0:8176",
                     "CLOUD_FMAN_TELEMETRY_DATA_DIR=/var/lib/cloud-fman-telemetry",
                     "CLOUD_FMAN_TELEMETRY_KEY_FILE=/run/secrets/cloud-fman-telemetry-key",
                     "CLOUD_FMAN_TELEMETRY_METRICS_POLL_SECONDS=1800",
                     "CLOUD_FMAN_TELEMETRY_LOG_POLL_SECONDS=300",
                     "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                   ] | sort))
                   and (.config.Volumes == {
                     "/var/lib/cloud-fman-telemetry": {}
                   })
                   and (.config.ExposedPorts == {
                     "8175/tcp": {},
                     "8176/tcp": {}
                   })
                   and (.config.Healthcheck == {
                     Test: [
                       "CMD",
                       "${pkgs.curl}/bin/curl",
                       "-fsS",
                       "http://127.0.0.1:8176/health"
                     ],
                     Interval: 10000000000,
                     Timeout: 2000000000,
                     Retries: 6,
                     StartPeriod: 5000000000
                   })
                  and (.config.Labels["org.opencontainers.image.title"] == "cloud-fman-telemetry")
                  and (.config.Labels["org.opencontainers.image.version"] == $version)
                  and (.config.Labels["org.opencontainers.image.source"] == "https://github.com/fedibtc/manifold")
                  and (.config.Labels["org.opencontainers.image.licenses"] == "MIT")
                ' "$config_path" >/dev/null

              touch "$out"
            '';

        cloudFmanTelemetryCliContract = pkgs.runCommand "cloud-fman-telemetry-cli-contract" { } ''
          help=$(${multiBuild.cloudFmanTelemetry}/bin/fedi-decentralized-cloud-fman-telemetry --help)
          for flag in \
            --public-bind --private-bind --private-bind-isolated \
            --public-base-url --data-dir --key-file \
            --key-id --environment --lease-seconds --metrics-poll-seconds \
            --log-poll-seconds --log-quota-bytes \
            --log-retention-days; do
            printf '%s\n' "$help" | grep -q -- "$flag" \
              || { echo "collector help omits $flag" >&2; exit 1; }
          done
          touch "$out"
        '';

        # FLIP is published to the same registry as the other two images, so it
        # gets the same runtime-contract check they have: entrypoint, daemon
        # subcommand, bind addresses, persistence volume, and provenance labels.
        liquidityManagerContainerImageCheck =
          pkgs.runCommand "liquidity-manager-oci-image-check"
            {
              nativeBuildInputs = [
                pkgs.jq
                pkgs.gnutar
                pkgs.gzip
              ];
            }
            ''
              tmp=$(mktemp -d)
              tar -xzf ${liquidityManagerCiDockerImage} -C "$tmp"
              config_path="$tmp/$(jq -r '.[0].Config' "$tmp/manifest.json")"

              entrypoint=$(jq -r '.config.Entrypoint[0]' "$config_path")
              test -x "$entrypoint" \
                || { echo "configured entrypoint not executable: $entrypoint" >&2; exit 1; }

              jq -e \
                --arg entrypoint "${ciRuntimeBins}/bin/liquidity-manager-daemon" \
                --arg version "${pushGatewayVersion}" \
                '
                  .config.Entrypoint == [$entrypoint]
                  and .config.Cmd == ["run", "daemon"]
                  and .os == "linux"
                  and .config.WorkingDir == "/var/lib/flip"
                  and (.config.Env | index("FLIP_DATA_DIR=/var/lib/flip"))
                  and (.config.Env | index("FLIP_ADMIN_BIND_ADDRESS=0.0.0.0:8173"))
                  and (.config.Env | index("FLIP_PUBLIC_BIND_ADDRESS=0.0.0.0:8174"))
                  and (.config.Env | any(startswith("SSL_CERT_FILE=")))
                  and ((.config.Env // []) | all(test("(?i)(SECRET|TOKEN|PASSWORD)") | not))
                  and (.config.Volumes["/var/lib/flip"] == {})
                  and (.config.ExposedPorts == {"8173/tcp": {}, "8174/udp": {}})
                  and (.config.Labels["org.opencontainers.image.title"] == "flip-liquidity-manager")
                  and (.config.Labels["org.opencontainers.image.version"] == $version)
                  and (.config.Labels["org.opencontainers.image.source"] == "https://github.com/fedibtc/manifold")
                  and (.config.Labels["org.opencontainers.image.licenses"] == "MIT")
                ' "$config_path" >/dev/null

              touch "$out"
            '';

        # Standalone fedimintd, still wanted by FLIP's E2E tests and
        # `nix build .#fedimintd`. FMan runs its own bundled build.
        fleetManagerFedimintd = fedimint.packages.${system}.fedimintd;

        # Single source of truth for the bundled fedimintd's fork release.
        # `fleetManagerReleaseSync` binds this to the Fedimint source revision,
        # the package README, and the OCI label. The Git tag identifies the
        # fork build; `FEDIMINTD_VERSION_0_1` owns the separate typed DKG
        # identity below.
        fedimintdRelease = "0.11.1-fedi18";
        fedimintdDkgVersion = "0.11.1+fedi";
        # `fedimintd` exports this upstream package version in `app_start_ts`.
        # It deliberately differs from the Fedi release tag above.
        fedimintdMetricVersion = "0.11.1";
        fedimintSourceRev = "5703f543f76746369f0a11e0d1635ac395b2efac";
        stabilityPoolSourceRev = "2f35ea4e3b2516d35b8ed315455718cd3b336758";

        # Nextest, CLI checks, and OCI runtime-contract checks all stay on the
        # CI Cargo profile. `runtimeBins` enables the two package-qualified UI
        # features so the existing bundle also serves the embedded dashboards.
        ciRuntimeBins = multiBuild.ci.runtimeBins;

        # Reuse the package's runtime contract script as the image entrypoint,
        # resolving `fleet-manager` from the Nix store.
        mkFleetManagerEntrypoint =
          fleetManager:
          pkgs.writeShellApplication {
            name = "fleet-manager-entrypoint";
            runtimeInputs = [ fleetManager ];
            text = builtins.readFile ./packages/fleet-manager/entrypoint.sh;
          };
        fleetManagerEntrypoint = mkFleetManagerEntrypoint multiBuild.fleetManager;
        fleetManagerCiEntrypoint = mkFleetManagerEntrypoint ciRuntimeBins;

        # Stable, store-hash-independent entrypoint path. StartOS pins the
        # container entrypoint by path in its manifest, so expose the wrapper at
        # a fixed location that survives wrapper rebuilds.
        fleetManagerEntrypointPath = "/usr/local/bin/fleet-manager-entrypoint";
        mkFleetManagerEntrypointStable =
          entrypoint:
          pkgs.runCommand "fleet-manager-entrypoint-stable" { } ''
            mkdir -p "$out/usr/local/bin"
            ln -s ${entrypoint}/bin/fleet-manager-entrypoint \
              "$out${fleetManagerEntrypointPath}"
          '';
        fleetManagerEntrypointStable = mkFleetManagerEntrypointStable fleetManagerEntrypoint;
        fleetManagerCiEntrypointStable = mkFleetManagerEntrypointStable fleetManagerCiEntrypoint;

        mkFleetManagerContainerImage =
          fleetManager: entrypoint: entrypointStable:
          pkgs.dockerTools.buildLayeredImage {
            name = "fleet-manager";
            tag = pushGatewayVersion;
            contents = [
              fleetManager
              entrypointStable
              pkgs.cacert
            ];
            config = {
              Entrypoint = [ "${entrypoint}/bin/fleet-manager-entrypoint" ];
              Env = [
                "FLEET_MANAGER_DATA_DIR=/data"
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              ];
              Volumes = {
                "/data" = { };
              };
              WorkingDir = "/data";
              Labels = {
                "org.fedi.fedimintd.release" = fedimintdRelease;
                "org.opencontainers.image.title" = "fleet-manager";
                "org.opencontainers.image.description" =
                  "Fleet Manager daemon hosting Fedimint guardians for decentralized federations";
                "org.opencontainers.image.version" = pushGatewayVersion;
                "org.opencontainers.image.source" = "https://github.com/fedibtc/manifold";
                "org.opencontainers.image.licenses" = "MIT";
              };
            };
          };
        fleetManagerContainerImage =
          mkFleetManagerContainerImage multiBuild.fleetManager fleetManagerEntrypoint
            fleetManagerEntrypointStable;
        fleetManagerCiContainerImage =
          mkFleetManagerContainerImage ciRuntimeBins fleetManagerCiEntrypoint
            fleetManagerCiEntrypointStable;

        fleetManagerContainerLoad = pkgs.writeShellScriptBin "fleet-manager-container-load" ''
          set -euo pipefail
          if ! command -v docker >/dev/null 2>&1; then
            echo "docker is required to load ${fleetManagerContainerImage.imageName}:${fleetManagerContainerImage.imageTag}" >&2
            exit 127
          fi
          exec docker load -i ${fleetManagerContainerImage}
        '';

        # Machine-checked models backing `claims/` records. `lake build` fails
        # when a proof stops checking, and `FMan/Audit.lean` fails it when a
        # headline theorem starts depending on `sorryAx`, so a model whose
        # theorem breaks fails CI the way a broken test does. The project is
        # deliberately dependency-free (Lean core only), so this builds offline.
        leanProofs = pkgs.runCommand "lean-proofs" { nativeBuildInputs = [ pkgs.lean4 ]; } ''
          cp -r ${./lean} lean
          chmod -R u+w lean
          cd lean
          export HOME="$TMPDIR"
          lake build | tee build.log

          # `lake` reports a failing `run_cmd` through its exit status, but the
          # regenerated axiom list is the artefact worth keeping in the log.
          grep -q 'depends on axioms' build.log \
            || { echo "FMan/Audit.lean produced no axiom report" >&2; exit 1; }
          ! grep -q 'sorryAx' build.log \
            || { echo "a headline theorem depends on sorryAx" >&2; exit 1; }

          cp build.log "$out"
        '';

        # The package image entrypoint runs `fleet-manager serve <flags>`. Guard
        # against CLI-shape drift: fail if the `serve` subcommand or any flag the
        # entrypoint passes has left the CLI, so the packaged image can't
        # silently regress to exiting at argument parsing.
        fleetManagerCliContract = pkgs.runCommand "fleet-manager-cli-contract" { } ''
          entrypoint=${./packages/fleet-manager/entrypoint.sh}

          grep -q 'fleet-manager serve' "$entrypoint" \
            || { echo "entrypoint.sh no longer invokes 'fleet-manager serve'" >&2; exit 1; }

          if ! help=$(${ciRuntimeBins}/bin/fleet-manager serve --help 2>&1); then
            echo "'fleet-manager serve --help' failed (is the 'serve' subcommand gone?):" >&2
            echo "$help" >&2
            exit 1
          fi

          for flag in $(grep -oE -- '--[a-z][a-z-]*' "$entrypoint" | sort -u); do
            printf '%s\n' "$help" | grep -q -- "$flag" \
              || { echo "entrypoint.sh passes '$flag' but 'fleet-manager serve --help' does not list it" >&2; exit 1; }
          done

          touch "$out"
        '';

        # Inspect a CI-profile Fleet Manager OCI image the way
        # `pushGatewayContainerImageCheck` inspects the push-gateway image. Its
        # daemon enables `embedded-operator-ui`, so the Nix operator UI fetch and
        # Vite output are part of this required packaging check without a second
        # release-profile Rust graph.
        fleetManagerContainerImageCheck =
          pkgs.runCommand "fleet-manager-oci-image-check"
            {
              nativeBuildInputs = [
                pkgs.jq
                pkgs.gnutar
                pkgs.gzip
              ];
            }
            ''
              tmp=$(mktemp -d)
              tar -xzf ${fleetManagerCiContainerImage} -C "$tmp"
              config_path="$tmp/$(jq -r '.[0].Config' "$tmp/manifest.json")"

              entrypoint=$(jq -r '.config.Entrypoint[0]' "$config_path")

              # The configured path is a Nix store path in the image closure and
              # in this check's build closure, so test it directly.
              test -x "$entrypoint" || { echo "configured entrypoint not executable: $entrypoint" >&2; exit 1; }

              # StartOS pins the entrypoint by path; the stable wrapper path must
              # also resolve to the executable wrapper.
              test -x "${fleetManagerCiEntrypointStable}${fleetManagerEntrypointPath}" \
                || { echo "stable entrypoint path not executable" >&2; exit 1; }

              jq -e \
                --arg entrypoint "$entrypoint" \
                --arg release "${fedimintdRelease}" \
                '
                  .config.Entrypoint == [$entrypoint]
                  and .os == "linux"
                  and (.config.Env | index("FLEET_MANAGER_DATA_DIR=/data"))
                  and (.config.Env | any(startswith("SSL_CERT_FILE=")))
                  and (.config.Volumes["/data"] == { })
                  and (.config.WorkingDir == "/data")
                  and (.config.Labels["org.fedi.fedimintd.release"] == $release)
                  and (.config.Labels["org.opencontainers.image.source"] == "https://github.com/fedibtc/manifold")
                  and (.config.Labels["org.opencontainers.image.licenses"] == "MIT")
                ' "$config_path" >/dev/null

              touch "$out"
            '';

        # Anti-drift: bind the Fedimint release tag in flake.nix to its resolved
        # revision in flake.lock, the separate FEDIMINTD_VERSION_0_1 DKG
        # identity, the package README, and the OCI label.
        fleetManagerReleaseSync = pkgs.runCommand "fleet-manager-release-sync" { } ''
          release="${fedimintdRelease}"
          tag="v''${release}"

          check() {
            grep -q -- "$2" "$1" \
              || { echo "release drift: $1 does not contain '$2' (release $release)" >&2; exit 1; }
          }

          check ${./flake.nix} "fedibtc/fedimint/v0.11.1-fedi18"
          check ${./flake.lock} '"rev": "${fedimintSourceRev}"'
          check ${./crates/service-fleet-manager/src/lib.rs} "FEDIMINTD_VERSION_0_1: &str = \"${fedimintdDkgVersion}\""
          check ${./crates/fman/bin/build.rs} "FEDIMINT_SOURCE_REV: &str = \"${fedimintSourceRev}\""
          check ${./packages/fleet-manager/README.md} "$tag"

          touch "$out"
        '';

        # The Markdown inventory gives the privacy rationale; this compact
        # manifest gives source registration a fail-closed mechanical boundary.
        # Updating the Fedimint input, adding a registration anywhere in its
        # Rust source, or omitting a registration from the review all fail here
        # until the reviewer deliberately regenerates the manifest.
        cloudFmanTelemetryMetricsInventory =
          pkgs.runCommand "cloud-fman-telemetry-metrics-inventory"
            {
              nativeBuildInputs = [
                pkgs.diffutils
                pkgs.gawk
                pkgs.perl
                pkgs.python3
                pkgs.ripgrep
              ];
            }
            ''
              set -euo pipefail

              manifest=${./docs/telemetry/fedimint-metrics-v0.11.1-fedi18.tsv}
              privacy_inventory=${./docs/telemetry/metrics-privacy-inventory.md}
              source=${fedimint.outPath}
              stability_pool_source=${fedi.outPath}

              field() {
                ${pkgs.gawk}/bin/awk -F '\t' -v key="$1" '$1 == key { print $2 }' "$manifest"
              }
              require_exact() {
                actual=$(field "$1")
                test "$actual" = "$2" \
                  || {
                    echo "Fedimint metric manifest $1 is '$actual', expected '$2'" >&2
                    exit 1
                  }
              }

              require_exact release_tag "${fedimintdRelease}"
              require_exact metric_version "${fedimintdMetricVersion}"
              require_exact revision "${fedimintSourceRev}"
              grep -q -- "\"rev\": \"${fedimintSourceRev}\"" ${./flake.lock}
              grep -q -- "\"rev\": \"${stabilityPoolSourceRev}\"" ${./flake.lock}
              grep -q -- "fedibtc/fedimint/v${fedimintdRelease}" ${./flake.nix}
              grep -q -- "fedixyz/fedi/${stabilityPoolSourceRev}" ${./flake.nix}
              grep -q -- $'producer\tfedimint\tfedibtc/fedimint\tv${fedimintdRelease}\t${fedimintSourceRev}' "$manifest"
              grep -q -- $'producer\tstability_pool\tfedixyz/fedi\t${stabilityPoolSourceRev}\t${stabilityPoolSourceRev}' "$manifest"
              grep -Fq -- "stability-pool-client = { git = \"https://github.com/fedixyz/fedi\", rev = \"${stabilityPoolSourceRev}\"" ${./Cargo.toml}
              grep -Fq -- "stability-pool-server = { git = \"https://github.com/fedixyz/fedi\", rev = \"${stabilityPoolSourceRev}\"" ${./Cargo.toml}
              grep -Fq -- "stability-pool-common = { git = \"https://github.com/fedixyz/fedi\", rev = \"${stabilityPoolSourceRev}\"" ${./Cargo.toml}
              if ${pkgs.ripgrep}/bin/rg -n 'github\.com/fedixyz/fedi' ${./crates} --glob Cargo.toml; then
                echo "crate manifests must inherit Fedi dependencies from the workspace" >&2
                exit 1
              fi
              grep -Fq -- "source = \"git+https://github.com/fedixyz/fedi?rev=${stabilityPoolSourceRev}#${stabilityPoolSourceRev}\"" ${./Cargo.lock}
              test -d "$stability_pool_source/crates/modules/stability-pool/server"
              source_metric_version=$(
                ${pkgs.gawk}/bin/awk '
                  /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
                  /^\[/ { in_workspace_package = 0 }
                  in_workspace_package && /^version[[:space:]]*=/ {
                    gsub(/^[^"]*"|".*$/, "")
                    print
                    exit
                  }
                ' "$source/Cargo.toml"
              )
              test "$source_metric_version" = "${fedimintdMetricVersion}" \
                || {
                  echo "fedimintd metric version is '$source_metric_version', expected '${fedimintdMetricVersion}'" >&2
                  exit 1
                }

              stability_pool_closure="$TMPDIR/stability-pool-closure"
              ${pkgs.python3}/bin/python3 - \
                "$stability_pool_source" \
                ${./Cargo.lock} \
                "${stabilityPoolSourceRev}" \
                > "$stability_pool_closure" <<'PY'
              import pathlib
              import sys
              import tomllib

              source_root = pathlib.Path(sys.argv[1])
              with open(sys.argv[2], "rb") as lock_file:
                  cargo_lock = tomllib.load(lock_file)
              revision = sys.argv[3]
              expected_source = (
                  f"git+https://github.com/fedixyz/fedi?rev={revision}#{revision}"
              )
              expected_dependency_source = (
                  f"git+https://github.com/fedixyz/fedi?rev={revision}"
              )
              with open(source_root / "Cargo.toml", "rb") as manifest_file:
                  workspace_manifest = tomllib.load(manifest_file)
              workspace_dependencies = workspace_manifest.get("workspace", {}).get(
                  "dependencies", {}
              )

              def resolved_dependency_name(
                  dependency_key,
                  dependency_spec,
                  inherited_dependencies=workspace_dependencies,
              ):
                  if (
                      isinstance(dependency_spec, dict)
                      and dependency_spec.get("workspace") is True
                  ):
                      if dependency_key not in inherited_dependencies:
                          raise SystemExit(
                              f"workspace dependency {dependency_key} is undefined"
                          )
                      dependency_spec = inherited_dependencies[dependency_key]
                  if isinstance(dependency_spec, dict):
                      return dependency_spec.get("package", dependency_key)
                  return dependency_key

              # Fixture: aliases inherited from [workspace.dependencies] must use
              # the workspace package name, not the local dependency key.
              workspace_alias_fixture = tomllib.loads(
                  """
                  [workspace.dependencies]
                  pool-common-alias = { package = "stability-pool-common", version = "0.3" }
                  """
              )["workspace"]["dependencies"]
              assert (
                  resolved_dependency_name(
                      "pool-common-alias",
                      {"workspace": True},
                      workspace_alias_fixture,
                  )
                  == "stability-pool-common"
              )

              packages = {}
              for path in source_root.rglob("Cargo.toml"):
                  with open(path, "rb") as manifest_file:
                      manifest = tomllib.load(manifest_file)
                  name = manifest.get("package", {}).get("name")
                  if name is None:
                      continue
                  if name in packages:
                      raise SystemExit(f"duplicate package name in source: {name}")
                  packages[name] = (path, manifest)

              closure = set()
              pending = ["stability-pool-server"]
              while pending:
                  name = pending.pop()
                  if name in closure:
                      continue
                  if name not in packages:
                      raise SystemExit(
                          f"runtime package {name} is absent from the pinned source"
                      )
                  lock_packages = [
                      package
                      for package in cargo_lock["package"]
                      if package["name"] == name
                      and package.get("source") == expected_source
                  ]
                  if len(lock_packages) != 1:
                      raise SystemExit(
                          f"{name} has {len(lock_packages)} lock entries at "
                          f"{expected_source}, expected one"
                      )
                  closure.add(name)
                  _, manifest = packages[name]
                  dependency_tables = [manifest.get("dependencies", {})]
                  dependency_tables.extend(
                      target.get("dependencies", {})
                      for target in manifest.get("target", {}).values()
                      if isinstance(target, dict)
                  )
                  expected_lock_edges = {
                      entry
                      for entry in lock_packages[0].get("dependencies", [])
                      if entry.split(" ", 1)[0] in packages
                      and (
                          " " not in entry
                          or entry.endswith(f" ({expected_dependency_source})")
                      )
                  }
                  accounted_lock_edges = set()
                  for dependencies in dependency_tables:
                      for dependency_key, dependency_spec in dependencies.items():
                          dependency = resolved_dependency_name(
                              dependency_key,
                              dependency_spec,
                          )
                          if dependency in packages:
                              lock_dependencies = [
                                  entry
                                  for entry in lock_packages[0].get("dependencies", [])
                                  if entry == dependency
                                  or (
                                      entry.startswith(f"{dependency} ")
                                      and entry.endswith(
                                          f" ({expected_dependency_source})"
                                      )
                                  )
                              ]
                              if len(lock_dependencies) != 1:
                                  raise SystemExit(
                                      f"{name} does not select {dependency} from "
                                      f"{expected_source}"
                                  )
                              accounted_lock_edges.update(lock_dependencies)
                              pending.append(dependency)
                  unexpected_lock_edges = expected_lock_edges - accounted_lock_edges
                  if unexpected_lock_edges:
                      raise SystemExit(
                          f"{name} has unaccounted same-source runtime edges: "
                          f"{sorted(unexpected_lock_edges)}"
                      )

              for name in sorted(closure):
                  path, _ = packages[name]
                  print(f"{name}\t{path.parent.relative_to(source_root)}")
              PY
              ${pkgs.gawk}/bin/awk -F '\t' \
                '$1 == "package" && $2 == "stability_pool" { print $3 "\t" $4 }' \
                "$manifest" \
                | sort -u > "$TMPDIR/expected-stability-pool-closure"
              ${pkgs.diffutils}/bin/diff -u \
                "$TMPDIR/expected-stability-pool-closure" \
                "$stability_pool_closure"

              actual="$TMPDIR/actual"
              expected="$TMPDIR/expected"
              metric_source_paths() {
                producer="$1"
                root="$2"
                paths="$3"

                if ${pkgs.ripgrep}/bin/rg -l --glob '*.rs' \
                  '(?:histogram_opts|opts)[[:space:]]*![[:space:]]*\(' "$root" \
                  > "$paths"; then
                  return 0
                else
                  status=$?
                fi

                if test "$status" = 1; then
                  # No registrations is valid for a checked zero-registration
                  # producer, but every scanner failure must stop the inventory.
                  return 0
                fi
                echo "metric source scan for $producer at $root failed with ripgrep status $status" >&2
                return "$status"
              }
              extract_metrics() {
                producer="$1"
                root="$2"
                paths=$(mktemp "$TMPDIR/$producer-metrics.XXXXXX")

                metric_source_paths "$producer" "$root" "$paths"
                sort -u "$paths" \
                  | while IFS= read -r path; do
                    ${pkgs.perl}/bin/perl -0777 -ne \
                      '
                        my @invocations = /(?:histogram_opts|opts)\s*!\s*\(/g;
                        my @families = /(?:histogram_opts|opts)\s*!\s*\(\s*"([^"]+)"/g;
                        die "unparseable metric registration in $ARGV\n"
                          unless @invocations == @families;
                        print "$_\n" for @families;
                      ' \
                      "$path"
                  done \
                  | sed "s/$/\t$producer/"
              }
              if metric_source_paths fixture "$TMPDIR/missing-metric-source" \
                "$TMPDIR/missing-metric-source-paths" >/dev/null 2>&1; then
                echo "missing metric source fixture unexpectedly scanned successfully" >&2
                exit 1
              else
                test "$?" = 2 \
                  || {
                    echo "missing metric source fixture did not preserve ripgrep status 2" >&2
                    exit 1
                  }
              fi
              {
                extract_metrics fedimint "$source"
                while IFS=$'\t' read -r _ path; do
                  extract_metrics stability_pool "$stability_pool_source/$path"
                done < "$stability_pool_closure"
              } | sort -u > "$actual"
              ${pkgs.gawk}/bin/awk -F '\t' '$1 == "metric" { print $3 "\t" $4 }' "$manifest" \
                | sort -u > "$expected"
              ${pkgs.diffutils}/bin/diff -u "$expected" "$actual"

              ${pkgs.gawk}/bin/awk -F '\t' '
                /^#/ || NF == 0 { next }
                $1 == "release_tag" || $1 == "metric_version" || $1 == "revision" {
                  if (NF != 2 || seen[$1]++) exit 1
                  next
                }
                $1 == "producer" {
                  if (NF != 5 || seen["producer:" $2]++) exit 1
                  next
                }
                $1 == "package" {
                  if (NF != 4 || !("producer:" $2 in seen) ||
                      seen["package:" $2 ":" $3]++) exit 1
                  next
                }
                $1 == "metric" {
                  if (NF != 4 || ($2 != "admit" && $2 != "deny") ||
                      !("producer:" $4 in seen) || seen["metric:" $3]++) exit 1
                  next
                }
                { exit 1 }
                END {
                  if (!seen["release_tag"] || !seen["metric_version"] || !seen["revision"]) exit 1
                }
              ' "$manifest" \
                || { echo "invalid or ambiguous metric manifest schema" >&2; exit 1; }

              while IFS=$'\t' read -r kind disposition family producer; do
                test "$kind" = metric || continue
                grep -Fq -- "\`$family\`" "$privacy_inventory" \
                  || {
                    echo "privacy inventory omits reviewed $disposition family: $family" >&2
                    exit 1
                  }
              done < "$manifest"

              touch "$out"
            '';

        mkPushGatewayContainerImage =
          pushGateway:
          pkgs.dockerTools.buildLayeredImage {
            name = "fedi-decentralized-push-gateway";
            tag = pushGatewayVersion;
            contents = [
              pushGateway
              pkgs.cacert
              pkgs.fakeNss
            ];
            fakeRootCommands = ''
              mkdir -p var/lib/push-gateway run/secrets
              chown 65534:65534 var/lib/push-gateway
              chmod 0750 var/lib/push-gateway
            '';
            config = {
              Entrypoint = [ "${pushGateway}/bin/fedi-decentralized-push-gateway" ];
              Cmd = [ ];
              Env = [
                "PUSH_GATEWAY_BIND=0.0.0.0:3000"
                "PUSH_GATEWAY_DATABASE_URL=sqlite:///var/lib/push-gateway/push.sqlite?mode=rwc"
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "NIX_SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              ];
              User = "65534:65534";
              WorkingDir = "/var/lib/push-gateway";
              Volumes = {
                "/var/lib/push-gateway" = { };
                "/run/secrets" = { };
              };
              ExposedPorts = {
                "3000/tcp" = { };
                "9100/tcp" = { };
              };
              StopSignal = "SIGTERM";
              Labels = {
                "org.opencontainers.image.title" = "fedi-decentralized-push-gateway";
                "org.opencontainers.image.description" =
                  "Webhook-to-mobile-push gateway for decentralized federations";
                "org.opencontainers.image.version" = pushGatewayVersion;
                "org.opencontainers.image.source" = "https://github.com/fedibtc/manifold";
                "org.opencontainers.image.licenses" = "MIT";
              };
            };
          };
        pushGatewayContainerImage = mkPushGatewayContainerImage multiBuild.pushGateway;
        pushGatewayCiContainerImage = mkPushGatewayContainerImage ciRuntimeBins;

        pushGatewayContainerLoad = pkgs.writeShellScriptBin "push-gateway-container-load" ''
          set -euo pipefail
          if ! command -v docker >/dev/null 2>&1; then
            echo "docker is required to load ${pushGatewayContainerImage.imageName}:${pushGatewayContainerImage.imageTag}" >&2
            exit 127
          fi
          exec docker load -i ${pushGatewayContainerImage}
        '';

        pushGatewayContainerImageCheck =
          pkgs.runCommand "push-gateway-oci-image-check"
            {
              nativeBuildInputs = [
                pkgs.jq
                pkgs.gnutar
                pkgs.gzip
              ];
            }
            ''
              tmp=$(mktemp -d)
              tar -xzf ${pushGatewayCiContainerImage} -C "$tmp"
              config_path="$tmp/$(jq -r '.[0].Config' "$tmp/manifest.json")"

              jq -e \
                --arg entrypoint "${ciRuntimeBins}/bin/fedi-decentralized-push-gateway" \
                --arg version "${pushGatewayVersion}" \
                '
                  .config.Entrypoint == [$entrypoint]
                  and .config.Cmd == []
                  and .os == "linux"
                  and .config.User == "65534:65534"
                  and .config.WorkingDir == "/var/lib/push-gateway"
                  and .config.StopSignal == "SIGTERM"
                  and (.config.Env | index("PUSH_GATEWAY_BIND=0.0.0.0:3000"))
                  and (.config.Env | index("PUSH_GATEWAY_DATABASE_URL=sqlite:///var/lib/push-gateway/push.sqlite?mode=rwc"))
                  and (.config.Env | any(startswith("SSL_CERT_FILE=")))
                  and (.config.Env | any(startswith("NIX_SSL_CERT_FILE=")))
                  and ((.config.Env // []) | all(test("(?i)(SECRET|TOKEN|PASSWORD|APP_ID|FCM_SERVICE_ACCOUNT|FIREBASE_CREDENTIALS)") | not))
                  and (.config.Volumes["/var/lib/push-gateway"] == {})
                  and (.config.Volumes["/run/secrets"] == {})
                  and (.config.ExposedPorts["3000/tcp"] == {})
                  and (.config.ExposedPorts["9100/tcp"] == {})
                  and (.config.Labels["org.opencontainers.image.title"] == "fedi-decentralized-push-gateway")
                  and (.config.Labels["org.opencontainers.image.description"] == "Webhook-to-mobile-push gateway for decentralized federations")
                  and (.config.Labels["org.opencontainers.image.version"] == $version)
                  and (.config.Labels["org.opencontainers.image.source"] == "https://github.com/fedibtc/manifold")
                  and (.config.Labels["org.opencontainers.image.licenses"] == "MIT")
                ' "$config_path" >/dev/null

              touch "$out"
            '';

        # Operator dashboards (FMan + FLIP).
        #
        # Both apps are Vite SPAs that call relative URLs and must be served
        # from the same origin as the daemon they operate: FMan's session is an
        # HttpOnly same-origin cookie set by `POST /api/auth`
        # (crates/fman/core/src/admin_http.rs), and FLIP sends a bearer token to
        # `/admin/v1/*`. The release daemons above satisfy that by embedding
        # this output and serving it from their own listener, so there is no
        # second origin and no reverse proxy to keep in step.
        #
        # Unlike the daemon closures this derivation carries no private
        # credential SDK -- `src` is the JavaScript workspace alone -- so
        # SECURITY.md's prohibition on pushing to the public Cachix does not
        # apply to it.
        # Include the lockfile digest in the fixed-output derivation name. Nix
        # may otherwise reuse an already-valid dependency store path when the
        # lockfile changes but the manually maintained hash does not, masking a
        # stale hash on a long-lived CI runner.
        operatorUiLockDigest = builtins.hashFile "sha256" ./operator-ui/pnpm-lock.yaml;
        operatorUiDeps = pkgs.pnpm_9.fetchDeps {
          pname = "operator-ui-${operatorUiLockDigest}";
          version = pushGatewayVersion;
          src = ./operator-ui;
          fetcherVersion = 2;
          # Refresh whenever operator-ui/pnpm-lock.yaml changes; the build error
          # prints the replacement.
          hash = "sha256-2uuWz8DS7znGIxxN8d7UskHko52+Jskk3NsLdi0f1dk=";
        };

        operatorUi = pkgs.stdenv.mkDerivation {
          pname = "operator-ui";
          version = pushGatewayVersion;
          src = ./operator-ui;
          nativeBuildInputs = [
            pkgs.nodejs_22
            pkgs.pnpm_9.configHook
          ];
          pnpmDeps = operatorUiDeps;
          # Root package.json: "build": "pnpm -r --filter \"./apps/*\" build".
          # `vite build` sets DEV=false, which is what switches the MSW mocks
          # off, so no mock-related configuration is needed here.
          buildPhase = ''
            runHook preBuild
            pnpm build
            runHook postBuild
          '';
          installPhase = ''
            runHook preInstall
            mkdir -p "$out/srv"
            cp -r apps/fleet-manager/dist "$out/srv/fman"
            cp -r apps/liquidity-provider/dist "$out/srv/flip"
            runHook postInstall
          '';
        };

        # One release-artifact target for publishing. Keeping every named image
        # link in one aggregate prevents the workflow's FMan, FLIP, cloud
        # collector and push gateway package selections from drifting apart.
        # Required CI uses the same image constructors with UI-enabled CI-profile
        # daemons above.
        releaseContainerImages = pkgs.linkFarm "release-container-images" [
          {
            name = "fleet-manager-image";
            path = fleetManagerContainerImage;
          }
          {
            name = "liquidity-manager-image";
            path = liquidityManagerDockerImage;
          }
          {
            name = "cloud-fman-telemetry-image";
            path = cloudFmanTelemetryContainerImage;
          }
          {
            name = "push-gateway-image";
            path = pushGatewayContainerImage;
          }
        ];

      in
      {
        packages = {
          inherit treefmt;
          cloud-fman-telemetry = multiBuild.cloudFmanTelemetry;
          push-gateway = multiBuild.pushGateway;
          fleet-manager = multiBuild.fleetManager;
          fman-cli = multiBuild.fleetManager;
          setup-payment-publisher = multiBuild.setupPaymentPublisher;
          # The flake-pinned fedimint binaries, re-exposed so E2E runs can point
          # FMAN_E2E_FEDIMINTD_BIN / FMAN_E2E_FEDIMINT_CLI_BIN at the exact build
          # the CI test derivation and the OCI image use.
          fedimintd = fleetManagerFedimintd;
          fedimint-cli = fedimint.packages.${system}.fedimint-cli;
          fedimint-load-test-tool = fedimint.packages.${system}.fedimint-load-test-tool;
          default = multiBuild.${projectName};
          liquidityManagerDaemon = multiBuild.liquidityManagerDaemon;
          # The built dashboards on their own, so the static output can be
          # inspected or served without unpacking a daemon binary.
          operator-ui = operatorUi;
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          cloud-fman-telemetry-oci-image = cloudFmanTelemetryContainerImage;
          push-gateway-oci-image = pushGatewayContainerImage;
          fleet-manager-oci-image = fleetManagerContainerImage;
          release-container-images = releaseContainerImages;
          # OCI images only build on Linux; keep FLIP gated like the other two so
          # `nix flake show` / darwin evaluation does not offer an unbuildable
          # attribute.
          inherit liquidityManagerDockerImage;
        };

        apps = {
          push-gateway = {
            type = "app";
            program = "${multiBuild.pushGateway}/bin/fedi-decentralized-push-gateway";
          };
          setup-payment-publisher = {
            type = "app";
            program = "${multiBuild.setupPaymentPublisher}/bin/setup-payment-publisher";
          };
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          selfci-check = {
            type = "app";
            program = "${selfciCheck}/bin/selfci-check";
          };
          push-gateway-container-load = {
            type = "app";
            program = "${pushGatewayContainerLoad}/bin/push-gateway-container-load";
          };
          cloud-fman-telemetry-container-load = {
            type = "app";
            program = "${pkgs.writeShellScript "cloud-fman-telemetry-container-load" ''
              set -euo pipefail
              if ! command -v docker >/dev/null 2>&1; then
                echo "docker is required" >&2
                exit 127
              fi
              exec docker load -i ${cloudFmanTelemetryContainerImage}
            ''}";
          };
          fleet-manager-container-load = {
            type = "app";
            program = "${fleetManagerContainerLoad}/bin/fleet-manager-container-load";
          };
        };

        ci = {
          # Keep ordinary Rust checks and OCI contract checks on the CI profile.
          # The FMan and FLIP checks still compile their embedded operator UIs;
          # only registry publishing builds the release-profile daemon graph.
          inherit (multiBuild.ci)
            clippy
            tests
            cargoFmt
            cargoDependencyHygiene
            ;
          inherit
            cloudFmanTelemetryCliContract
            cloudFmanTelemetryMetricsInventory
            fleetManagerCliContract
            fleetManagerReleaseSync
            leanProofs
            ;
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          cloudFmanTelemetryOciImage = cloudFmanTelemetryContainerImageCheck;
          pushGatewayOciImage = pushGatewayContainerImageCheck;
          fleetManagerOciImage = fleetManagerContainerImageCheck;
          liquidityManagerOciImage = liquidityManagerContainerImageCheck;
        };

        legacyPackages = multiBuild;

        devShells = flakeboxLib.mkShells {
          packages = [
            linkExternalDeps
            pkgs.pkg-config
            pkgs.cargo-nextest
            pkgs.cargo-watch
            pkgs.nodejs_22
            pkgs.pnpm
            # aws-lc-sys (via fedimint) builds with cmake
            pkgs.cmake
            # Proof models under `lean/`; see `leanProofs`.
            pkgs.lean4
            pkgs.nostr-rs-relay
            pkgs.bitcoind
            fedimint.packages.${system}.gatewayd
            fedimint.packages.${system}.gateway-cli
            fedimint.packages.${system}.fedimintd
            fedimint.packages.${system}.fedimint-cli
            fedimint.packages.${system}.fedimint-load-test-tool
            # esplora for the FMAN_E2E tier (scripts/test-e2e-local.sh); the
            # same flake-pinned build the CI tests derivation uses.
            fedimintPkgs.esplora-electrs
            pkgs.sqlx-cli
            pkgs.systemfd
            pkgs.taplo
          ]
          # selfci (and the `mq` wrapper) call the Linux-only pidfd syscall and
          # fail to build on darwin, which blocks the whole dev shell. They are a
          # dev convenience, not a build dependency, so gate them to Linux.
          ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            selfciPkg
            mq
          ];
          shellHook = ''
            link-external-deps "$FLAKEBOX_PROJECT_ROOT_DIR"

            ${linkProjectSkills}/bin/link-project-skills "$FLAKEBOX_PROJECT_ROOT_DIR"

            export DEFE_SOCKET="$PWD/.defe.sock"
            export DEV_DEFE_SOCKET_PATH="$DEFE_SOCKET"

            # SP-enabled fedimintd for the live stability-pool E2E. It shares the
            # binary name `fedimintd` with the base build, so it is passed by env
            # var rather than added to PATH.
            export FLIP_E2E_SP_FEDIMINTD_BIN=${fedi.packages.${system}.fedi-fedimintd}/bin/fedimintd
          '';
        };
      }
    );

  nixConfig = {
    extra-substituters = [ "https://fedimint.cachix.org" ];
    extra-trusted-public-keys = [
      "fedimint.cachix.org-1:FpJJjy1iPVlvyv4OMiN5y9+/arFLPcnZhZVVCHCDYTs="
    ];
  };
}
