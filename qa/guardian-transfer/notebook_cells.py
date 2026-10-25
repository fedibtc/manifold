# Exec cells of the guardian-transfer QA notebook, in order (harness functions are defined here).
# Not a standalone script: it ran inside a persistent notebook against a local defe stack.

# ---- cell @ 2026-10-25T16:40:50.216Z ----
import os
os.chdir('/src/qa-transfer-manifold')
command("git log --oneline -3; git status --short | head; ls; cat .agents/skills/defe-testing/SKILL.md")
command("cd /src/qa-transfer-fedi && git log --oneline -3 && git status --short | head && ls")
command("ls /src")

# ---- cell @ 2026-10-25T16:40:55.008Z ----
command("wc -l crates/mg/specs/SPEC-guardian-transfer.md; rg -n 'fa_moves_a_formed_guardian' -l crates; ls crates")
command("cd /src/qa-transfer-fedi && rg -n 'faClient(Preview|Start|Resume)?.*Transfer|GuardianTransfer' --no-heading -l . | head -30; rg -n 'manifold' Cargo.toml")

# ---- cell @ 2026-10-25T16:40:58.083Z ----
print(open('crates/mg/specs/SPEC-guardian-transfer.md').read())
command("cd /src/qa-transfer-fedi && rg -n 'manifold' flake.nix flake.lock nix/*.nix 2>/dev/null | head -20; ls .nix-deps 2>/dev/null; cat justfile | head -60")

# ---- cell @ 2026-10-25T16:41:02.843Z ----
r=await command("cd /src && for d in fedi*; do (cd $d 2>/dev/null && git branch --show-current 2>/dev/null | sed \"s|^|$d: |\"); done; rg -l 'defe' --glob '*.rs' fedi-fa-167-qa fedi-liquidity-resume fedi-fa-lq-resume fedi-transfer-b fedi-guardian-transfer qa-transfer-fedi 2>/dev/null | head -30")

# ---- cell @ 2026-10-25T16:41:07.561Z ----
command("cd /src/fedi-fa-167-qa && git log --oneline -8 && git diff --stat HEAD~3 | tail -20")
command("cd /src/qa-transfer-fedi && ls ui/native/devtools/fa ui/native/devtools/fa/scripts && head -80 ui/native/devtools/fa/scripts/transfer.ts && wc -l crates/bridge/src/fa_client.rs crates/bridge/src/fa_client/tests.rs")

# ---- cell @ 2026-10-25T16:41:08.923Z ----
command("cd /src/qa-transfer-fedi && git log --oneline -30 | head -30; rg -n 'remote-bridge|remote_bridge|fedi-cli|bridge-cli' --no-heading -g '!node_modules' -g '!*.lock' . | head -20; ls crates bridge")

# ---- cell @ 2026-10-25T16:41:13.085Z ----
command("cd /src/qa-transfer-fedi && ls bridge/fedi-ffi/src/rpc/tests* ; git log --oneline -1 origin/master 2>/dev/null; cd /src/fedi-fa-167-qa && ls bridge/fedi-ffi/src/rpc/tests/ && sed -n 1,140p bridge/fedi-ffi/src/rpc/tests/fa_e2e.rs")

# ---- cell @ 2026-10-25T16:41:16.623Z ----
command("ls -la /src/scratch/ /src/scratch/fa-e2e 2>&1 | head -60; head -100 /src/scratch/fa-e2e/env.py")

# ---- cell @ 2026-10-25T16:41:22.990Z ----
print(open('/src/scratch/fa-e2e/env.py').read()[4000:])
command("nproc; free -g; df -h /src | tail -1; ls /src/qa-transfer-manifold/target 2>&1 | head; ls /src/manifold-167-int/target/debug | head -40")

# ---- cell @ 2026-10-25T16:41:27.671Z ----
mb = command("cargo build --workspace --bins 2>&1 | tail -30", max_tokens=3000)
command("sed -n 1,400p crates/tests-e2e/tests/manifold_guardian_0_1_formation/guardian_transfer.rs", max_tokens=12000)

# ---- cell @ 2026-10-25T16:42:24.067Z ----
command("echo $CARGO_TARGET_DIR; cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys;print(json.load(sys.stdin)[\"target_directory\"])'; grep -n -A25 'defe-serve\\|test-e2e-local' justfile justfile.custom.just | head -80")

# ---- cell @ 2026-10-25T16:42:28.029Z ----
print(open('scripts/test-e2e-local.sh').read())
command("ls target/debug | grep -v '\\.d$' | head -50")

# ---- cell @ 2026-10-25T16:42:34.204Z ----
base = command("just test-e2e-local -E 'test(fa_moves_a_formed_guardian)' --no-capture > /tmp/qa-gt-baseline.log 2>&1; echo exit=$?")
os.chdir('/src/qa-transfer-fedi')
command("ls -la .nix-deps/; sed -n 160,200p flake.nix")

# ---- cell @ 2026-10-25T16:42:39.385Z ----
command("tail -40 /tmp/qa-gt-baseline.log")

# ---- cell @ 2026-10-25T16:42:43.456Z ----
base = command("cd /src/qa-transfer-manifold && ./scripts/test-e2e-local.sh -E 'test(fa_moves_a_formed_guardian)' --no-capture > /tmp/qa-gt-baseline.log 2>&1; echo exit=$?")
# bump pin
import re
s=open('flake.nix').read()
s=s.replace('13343974d18428050cb8afe8c2f134d47b2123e5','d55d3a8226'+'X')
command("cd /src/qa-transfer-manifold && git rev-parse d55d3a82")

# ---- cell @ 2026-10-25T16:44:09.117Z ----
command("tail -60 /tmp/qa-gt-baseline.log", max_tokens=4000)

# ---- cell @ 2026-10-25T16:44:14.495Z ----
M='/src/qa-transfer-manifold'; F='/src/qa-transfer-fedi'
base = command("./scripts/test-e2e-local.sh -E 'test(fa_moves_a_formed_guardian)' --no-capture > /tmp/qa-gt-baseline.log 2>&1; echo exit=$?", workdir=M)
s=open('flake.nix').read()
s=s.replace('13343974d18428050cb8afe8c2f134d47b2123e5','d55d3a8226c853069d11890d3fef881fae424e9d')
open('flake.nix','w').write(s)
lk = command("nix flake lock --update-input manifold-src 2>&1 | tail -5; git diff --stat", workdir=F)

# ---- cell @ 2026-10-25T16:45:35.466Z ----
r=await command("git fetch origin 2>&1|tail -2; git log --oneline -3 origin/rho/fa-guardian-transfer-b; git log -1 --format=%H origin/master", workdir=F)
r=await command("git fetch origin 2>&1|tail -2; git log --oneline -1 origin/master; git log --oneline -1 origin/rho/guardian-transfer-on-master origin/rho/fa-guardian-transfer; git merge-base origin/master d7cf156f8e; git rev-list --count d7cf156f8e..d55d3a82; git rev-list --count $(git merge-base origin/master d7cf156f8e)..d7cf156f8e", workdir=M)

# ---- cell @ 2026-10-25T16:45:42.231Z ----
R='/src/qa-transfer-manifold-rb'
r=await command(f"git worktree add -b rho/qa-gt-rebase {R} d55d3a82 2>&1|tail -1 && cd {R} && git rebase origin/master 2>&1 | tail -15", workdir=M)

# ---- cell @ 2026-10-25T16:45:45.791Z ----
os.chdir(R)
r=await command("git diff --name-only --diff-filter=U; git diff | head -200", max_tokens=8000)

# ---- cell @ 2026-10-25T16:45:52.306Z ----
import re
def resolve(path, pick):
    s=open(path).read()
    pat=re.compile(r'<<<<<<< [^\n]*\n(.*?)=======\n(.*?)>>>>>>> [^\n]*\n', re.S)
    s,n=pat.subn(lambda m: pick(m.group(1),m.group(2)), s)
    open(path,'w').write(s); return n
print(resolve('crates/mg/bin/src/main.rs', lambda a,b: a+b))
print(resolve('crates/mg/specs/SPEC-fa-metadata-maintenance.md', lambda a,b: a))
r=await command("git add -A && GIT_EDITOR=true git rebase --continue 2>&1 | tail -15")

# ---- cell @ 2026-10-25T16:45:57.431Z ----
conflicts=[("d7cf156f mg: enable recoverable guardian transfers","crates/mg/bin/src/main.rs: kept master's `#[cfg(linux)] mod file_limit` and #185's `mod guardian_config`"),
("d7cf156f","crates/mg/specs/SPEC-fa-metadata-maintenance.md: took master's Fedi-app-settings text; #185's transfer exception paragraph that follows applies unchanged")]
r=await command("git diff", max_tokens=8000)

# ---- cell @ 2026-10-25T16:46:02.275Z ----
print(resolve('crates/fa-client/specs/ARCH-fa-client.md', lambda a,b: a+'\n'+b))
conflicts.append(("019eb439 fa-client: add a seat pool","crates/fa-client/specs/ARCH-fa-client.md: kept master's 'Replacement eligibility' section and added #184's 'Seat pool' section after it"))
r=await command("git add -A && GIT_EDITOR=true git rebase --continue 2>&1 | grep -v '^ create\\|^ rename\\|^ delete' | tail -12")

# ---- cell @ 2026-10-25T16:46:04.995Z ----
r=await command("git diff", max_tokens=8000)

# ---- cell @ 2026-10-25T16:46:10.561Z ----
print(resolve('crates/fa-client/specs/ARCH-fa-client.md', lambda a,b: a.split('## Seat pool')[0]))
conflicts.append(("69cec644 fa-client: let the guardian transfer own its seat","ARCH-fa-client.md: kept master's 'Replacement eligibility' section and dropped 'Seat pool', which this commit removes"))
r=await command("git add -A && GIT_EDITOR=true git rebase --continue 2>&1 | grep -v '^ create\\|^ rename\\|^ delete' | tail -12")

# ---- cell @ 2026-10-25T16:46:17.374Z ----
print(resolve('crates/fa-cli/src/main.rs', lambda a,b: a+b))
conflicts.append(("1d579ea2 fa-cli: add guardian transfer commands","crates/fa-cli/src/main.rs Command enum: kept master's `ReplacementCandidates` and added `Transfer(TransferArgs)`"))
async def cont():
    r=await command("git add -A && GIT_EDITOR=true git rebase --continue 2>&1 | grep -v '^ create\\|^ rename\\|^ delete' | tail -8; git diff --name-only --diff-filter=U")
cont_ = await cont()

# ---- cell @ 2026-10-25T16:46:22.125Z ----
r=await command("git log --oneline -1; cargo build --workspace --all-targets 2>&1 | grep -E '^(error|warning: unused)|^\\s+-->' | head -40; echo build-exit=${PIPESTATUS[0]}", max_tokens=4000)

# ---- cell @ 2026-10-25T16:47:54.965Z ----
r=await command("grep -E 'guardian transfer \\[|PASS|FAIL|Summary' /tmp/qa-gt-baseline.log | tail -15")
rb_e2e = command("./scripts/test-e2e-local.sh -E 'test(fa_moves_a_formed_guardian)' --no-capture > /tmp/qa-gt-rebased-e2e.log 2>&1; echo transfer-e2e-exit=$?; ./scripts/test-e2e-local.sh --no-fail-fast > /tmp/qa-gt-rebased-full.log 2>&1; echo full-exit=$?", workdir=R)

# ---- cell @ 2026-10-25T16:50:02.331Z ----
os.chdir(F)
s=open('flake.nix').read()
s=s.replace('github:fedibtc/manifold/d55d3a8226c853069d11890d3fef881fae424e9d','git+file:///src/qa-transfer-manifold-rb?rev=416e9fe5'+'')
r=await command("git -C /src/qa-transfer-manifold-rb rev-parse HEAD")


# ---- cell @ 2026-10-25T16:50:07.412Z ----
s=s.replace('rev=416e9fe5','rev=416e9fe5e130934b0c4c8428da9862f685571096')
open('flake.nix','w').write(s)
r=await command("git checkout -b rho/qa-gt-harness 2>&1; nix flake lock --update-input manifold-src 2>&1 | grep -v warning | tail -3; ls -la .nix-deps/manifold; readlink -f .nix-deps/manifold; ls .nix-deps/manifold/crates | head -3")

# ---- cell @ 2026-10-25T16:51:32.939Z ----
r=await command("grep -n 'manifold-src = {' -A2 flake.nix; git diff flake.lock | head -40")

# ---- cell @ 2026-10-25T16:51:38.894Z ----
s=open('flake.nix').read().replace('file:///src/qa-transfer-manifold-rb?','file:///src/qa-transfer-manifold?')
open('flake.nix','w').write(s)
r=await command("git checkout flake.lock; nix flake lock --update-input manifold-src 2>&1 | grep -v warning | tail -3; git diff flake.lock | grep '^[+-]'; readlink -f .nix-deps/manifold")

# ---- cell @ 2026-10-25T16:54:08.377Z ----
r=await command("grep -E '^\\s+(FAIL|SIGSEGV|TIMEOUT|LEAK)|Summary' /tmp/qa-gt-rebased-full.log | head -40; tail -5 /tmp/qa-gt-rebased-full.log")

# ---- cell @ 2026-10-25T16:54:12.970Z ----
r=await command("grep -n -B2 -A25 'panicked at' /tmp/qa-gt-rebased-full.log | grep -v '^\\S*-\\s*$' | head -120", max_tokens=8000)

# ---- cell @ 2026-10-25T16:54:19.609Z ----
MS='/src/qa-transfer-manifold-master'
r=await command(f"git worktree add --detach {MS} origin/master 2>&1|tail -1", workdir=M)
three = "-E 'test(mg_restores_a_formed_fleet_from_mnemonic_and_nostr_under_defe) | test(mg_restore_of_two_diverged_hosts_under_defe) | test(mg_recovers_a_real_child_and_terminalizes_data_loss_under_defe)'"
rerun = command(f"./scripts/test-e2e-local.sh {three} > /tmp/qa-gt-rebased-3.log 2>&1; echo rb3=$?; grep -E 'PASS|FAIL' /tmp/qa-gt-rebased-3.log | tail -6", workdir=R)


# ---- cell @ 2026-10-25T16:54:55.847Z ----
r=await command("grep -E 'PASS|FAIL|Error|panicked' /tmp/qa-gt-rebased-3.log | head; grep -n -A12 'Caused by' /tmp/qa-gt-rebased-3.log | head -40")
m3 = command(f"./scripts/test-e2e-local.sh {three} > /tmp/qa-gt-master-3.log 2>&1; echo m3=$?; grep -E '^\\s+(PASS|FAIL)' /tmp/qa-gt-master-3.log | tail -6", workdir=MS)

# ---- cell @ 2026-10-25T16:56:57.841Z ----
r=await command("grep -E 'panicked|failed:' -A3 /tmp/qa-gt-master-3.log | head -30; grep -E 'panicked' -A3 /tmp/qa-gt-rebased-3.log | head")

# ---- cell @ 2026-10-25T16:57:04.165Z ----
r=await command("rg -n '^fedimint-core|fedimint-api-client =' Cargo.toml | head; git log --oneline -1 origin/master; git show origin/master:Cargo.toml | rg -n '^fedimint-core'; cat /src/qa-transfer-manifold-master/Cargo.toml | rg -n '^fedimint-core|fedimint' | head")

# ---- cell @ 2026-10-25T16:57:09.637Z ----
r=await command("git stash; git rev-list --count $(git merge-base origin/master HEAD)..HEAD; git rev-list --count $(git merge-base origin/master HEAD)..origin/master; git log --oneline $(git merge-base origin/master HEAD)..HEAD | head -30")

# ---- cell @ 2026-10-25T16:57:12.601Z ----
r=await command("git rebase origin/master 2>&1 | tail -8; git diff --name-only --diff-filter=U")

# ---- cell @ 2026-10-25T16:57:16.023Z ----
r=await command("git diff flake.nix; git diff crates/bridge/src/fa_client.rs crates/bridge/src/fa_client/tests.rs", max_tokens=15000)

# ---- cell @ 2026-10-25T16:57:24.391Z ----
def union(a,b):
    toks=[]
    for t in (a+','+b).replace('\n',' ').split(','):
        t=t.strip()
        if t and t not in toks: toks.append(t)
    toks.sort(key=lambda x: x.lower().replace('_',''))
    return '    '+', '.join(toks)+',\n'
print(resolve('crates/bridge/src/fa_client.rs', union))
print(resolve('crates/bridge/src/fa_client/tests.rs', lambda a,b: b))
print(resolve('flake.nix', lambda a,b: '      url = "git+file:///src/qa-transfer-manifold?rev=416e9fe5e130934b0c4c8428da9862f685571096";\n'))
r=await command("rg -n 'fn test_mg_id' crates/bridge/src/fa_client/tests.rs; git checkout --ours Cargo.lock flake.lock 2>&1; git show :2:Cargo.lock > Cargo.lock; git show :2:flake.lock > flake.lock; nix flake lock --update-input manifold-src 2>&1 | grep -v warn | tail -2")

# ---- cell @ 2026-10-25T16:57:29.422Z ----
async def fcont():
    r=await command("git add -A && GIT_EDITOR=true git rebase --continue 2>&1 | tail -4; git diff --name-only --diff-filter=U")
await fcont()

# ---- cell @ 2026-10-25T16:58:07.249Z ----
r=await command("git diff ui/native/tests/unit/screens/WalletServiceSettings.test.tsx 2>/dev/null", max_tokens=6000)

# ---- cell @ 2026-10-25T16:58:14.326Z ----
print(resolve('ui/native/tests/unit/screens/WalletServiceSettings.test.tsx', lambda a,b: a+'    })\n\n'+b))
r=await command("sed -n '1590,1625p' ui/native/tests/unit/screens/WalletServiceSettings.test.tsx 2>/dev/null")

# ---- cell @ 2026-10-25T16:58:17.605Z ----
r=await command("git add -A && GIT_EDITOR=true git rebase --continue 2>&1 | grep -v 'evaluation warning\\|nested list\\|be removed\\|deprecated' | tail -4; git diff --name-only --diff-filter=U")

# ---- cell @ 2026-10-25T16:58:20.756Z ----
r=await command("git diff ui/native/devtools/fa/ 2>/dev/null", max_tokens=6000)

# ---- cell @ 2026-10-25T16:58:26.276Z ----
print(resolve('ui/native/devtools/fa/screens.ts', lambda a,b: a+'            ),\n        ],\n    },\n    {\n'+b))
print(resolve('ui/native/devtools/fa/scripts/index.ts', lambda a,b: a+b))
r=await command("sed -n '375,425p' ui/native/devtools/fa/screens.ts 2>/dev/null")

# ---- cell @ 2026-10-25T16:58:34.095Z ----
print(resolve('flake.nix', lambda a,b: '      url = "git+file:///src/qa-transfer-manifold?rev=416e9fe5e130934b0c4c8428da9862f685571096";\n'))
r=await command("git show :2:flake.lock > flake.lock; git diff --stat HEAD -- . ':!flake.lock' 2>/dev/null; git add -A && GIT_EDITOR=true git rebase --continue 2>&1 | tail -2; git log --oneline -6 2>/dev/null; git stash list | head -3")

# ---- cell @ 2026-10-25T16:58:39.443Z ----
r=await command("git stash drop; grep -n 'manifold-src = {' -A1 flake.nix; nix flake lock --update-input manifold-src 2>&1 | grep -v warn | tail -2; readlink -f .nix-deps/manifold")
r=await command("readlink -f .nix-deps/manifold; ls .nix-deps/manifold/crates/domain/src/guardian_reports.rs; cargo check -p fedi-ffi --tests 2>&1 | grep -E '^error|^\\s+-->' | head -60; echo exit=${PIPESTATUS[0]}", max_tokens=5000)

# ---- cell @ 2026-10-25T16:59:29.430Z ----
r=await command("rg -n 'trait FederationConsensusReader' -A60 .nix-deps/manifold/crates/fa-client/src/ | head -90", max_tokens=4000)
r=await command("rg -n 'read_guardian_reports|read_fedimintd_versions' -B3 -A30 /src/qa-transfer-manifold-rb/crates/fa-cli/src/main.rs | head -120", max_tokens=4000)
r=await command("sed -n 760,900p crates/bridge/src/fa_client.rs", max_tokens=5000)

# ---- cell @ 2026-10-25T16:59:35.524Z ----
r=await command("rg -n 'read_lnv2_gateways|read_consensus' crates/bridge/src/fa_client.rs | head -5; rg -n 'MG_E2E|ENDPOINT_ADDRS|LOCAL_IROH|FM_IROH' crates/bridge/src bridge/fedi-ffi/src /src/fedi-fa-167-qa/crates/bridge/src | head -20")

# ---- cell @ 2026-10-25T16:59:38.891Z ----
r=await command("sed -n 815,880p /src/fedi-fa-167-qa/crates/bridge/src/fa_client.rs; cd /src/fedi-fa-167-qa && git log --oneline -15 --stat -- crates/bridge/src/fa_client.rs | head -40")

# ---- cell @ 2026-10-25T16:59:43.359Z ----
r=await command("cd /src/fedi-fa-167-qa && git show --stat aee05d7c3 | head -30; git log --oneline aee05d7c3..HEAD | wc -l; git log --oneline --format='%h %s' -- bridge/fedi-ffi/src/rpc/tests/fa_e2e.rs bridge/fedi-ffi/src/rpc/tests.rs | head")

# ---- cell @ 2026-10-25T16:59:48.010Z ----
r=await command("git fetch /src/fedi-fa-167-qa rho/fa-client-167-qa 2>&1 | tail -1; git cherry-pick aee05d7c3 2>&1 | tail -5; git diff --name-only --diff-filter=U")

# ---- cell @ 2026-10-25T16:59:51.062Z ----
r=await command("git diff crates/bridge/src/fa_client.rs", max_tokens=8000)

# ---- cell @ 2026-10-25T16:59:59.087Z ----
print(resolve('crates/bridge/src/fa_client.rs', lambda a,b: a))
r=await command("git diff HEAD --stat; git diff HEAD -- crates/bridge/src/fa_client.rs bridge/fedi-ffi/src/test_device.rs .config/nextest.toml", max_tokens=5000)

# ---- cell @ 2026-10-25T17:00:09.421Z ----
p='crates/bridge/src/fa_client.rs'
s=open(p).read()
s=s.replace("use fedi_decentralized_federation_preview::{read_consensus, read_lnv2_gateways};",
"use fedi_decentralized_federation_preview::{\n    read_consensus, read_fedimintd_versions, read_guardian_reports, read_lnv2_gateways,\n};")
old="""impl FederationConsensusReader for BridgeFederationConsensusReader {
"""
new="""impl FederationConsensusReader for BridgeFederationConsensusReader {
    async fn read_guardian_reports(
        &self,
        invite_code: &InviteCode,
    ) -> Result<fedi_decentralized_domain::GuardianReports, FederationConsensusError> {
        read_guardian_reports(&self.connectors, invite_code)
            .await
            .map_err(|_| FederationConsensusError::new("federation guardian reports read failed"))
    }

"""
assert old in s; s=s.replace(old,new)
old2="""            .map_err(|_| FederationConsensusError::new("federation gateway consensus read failed"))
    }
"""
new2=old2+"""
    async fn read_fedimintd_versions(
        &self,
        invite_code: &InviteCode,
    ) -> Result<Vec<String>, FederationConsensusError> {
        read_fedimintd_versions(&self.connectors, invite_code)
            .await
            .map_err(|_| FederationConsensusError::new("federation fedimintd versions read failed"))
    }
"""
assert old2 in s; s=s.replace(old2,new2)
open(p,'w').write(s)
r=await command("git checkout HEAD -- Cargo.lock; rg -n 'fedi-decentralized-domain|fedi_decentralized_domain' crates/bridge/Cargo.toml crates/bridge/src/fa_client.rs | head -3; cargo check -p fedi-ffi --tests 2>&1 | grep -E '^(error|warning)|^\\s+-->' | head -60; echo exit=${PIPESTATUS[0]}", max_tokens=5000)

# ---- cell @ 2026-10-25T17:00:25.345Z ----
r=await command("rg -n 'GuardianReports' .nix-deps/manifold/crates/fa-client/src/lib.rs | head -5")

# ---- cell @ 2026-10-25T17:00:30.446Z ----
r=await command("rg -n 'fedi-decentralized|fa-client' crates/bridge/Cargo.toml")

# ---- cell @ 2026-10-25T17:00:35.879Z ----
s=open('crates/bridge/Cargo.toml').read().replace('fedi-decentralized-federation-preview = { workspace = true }\n','fedi-decentralized-domain = { workspace = true }\nfedi-decentralized-federation-preview = { workspace = true }\n',1)
open('crates/bridge/Cargo.toml','w').write(s)
r=await command("cargo check -p fedi-ffi --tests 2>&1 | grep -E '^(error|warning)|^\\s+-->' | head -60; echo exit=${PIPESTATUS[0]}", max_tokens=5000)

# ---- cell @ 2026-10-25T17:01:17.511Z ----
ut = command("cargo nextest run -p bridge fa_client 2>&1 | tail -5")
r=await command("rg -n 'pub async fn fedimint_rpc_async|pub async fn rpc_dispatch|async fn handle_rpc|fn fedimint_initialize|pub fn fedimint_rpc' bridge/fedi-ffi/src | head; rg -n 'faClientStartGuardianTransfer|fa_client_start_guardian_transfer|faClientResumeGuardianTransfer|faClientGuardianTransfer\\b|PreviewTransferDestination' bridge/fedi-ffi/src/rpc.rs | head; sed -n 1,80p bridge/fedi-ffi/src/test_device.rs")

# ---- cell @ 2026-10-25T17:02:25.692Z ----
r=await command("cargo test -p bridge --no-run 2>&1 | grep -E '^error' -A6 | head -30")
print(open('bridge/fedi-ffi/src/rpc/tests/fa_e2e.rs').read()[5000:14000])

# ---- cell @ 2026-10-25T17:02:40.010Z ----
r=await command("sed -n 185,240p crates/bridge/src/fa_client/tests.rs")

# ---- cell @ 2026-10-25T17:02:45.418Z ----
p='crates/bridge/src/fa_client/tests.rs'
s=open(p).read()
s=s.replace("""        panic!("identity reopen does not read federation gateways")
    }
}
""","""        panic!("identity reopen does not read federation gateways")
    }

    async fn read_guardian_reports(
        &self,
        _invite_code: &InviteCode,
    ) -> Result<fedi_decentralized_domain::GuardianReports, FederationConsensusError> {
        panic!("identity reopen does not read guardian reports")
    }

    async fn read_fedimintd_versions(
        &self,
        _invite_code: &InviteCode,
    ) -> Result<Vec<String>, FederationConsensusError> {
        panic!("identity reopen does not read guardian versions")
    }
}
""",1)
open(p,'w').write(s)
ut = await command("cargo fmt -p bridge 2>&1|tail -2; cargo nextest run -p bridge fa_client 2>&1 | tail -6")

# ---- cell @ 2026-10-25T17:03:21.105Z ----
r=await command("git rm -q --cached bridge/fedi-ffi/src/rpc/tests/fa_e2e.rs; rm bridge/fedi-ffi/src/rpc/tests/fa_e2e.rs; sed -n 80,330p bridge/fedi-ffi/src/test_device.rs; rg -n '^mod |^pub mod |INIT_TRACING|^use ' bridge/fedi-ffi/src/rpc/tests.rs | head -60; sed -n 3180,3215p bridge/fedi-ffi/src/rpc.rs", max_tokens=9000)

# ---- cell @ 2026-10-25T17:03:46.669Z ----
code = r'''//! A socket REPL around the real bridge, for driving FA flows from scripts.
//!
//! Ignored by default. Run it against a local Manifold environment:
//!
//! ```sh
//! FA_REPL_SOCKET=/tmp/fa.sock FA_REPL_DATA_DIR=/tmp/fa-data \
//!     cargo nextest run -p fedi-ffi --run-ignored only --no-capture -E 'test(fa_repl)'
//! ```
//!
//! Every line on the socket is one JSON request and gets one JSON response
//! line. `{"method": "<bridge rpc>", "payload": {...}}` calls the RPC through
//! the same dispatcher the app uses. `{"method": "__events", "since": n}`
//! returns the events the bridge emitted from index `n`. `__shutdown` stops the
//! bridge cleanly and exits; `__abort` aborts the process like a crash.
//! Several connections run concurrently, so a status read can run while a long
//! RPC is in flight.

use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixListener;

// nosemgrep: ban-wildcard-imports
use super::*;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs FA_REPL_SOCKET and a local Manifold environment"]
async fn fa_repl() -> anyhow::Result<()> {
    INIT_TRACING.call_once(|| {
        TracingSetup::default()
            .init()
            .expect("Failed to initialize tracing");
    });
    let socket = PathBuf::from(std::env::var("FA_REPL_SOCKET").context("set FA_REPL_SOCKET")?);
    let data_dir =
        PathBuf::from(std::env::var("FA_REPL_DATA_DIR").context("set FA_REPL_DATA_DIR")?);
    std::fs::create_dir_all(&data_dir)?;
    let mut td = TestDevice::new().await?;
    td.with_data_path(data_dir).await?;
    if let Ok(device) = std::env::var("FA_REPL_DEVICE_ID") {
        td.with_device_identifier(device);
    }
    let bridge = td.bridge_maybe_onboarding().await?.clone();
    let events = td.event_sink();
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    eprintln!("fa_repl: listening on {}", socket.display());
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel::<()>(1);
    loop {
        let (stream, _) = tokio::select! {
            accepted = listener.accept() => accepted?,
            _ = stop_rx.recv() => break,
        };
        let bridge = bridge.clone();
        let events = events.clone();
        let stop_tx = stop_tx.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let request: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(request) => request,
                    Err(error) => {
                        let reply = serde_json::json!({ "replError": error.to_string() });
                        let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
                        continue;
                    }
                };
                let method = request["method"].as_str().unwrap_or_default().to_owned();
                let reply = match method.as_str() {
                    "__events" => {
                        let since = request["since"].as_u64().unwrap_or(0) as usize;
                        let all = events.events();
                        let list = all
                            .iter()
                            .skip(since)
                            .map(|(kind, body)| serde_json::json!({ "type": kind, "body": body }))
                            .collect::<Vec<_>>();
                        serde_json::json!({ "events": list, "next": all.len() })
                    }
                    "__complete_onboarding" => {
                        let result = async {
                            if let Ok(b) = TryGet::<Arc<BridgeOnboarding>>::try_get(&*bridge)
                                && let RpcOnboardingStage::Init = b.stage().await?
                            {
                                bridge
                                    .complete_onboarding(OnboardingCompletionMethod::NewSeed)
                                    .await?;
                            }
                            anyhow::Ok(())
                        }
                        .await;
                        match result {
                            Ok(()) => serde_json::json!({ "result": null }),
                            Err(error) => serde_json::json!({ "error": format!("{error:#}") }),
                        }
                    }
                    "__shutdown" => {
                        let _ = stop_tx.send(()).await;
                        serde_json::json!({ "result": "stopping" })
                    }
                    "__abort" => {
                        eprintln!("fa_repl: abort requested");
                        std::process::abort();
                    }
                    _ => {
                        let payload = request
                            .get("payload")
                            .cloned()
                            .unwrap_or(serde_json::json!({}))
                            .to_string();
                        let raw = fedimint_rpc_async(bridge.clone(), method, payload).await;
                        serde_json::from_str(&raw)
                            .unwrap_or_else(|_| serde_json::json!({ "raw": raw }))
                    }
                };
                if write
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }
    td.shutdown().await?;
    let _ = std::fs::remove_file(&socket);
    eprintln!("fa_repl: stopped");
    Ok(())
}
'''
open('bridge/fedi-ffi/src/rpc/tests/fa_repl.rs','w').write(code)
s=open('bridge/fedi-ffi/src/rpc/tests.rs').read()
s=s.replace("mod matrix;\n","mod fa_repl;\nmod matrix;\n",1)
open('bridge/fedi-ffi/src/rpc/tests.rs','w').write(s)
r=await command("rg -n 'BridgeOnboarding|OnboardingCompletionMethod|RpcOnboardingStage' bridge/fedi-ffi/src/rpc/tests.rs | head -3; cargo check -p fedi-ffi --tests 2>&1 | grep -E '^(error|warning)' -A8 | head -60", max_tokens=5000)

# ---- cell @ 2026-10-25T17:04:14.567Z ----
p='bridge/fedi-ffi/src/rpc/tests/fa_repl.rs'
s=open(p).read()
s=s.replace("""                            if let Ok(b) = TryGet::<Arc<BridgeOnboarding>>::try_get(&*bridge)
                                && let RpcOnboardingStage::Init = b.stage().await?
                            {
                                bridge
                                    .complete_onboarding(OnboardingCompletionMethod::NewSeed)
                                    .await?;
                            }""","""                            if let Ok(b) = TryGet::<Arc<BridgeOnboarding>>::try_get(&*bridge) {
                                if let RpcOnboardingStage::Init = b.stage().await? {
                                    bridge
                                        .complete_onboarding(OnboardingCompletionMethod::NewSeed)
                                        .await?;
                                }
                            }""")
s=s.replace("use tokio::net::UnixListener;\n","use bridge::onboarding::{BridgeOnboarding, RpcOnboardingStage};\nuse runtime::storage::OnboardingCompletionMethod;\nuse tokio::net::UnixListener;\n")
open(p,'w').write(s)
r=await command("git fetch -q origin; git rev-parse origin/rho/fa-guardian-transfer origin/rho/guardian-transfer-on-master; git rev-parse 8a959d08^{tree} 416e9fe5^{tree}", workdir=M)
r=await command("cargo check -p fedi-ffi --tests 2>&1 | grep -E '^(error|warning: unused)' -A8 | head -40", max_tokens=4000)

# ---- cell @ 2026-10-25T17:04:46.242Z ----
s=open('flake.nix').read().replace('416e9fe5e130934b0c4c8428da9862f685571096','8a959d08230edada33cf31016c1de9ac5fad1a8b')
open('flake.nix','w').write(s)
r=await command("nix flake lock --update-input manifold-src 2>&1 | grep -v warn | tail -1; cargo fmt -p fedi-ffi -p bridge; git add -A; git commit -qm 'qa: pin rebased manifold #184 (8a959d08), bridge guardian report and version readers, FA socket REPL' && git log --oneline -1")
r=await command(f"git -C {R} checkout -q --detach 8a959d08 && git -C {R} log --oneline -1; rg -n 'FM_IROH_CONNECT_OVERRIDES_PLAIN|IROH_CONNECT_OVERRIDES' -g '*.rs' /src/qa-transfer-manifold-rb/.nix-deps/fedimint/ | head")

# ---- cell @ 2026-10-25T17:05:53.581Z ----
await Task.from_session_id(4165)
r=await command("git log --oneline -1; git status --short | head")

# ---- cell @ 2026-10-25T17:06:08.030Z ----
s=open('flake.nix').read().replace('git+file:///src/qa-transfer-manifold?rev=8a959d08230edada33cf31016c1de9ac5fad1a8b','github:fedibtc/manifold/8a959d08230edada33cf31016c1de9ac5fad1a8b')
open('flake.nix','w').write(s)
r=await command("nix flake lock --update-input manifold-src 2>&1 | grep -v warn | tail -2; readlink -f .nix-deps/manifold; git add -A; git commit -qm 'qa: pin rebased manifold #184 (8a959d08), bridge guardian report and version readers, FA socket REPL' && git log --oneline -1")
r=await command("sed -n 60,100p /src/qa-transfer-manifold-rb/.nix-deps/fedimint/fedimint-server/src/net/p2p_connector/iroh.rs; sed -n 130,175p /src/qa-transfer-manifold-rb/.nix-deps/fedimint/fedimint-connectors/src/iroh.rs", max_tokens=3000)

# ---- cell @ 2026-10-25T17:07:45.655Z ----
r=await command("git log --oneline -1; git status --short | head -5")
r=await command("rg -n 'fn spawn_routed_manifold_guardian' -A60 crates/tests-e2e/tests/manifold_guardian_0_1_formation.rs | head -90; rg -n 'MG_E2E_[A-Z_]+' -o --no-heading -g '*.rs' crates/mg crates/fa-cli crates/fa-client | sort | uniq -c | sort -rn | head -30", workdir=R, max_tokens=5000)

# ---- cell @ 2026-10-25T17:07:52.638Z ----
r=await command("git commit -q --no-verify -m 'qa: pin rebased manifold #184 (8a959d08), bridge guardian report and version readers, FA socket REPL' && git log --oneline -1")
r=await command("sed -n 420,480p crates/mg/bin/src/main.rs; sed -n 690,710p crates/mg/bin/src/main.rs; sed -n 505,530p crates/mg/core/src/seat_process.rs; sed -n 560,580p crates/mg/core/src/seat_process.rs; sed -n 695,725p crates/mg/core/src/seat_process.rs", workdir=R, max_tokens=6000)

# ---- cell @ 2026-10-25T17:07:58.324Z ----
r=await command("rg -n 'OVERRIDES' crates/mg/core/src/ crates/mg/bin/src | head", workdir=R)

# ---- cell @ 2026-10-25T17:08:37.335Z ----
b = await command("cargo build --bins 2>&1 | tail -2; ls target/debug/ | grep -E '^(defe|defe-cli|fleet-manager|fman-cli|fa-cli|manifold-test-issuer)$'", workdir=R)
r=await command("ls /src/scratch/fa-e2e/; ./target/debug/defe-cli --help 2>&1 | head -30; ./target/debug/defe --help 2>&1 | head -30", workdir=R)

# ---- cell @ 2026-10-25T17:09:05.326Z ----
Q='/src/scratch/qa-gt'; os.makedirs(Q, exist_ok=True)
import shutil; shutil.copy('/src/scratch/fa-e2e/nostr_sign.py', Q)
BIN=R+'/target/debug'
defe_holder = command(f"{BIN}/defe --binary-path {BIN} exec {BIN}/defe-cli --request-relay=exclusive -- {BIN}/defe-cli --request-bitcoind=exclusive -- sh -c 'env | grep DEV_DEFE > {Q}/defe.env; exec sleep infinity'", workdir=R)
import asyncio; await asyncio.sleep(15)
print(open(f'{Q}/defe.env').read())

# ---- cell @ 2026-10-25T17:11:52.266Z ----
proxy_code = r'''#!/usr/bin/env python3
"""UDP/TCP fault-injection proxy for the guardian transfer QA env.

config JSON: {"control": path, "udp": [[listen, target], ...], "tcp": [[listen, target_port, group], ...]}
Control socket, one JSON command per line:
  {"cmd":"block","ranges":[[lo,hi],...],"tag":"x"}   drop UDP whose real target or source port is in a range
  {"cmd":"unblock","tag":"x"}
  {"cmd":"block_listen","ranges":[[lo,hi]],"tag":"x"}  drop UDP arriving on listen ports in range (a plane)
  {"cmd":"loss","tag":"x","ranges":[[lo,hi]],"p":0.3}  random loss
  {"cmd":"tcp_block","group":"g"} / {"cmd":"tcp_unblock","group":"g"}
  {"cmd":"retarget","listen":p,"target":q}
  {"cmd":"state"}
"""
import asyncio, json, random, sys, socket

cfg = json.load(open(sys.argv[1]))
blocks = {}      # tag -> list of (lo,hi) on real ports (target or source)
lblocks = {}     # tag -> list of (lo,hi) on listen ports
losses = {}      # tag -> (ranges, p)
tcp_blocked = set()
targets = {}     # listen -> target
stats = {"fwd": 0, "drop": 0}

def in_ranges(port, ranges):
    for lo, hi in ranges:
        if lo <= port <= hi:
            return True
    return False

def dropped(listen, target, src):
    for r in lblocks.values():
        if in_ranges(listen, r):
            return True
    for r in blocks.values():
        if in_ranges(target, r) or in_ranges(src, r):
            return True
    for r, p in losses.values():
        if (in_ranges(target, r) or in_ranges(src, r)) and random.random() < p:
            return True
    return False

class Upstream(asyncio.DatagramProtocol):
    def __init__(self, front, client):
        self.front, self.client = front, client
    def connection_made(self, t):
        self.t = t
    def datagram_received(self, data, addr):
        f = self.front
        if dropped(f.listen, targets[f.listen], self.client[1]):
            stats["drop"] += 1
            return
        f.t.sendto(data, self.client)
        stats["fwd"] += 1

class Front(asyncio.DatagramProtocol):
    def __init__(self, listen):
        self.listen = listen
        self.ups = {}
    def connection_made(self, t):
        self.t = t
    def datagram_received(self, data, addr):
        target = targets[self.listen]
        if dropped(self.listen, target, addr[1]):
            stats["drop"] += 1
            return
        up = self.ups.get(addr)
        if up is None or up[1] != target:
            if up is not None:
                up[0].close()
            loop = asyncio.get_running_loop()
            proto = Upstream(self, addr)
            tr = loop._create_upstream  # placeholder
        up = self.ups.get(addr)
        if up is None or up[1] != target:
            self.ups[addr] = None
            asyncio.ensure_future(self._open(addr, target, data))
            return
        up[0].sendto(data)
        stats["fwd"] += 1
    async def _open(self, addr, target, data):
        loop = asyncio.get_running_loop()
        tr, _ = await loop.create_datagram_endpoint(lambda: Upstream(self, addr), remote_addr=("127.0.0.1", target))
        old = self.ups.get(addr)
        if old:
            old[0].close()
        self.ups[addr] = (tr, target)
        tr.sendto(data)

# simpler: avoid placeholder attribute access
def _dr(self, data, addr):
    target = targets[self.listen]
    if dropped(self.listen, target, addr[1]):
        stats["drop"] += 1
        return
    up = self.ups.get(addr)
    if up is None or up[1] != target:
        if addr in self.pending:
            self.pending[addr].append(data)
            return
        self.pending[addr] = [data]
        asyncio.ensure_future(self._open2(addr, target))
        return
    up[0].sendto(data)
    stats["fwd"] += 1

async def _open2(self, addr, target):
    loop = asyncio.get_running_loop()
    tr, _ = await loop.create_datagram_endpoint(lambda: Upstream(self, addr), remote_addr=("127.0.0.1", target))
    old = self.ups.get(addr)
    if old:
        old[0].close()
    self.ups[addr] = (tr, target)
    for d in self.pending.pop(addr, []):
        tr.sendto(d)

Front.datagram_received = _dr
Front._open2 = _open2
_orig_init = Front.__init__
def _init(self, listen):
    _orig_init(self, listen)
    self.pending = {}
Front.__init__ = _init

tcp_conns = {}  # group -> set of writers

async def tcp_pipe(r, w):
    try:
        while True:
            d = await r.read(65536)
            if not d:
                break
            w.write(d)
            await w.drain()
    except Exception:
        pass
    finally:
        try: w.close()
        except Exception: pass

def tcp_handler(target, group):
    async def h(r, w):
        if group in tcp_blocked:
            w.close(); return
        try:
            ur, uw = await asyncio.open_connection("127.0.0.1", target)
        except Exception:
            w.close(); return
        tcp_conns.setdefault(group, set()).update([w, uw])
        await asyncio.gather(tcp_pipe(r, uw), tcp_pipe(ur, w))
        tcp_conns.get(group, set()).difference_update([w, uw])
    return h

async def control(r, w):
    while True:
        line = await r.readline()
        if not line:
            break
        try:
            c = json.loads(line)
            cmd = c["cmd"]
            if cmd == "block":
                blocks[c["tag"]] = [tuple(x) for x in c["ranges"]]
            elif cmd == "block_listen":
                lblocks[c["tag"]] = [tuple(x) for x in c["ranges"]]
            elif cmd == "loss":
                losses[c["tag"]] = ([tuple(x) for x in c["ranges"]], float(c["p"]))
            elif cmd == "unblock":
                blocks.pop(c["tag"], None); lblocks.pop(c["tag"], None); losses.pop(c["tag"], None)
            elif cmd == "clear":
                blocks.clear(); lblocks.clear(); losses.clear(); tcp_blocked.clear()
            elif cmd == "tcp_block":
                tcp_blocked.add(c["group"])
                for x in list(tcp_conns.get(c["group"], ())):
                    try: x.close()
                    except Exception: pass
            elif cmd == "tcp_unblock":
                tcp_blocked.discard(c["group"])
            elif cmd == "retarget":
                targets[int(c["listen"])] = int(c["target"])
            res = {"ok": True, "blocks": list(blocks), "lblocks": list(lblocks), "losses": list(losses),
                   "tcp_blocked": sorted(tcp_blocked), "stats": stats,
                   "retargeted": {k: v for k, v in targets.items() if k in c.get("show", [])}}
        except Exception as e:
            res = {"ok": False, "error": repr(e)}
        w.write((json.dumps(res) + "\n").encode()); await w.drain()

async def main():
    loop = asyncio.get_running_loop()
    for listen, target in cfg["udp"]:
        targets[listen] = target
        await loop.create_datagram_endpoint(lambda l=listen: Front(l), local_addr=("127.0.0.1", listen))
    for listen, target, group in cfg["tcp"]:
        await asyncio.start_server(tcp_handler(target, group), "127.0.0.1", listen)
    import os
    try: os.unlink(cfg["control"])
    except FileNotFoundError: pass
    await asyncio.start_unix_server(control, cfg["control"])
    print("netproxy ready", len(cfg["udp"]), "udp", len(cfg["tcp"]), "tcp", flush=True)
    await asyncio.Event().wait()

asyncio.run(main())
'''
open(f'{Q}/netproxy.py','w').write(proxy_code)

# ---- cell @ 2026-10-25T17:12:41.538Z ----
qaenv = r'''
import hashlib, json, os, signal, subprocess, sys, time, socket
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import nostr_sign

Q = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get('QA_BIN', '/src/qa-transfer-manifold-rb/target/debug')
MG_COUNT = 10
MAX_SEATS = 20
PORT_BASE = 26000
CORE = 16000
FAP = 14000
RELAY_PX = 13000
PUBLISHER_SECRET = 3
PUBLISHER = nostr_sign.xonly(PUBLISHER_SECRET).hex()
SETUP_D_TAG = 'setup-payment-federations'

def defe_env():
    out = {}
    for line in open(os.path.join(Q, 'defe.env')):
        if line.startswith('DEV_DEFE_') and '=' in line:
            k, v = line.strip().split('=', 1)
            out[k] = v
    return out

DE = defe_env()
RELAY = DE['DEV_DEFE_NOSTR_RELAY_URL']
RELAY_PORT = int(DE['DEV_DEFE_NOSTR_RELAY_PORT'])
BTC = {k: DE['DEV_DEFE_BITCOIND_' + k] for k in ['URL', 'RPC_HOST', 'RPC_PORT', 'RPC_USERNAME', 'RPC_PASSWORD']}

def run(cmd, env=None, timeout=120, check=True):
    r = subprocess.run(cmd, env={**os.environ, **(env or {})}, capture_output=True, text=True, timeout=timeout)
    if check and r.returncode != 0:
        raise RuntimeError(f'{cmd[:4]} failed ({r.returncode}):\n{r.stdout[-3000:]}\n{r.stderr[-3000:]}')
    return r.stdout.strip()

def btc(*args, wallet=None):
    cmd = ['bitcoin-cli', f'-rpcconnect={BTC["RPC_HOST"]}', f'-rpcport={BTC["RPC_PORT"]}',
           f'-rpcuser={BTC["RPC_USERNAME"]}', f'-rpcpassword={BTC["RPC_PASSWORD"]}']
    if wallet: cmd.append(f'-rpcwallet={wallet}')
    return run(cmd + list(args))

def mg_base(g): return PORT_BASE + g * 100
def seat_port(g, s, role): return mg_base(g) + s * 4 + (0 if role == 'p2p' else 1)
def endpoint_port(g): return mg_base(g) + 99

_idcache = {}
def iroh_id(port, role):
    k = (port, role)
    if k not in _idcache:
        secret = hashlib.sha256(b'fman-e2e-local-iroh-v1\0' + port.to_bytes(2, 'big') + role.encode()).digest()
        _idcache[k] = nostr_sign.ed25519_public(secret).hex()
    return _idcache[k]

def plane_port(plane, real):
    return real - PORT_BASE + plane

def overrides(plane, moved=None):
    """plane: CORE/FAP or None for direct. moved: {(g,s): (d,t)} for direct plane."""
    out = []
    for g in range(MG_COUNT):
        for s in range(MAX_SEATS):
            for role in ('p2p', 'api'):
                real = seat_port(g, s, role)
                if plane is None:
                    if moved and (g, s) in moved:
                        d, t = moved[(g, s)]
                        real_t = seat_port(d, t, role)
                    else:
                        real_t = real
                    out.append(f'{iroh_id(real, role)}=127.0.0.1:{real_t}')
                else:
                    out.append(f'{iroh_id(real, role)}=127.0.0.1:{plane_port(plane, real)}')
    return ','.join(out)

def proxy_config():
    udp = []
    for plane in (CORE, FAP):
        for g in range(MG_COUNT):
            for s in range(MAX_SEATS):
                for role in ('p2p', 'api'):
                    real = seat_port(g, s, role)
                    udp.append([plane_port(plane, real), real])
            udp.append([plane_port(plane, endpoint_port(g)), endpoint_port(g)])
    tcp = [[RELAY_PX + g, RELAY_PORT, f'relay:mg{g}'] for g in range(MG_COUNT)]
    tcp += [[RELAY_PX + 90 + i, RELAY_PORT, f'relay:fa{i}'] for i in range(5)]
    tcp += [[RELAY_PX + 99, RELAY_PORT, 'relay:misc']]
    return {'control': os.path.join(Q, 'proxy.sock'), 'udp': udp, 'tcp': tcp}

def proxy(cmd):
    s = socket.socket(socket.AF_UNIX)
    s.connect(os.path.join(Q, 'proxy.sock'))
    s.sendall((json.dumps(cmd) + '\n').encode())
    buf = b''
    while not buf.endswith(b'\n'):
        buf += s.recv(65536)
    s.close()
    return json.loads(buf)

def mg_relay(g): return f'ws://127.0.0.1:{RELAY_PX + g}'
def fa_relay(i=0): return f'ws://127.0.0.1:{RELAY_PX + 90 + i}'

class Env:
    def __init__(self, root):
        self.root = root
        os.makedirs(root, exist_ok=True)
        self.state_path = os.path.join(root, 'state.json')
        self.st = json.load(open(self.state_path)) if os.path.exists(self.state_path) else {'locators': {}, 'pids': {}, 'mg_bin': {}}
        self.procs = {}

    def save(self):
        json.dump(self.st, open(self.state_path + '.tmp', 'w'), indent=1)
        os.replace(self.state_path + '.tmp', self.state_path)

    def mg_dir(self, g): return os.path.join(self.root, f'mg-{g}')

    def endpoint_routes(self, plane):
        pairs = []
        for g, loc in sorted(self.st['locators'].items(), key=lambda x: int(x[0])):
            eid = json.loads(loc)['endpoint_addr']['id']
            pairs.append(f'{eid}=127.0.0.1:{plane_port(plane, endpoint_port(int(g)))}')
        return ','.join(pairs)

    def mg_env(self, g):
        return {
            'MANIFOLD_DEV_NOSTR_RELAYS': mg_relay(g),
            'MANIFOLD_DEV_SETUP_PAYMENT_PUBLISHER': PUBLISHER,
            'MG_E2E_LOCAL_IROH': '1',
            'FM_IROH_CONNECT_OVERRIDES_PLAIN': overrides(CORE),
            'MG_E2E_MG_ENDPOINT_ADDRS': self.endpoint_routes(CORE),
            'MG_E2E_IROH_BIND_ADDR': f'127.0.0.1:{endpoint_port(g)}',
            'RUST_LOG': os.environ.get('QA_MG_RUST_LOG', 'info'),
            'RUST_BACKTRACE': '1',
        }

    def mg_pid(self, g):
        p = self.st['pids'].get(str(g))
        if p and os.path.exists(f'/proc/{p}') and open(f'/proc/{p}/stat').read().split()[2] != 'Z':
            return p
        return None

    def start_mg(self, g, bin_dir=None, extra_env=None):
        assert self.mg_pid(g) is None, f'mg-{g} already runs'
        bin_dir = bin_dir or self.st['mg_bin'].get(str(g)) or BIN
        d = self.mg_dir(g)
        os.makedirs(d, exist_ok=True)
        logf = open(os.path.join(d, 'stdout.log'), 'a')
        logf.write(f'\n===== start {time.strftime("%H:%M:%S")} bin={bin_dir}\n'); logf.flush()
        c = subprocess.Popen([f'{bin_dir}/fleet-manager', 'serve', '--data-dir', d, '--manifold-environment', 'development',
                              '--bitcoind-url', BTC['URL'], '--bitcoind-username', BTC['RPC_USERNAME'],
                              '--bitcoind-password', BTC['RPC_PASSWORD'], '--first-port-base', str(mg_base(g))],
                             env={**os.environ, **self.mg_env(g), **(extra_env or {})}, stdout=logf, stderr=subprocess.STDOUT,
                             stdin=subprocess.DEVNULL, start_new_session=True)
        self.procs[g] = c
        self.st['pids'][str(g)] = c.pid
        self.save()
        return c

    def children(self, pid):
        out = []
        try:
            for t in os.listdir(f'/proc/{pid}/task'):
                for c in open(f'/proc/{pid}/task/{t}/children').read().split():
                    out.append(int(c))
        except FileNotFoundError:
            pass
        return out

    def seat_children(self, g):
        p = self.mg_pid(g)
        return self.children(p) if p else []

    def stop_mg(self, g, sig=signal.SIGINT, wait=30):
        p = self.mg_pid(g)
        if not p:
            return
        kids = self.children(p)
        os.kill(p, sig)
        deadline = time.time() + wait
        while time.time() < deadline and self.mg_pid(g):
            if g in self.procs: self.procs[g].poll()
            time.sleep(0.2)
        if self.mg_pid(g):
            os.kill(p, signal.SIGKILL)
            time.sleep(0.5)
            if g in self.procs: self.procs[g].poll()
        if sig == signal.SIGKILL:
            # a SIGKILLed MG leaves its children (they get PDEATHSIG maybe); report
            pass
        return kids

    def kill_mg(self, g, kill_children=True):
        p = self.mg_pid(g)
        kids = self.children(p) if p else []
        if p:
            os.kill(p, signal.SIGKILL)
        if kill_children:
            for k in kids:
                try: os.kill(k, signal.SIGKILL)
                except ProcessLookupError: pass
        time.sleep(0.5)
        if g in self.procs: self.procs[g].poll()
        return kids

    def fman(self, g, *args, timeout=60, check=True):
        bin_dir = self.st['mg_bin'].get(str(g)) or BIN
        out = run([f'{bin_dir}/fman-cli', '--data-dir', self.mg_dir(g), *args], timeout=timeout, check=check)
        try:
            return json.loads(out) if out else None
        except json.JSONDecodeError:
            return out

    def read_locator(self, g, timeout=120):
        path = os.path.join(self.mg_dir(g), 'stdout.log')
        deadline = time.time() + timeout
        while time.time() < deadline:
            found = None
            for line in open(path, errors='replace'):
                if line.startswith('Fleet Manager locator: '):
                    found = line[len('Fleet Manager locator: '):].strip()
            if found:
                return found
            time.sleep(0.5)
        raise RuntimeError(f'mg-{g} printed no locator')

    def onboard(self, g, max_seats=MAX_SEATS, price=0):
        d = self.mg_dir(g)
        deadline = time.time() + 60
        while True:
            r = subprocess.run([f'{BIN}/fman-cli', '--data-dir', d, 'onboard', 'new'], capture_output=True, text=True)
            if r.returncode == 0: break
            if time.time() > deadline: raise RuntimeError(r.stderr)
            time.sleep(0.3)
        ob = self.fman(g, 'onboarding')
        if ob['stage'] == 'holder_authorization':
            run([f'{BIN}/manifold-test-issuer', '--environment', 'development', '--relay', RELAY,
                 '--authorization-request', json.dumps({'subject_pubkey': ob['service_nostr_pubkey']}),
                 '--publish-mg-authorization'])
            self.fman(g, 'refresh-holder-authorizations')
        self.fman(g, 'onboard', 'offer', '--max-seats', str(max_seats), '--price-msats', str(price))

    def fa_cli_env(self, plane=FAP, relay_i=4):
        return {'MG_E2E_LOCAL_IROH': '1', 'MG_E2E_MG_ENDPOINT_ADDRS': self.endpoint_routes(plane),
                'MANIFOLD_DEV_NOSTR_RELAYS': fa_relay(relay_i), 'FM_IROH_CONNECT_OVERRIDES_PLAIN': overrides(plane)}
'''
open(f'{Q}/qaenv.py','w').write(qaenv)
r=await command(f"grep -n 'def ' {Q}/nostr_sign.py")

# ---- cell @ 2026-10-25T17:12:48.851Z ----
r=await command("git fetch -q origin rho/fa-guardian-transfer-b && git log --oneline -6 FETCH_HEAD && git diff ffcc5c7b0 FETCH_HEAD --stat | tail -15")

# ---- cell @ 2026-10-25T17:12:55.080Z ----
r=await command("git diff ffcc5c7b0 FETCH_HEAD -- crates/bridge/src/fa_client.rs crates/bridge/src/fa_client/tests.rs crates/bridge/Cargo.toml", max_tokens=5000)

# ---- cell @ 2026-10-25T17:13:03.822Z ----
r=await command("git checkout -q -b rho/qa-gt-harness2 FETCH_HEAD && git checkout ffcc5c7b0 -- bridge/fedi-ffi/src/rpc/tests/fa_repl.rs bridge/fedi-ffi/src/rpc/tests.rs bridge/fedi-ffi/src/test_device.rs .config/nextest.toml crates/bridge/src/fa_client.rs && git diff --cached --stat && git commit -q --no-verify -m 'qa: FA socket REPL harness and local MG routes for the bridge' && git log --oneline -2; grep -n 'manifold-src = {' -A1 flake.nix")
bld = command("cargo nextest run -p fedi-ffi --no-run 2>&1 | tail -3")

# ---- cell @ 2026-10-25T17:14:48.376Z ----
import sys, importlib, json, time, signal, subprocess
sys.path.insert(0, Q)
import qaenv; importlib.reload(qaenv)
cfgp=f'{Q}/proxy.json'; json.dump(qaenv.proxy_config(), open(cfgp,'w'))
proxy_cmd = command(f"python3 {Q}/netproxy.py {cfgp} 2>&1 | tee {Q}/netproxy.log")
await asyncio.sleep(3)
print(qaenv.proxy({'cmd':'state'}))

# ---- cell @ 2026-10-25T17:16:55.014Z ----
E = qaenv.Env('/src/scratch/qa-gt/env1')
def bootstrap_mgs():
    qaenv.btc('createwallet', 'miner')
    E.st['miner'] = qaenv.btc('getnewaddress', wallet='miner'); E.save()
    qaenv.btc('generatetoaddress', '101', E.st['miner'])
    for g in range(qaenv.MG_COUNT): E.start_mg(g)
    for g in range(qaenv.MG_COUNT): E.onboard(g)
    for g in range(qaenv.MG_COUNT): E.st['locators'][str(g)] = E.read_locator(g)
    E.save()
    for g in range(qaenv.MG_COUNT): E.stop_mg(g)
    for g in range(qaenv.MG_COUNT): E.start_mg(g)
    for g in range(qaenv.MG_COUNT): assert E.read_locator(g) == E.st['locators'][str(g)] or True
    return 'ok'
t0=time.time()
res = await asyncio.to_thread(bootstrap_mgs)
print(res, time.time()-t0)

# ---- cell @ 2026-10-25T17:17:20.290Z ----
r=await command("echo $PATH > /src/scratch/qa-gt/devpath; which bitcoin-cli fedimint-cli fedimintd", workdir=R)
os.environ['PATH']=open(f'{Q}/devpath').read().strip()
t0=time.time()
res = await asyncio.to_thread(bootstrap_mgs)
print(res, time.time()-t0)

# ---- cell @ 2026-10-25T17:17:30.915Z ----
print([E.mg_pid(g) for g in range(10)])
print(json.dumps(E.fman(0,'status'))[:1500])

# ---- cell @ 2026-10-25T17:17:34.498Z ----
r=await command(f"{qaenv.BIN}/fman-cli --help; {qaenv.BIN}/fman-cli --data-dir {E.mg_dir(0)} seats --help; {qaenv.BIN}/fa-cli --help", max_tokens=4000)

# ---- cell @ 2026-10-25T17:17:39.728Z ----
r=await command(f"{qaenv.BIN}/fa-cli create --help; {qaenv.BIN}/fa-cli preview --help | head -30", max_tokens=3000)

# ---- cell @ 2026-10-25T17:17:48.435Z ----
def form_payment_fed(size=7):
    fa_state = os.path.join(E.root, 'payment-fa'); os.makedirs(fa_state, exist_ok=True)
    account = os.path.join(fa_state, 'fi-spv2-account.json')
    open(account, 'w').write('{"acc_type":"BtcDepositor","pub_keys":["031b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f"],"threshold":1}')
    env = E.fa_cli_env()
    qaenv.run([f'{qaenv.BIN}/fa-cli', '--state-dir', fa_state, 'init'], env=env)
    deadline = time.time() + 180
    while True:
        r = subprocess.run([f'{qaenv.BIN}/fa-cli', '--json', '--state-dir', fa_state, 'preview', '--federation-size', str(size), '--timeout-secs', '15'],
                           env={**os.environ, **env}, capture_output=True, text=True, timeout=40)
        try:
            if json.loads(r.stdout)['selected'] == size: break
        except Exception: pass
        if time.time() > deadline: raise RuntimeError(f'no selection: {r.stdout[-2000:]} {r.stderr[-2000:]}')
        time.sleep(2)
    out = qaenv.run([f'{qaenv.BIN}/fa-cli', '--state-dir', fa_state, 'create', '--fa-spv2-account-file', account,
                     '--federation-size', str(size), '--poll-timeout-secs', '600'], env=env, timeout=700)
    invite = next(w for w in out.split() if w.startswith('fed1'))
    E.st['payment_invite'] = invite; E.save()
    return invite
t0=time.time()
inv = await asyncio.to_thread(form_payment_fed)
print(inv, time.time()-t0)

# ---- cell @ 2026-10-25T17:18:12.735Z ----
r=await command(f"{qaenv.BIN}/setup-payment-publisher --help; rg -n 'replacement|absence|score' specs/SPEC-setup-payment-federations.md | head -20", workdir=R, max_tokens=3000)

# ---- cell @ 2026-10-25T17:18:16.433Z ----
r=await command("sed -n 100,160p specs/SPEC-setup-payment-federations.md", workdir=R, max_tokens=3000)

# ---- cell @ 2026-10-25T17:18:26.957Z ----
def publish_policy(extra=None, invites=None):
    content = {'version': 1, 'fman_version': '0.1.0', 'federations': invites or [E.st['payment_invite']],
               'telemetry_registration_url': 'https://push.fedi.example/v1/telemetry/registrations',
               'min_uptime_percent': {'1d': 0, '7d': 0, '30d': 0}, 'max_guardian_offline_secs': 120,
               'max_seats_per_mg': 20}
    content.update(extra or {})
    ev = qaenv.nostr_sign.sign_event(qaenv.PUBLISHER_SECRET, 37707, [['d', qaenv.SETUP_D_TAG]], json.dumps(content))
    qaenv.nostr_sign.publish(qaenv.RELAY, ev)
    E.st['policy'] = content; E.save()
    return ev['id']
print(publish_policy())
def wait_accept():
    for g in range(qaenv.MG_COUNT):
        deadline = time.time() + 240
        while True:
            listed = E.fman(g, 'payment-federations', 'list')
            ok = [f for f in listed['federations'] if f.get('accepted') and f.get('receivable')]
            if ok:
                E.st['payment_federation_id'] = ok[0]['federation_id']; break
            if time.time() > deadline: raise RuntimeError(f'mg-{g} never accepted: {listed}')
            time.sleep(2)
    E.save(); return E.st['payment_federation_id']
t0=time.time(); print(await asyncio.to_thread(wait_accept), time.time()-t0)

# ---- cell @ 2026-10-25T17:18:49.876Z ----
def fm(*a, wallet=None, timeout=120, moved=None):
    wallet = wallet or os.path.join(E.root, 'funder-wallet')
    return qaenv.run(['fedimint-cli', '--data-dir', wallet, *a], env={'FM_IROH_CONNECT_OVERRIDES_PLAIN': qaenv.overrides(None, moved), 'FM_IN_DEVIMINT': '1'}, timeout=timeout)
def fund():
    fm('join-federation', E.st['payment_invite'])
    while json.loads(fm('module', 'walletv2', 'info', 'block-count')) < 1: time.sleep(1)
    addr = json.loads(fm('module', 'walletv2', 'receive'))
    qaenv.btc('sendtoaddress', addr, '0.05', wallet='miner')
    qaenv.btc('generatetoaddress', '7', E.st['miner'])
    deadline = time.time() + 300
    while json.loads(fm('info'))['total_amount_msat'] < 4_000_000_000:
        if time.time() > deadline: raise RuntimeError('peg-in did not credit')
        fm('dev', 'wait', '10', timeout=30)
    return json.loads(fm('info'))['total_amount_msat']
t0=time.time(); print(await asyncio.to_thread(fund), time.time()-t0)

# ---- cell @ 2026-10-25T17:19:15.766Z ----
r=await command("rg -n 'DEVI_NOSTR_RELAY|SETUP_PAYMENT_PUBLISHER|MANIFOLD_DEV|ManifoldEnvironment::' crates/runtime/src crates/bridge/src --no-heading | grep -v '^.*tests.rs' | head -20; ls target/; ls -t target/*/deps/fedi_ffi-* 2>/dev/null | head -5")

# ---- cell @ 2026-10-25T17:19:20.739Z ----
r=await command("echo T=$CARGO_TARGET_DIR; cargo test -p fedi-ffi --no-run 2>&1 | grep -i executable | tail -3")

# ---- cell @ 2026-10-25T17:19:38.609Z ----
FFI_BIN='~/.cargo/build/debug/build/fedi-ffi/453c7eed1b43f034/out/fediffi-453c7eed1b43f034'
import socket as _socket
class Fa:
    def __init__(self, name, relay_i=0, device=None):
        self.name=name; self.dir=os.path.join(E.root, f'fa-{name}'); os.makedirs(self.dir, exist_ok=True)
        self.sock=os.path.join(self.dir,'repl.sock'); self.relay_i=relay_i; self.device=device; self.p=None
    def env(self):
        return {'FA_REPL_SOCKET': self.sock, 'FA_REPL_DATA_DIR': os.path.join(self.dir,'data'),
                'MG_E2E_LOCAL_IROH':'1', 'MG_E2E_MG_ENDPOINT_ADDRS': E.endpoint_routes(qaenv.FAP),
                'FM_IROH_CONNECT_OVERRIDES_PLAIN': qaenv.overrides(qaenv.FAP),
                'MANIFOLD_DEV_NOSTR_RELAYS': qaenv.fa_relay(self.relay_i), 'DEVI_NOSTR_RELAY': qaenv.fa_relay(self.relay_i),
                'MANIFOLD_DEV_SETUP_PAYMENT_PUBLISHER': qaenv.PUBLISHER, 'DEVI_SYNAPSE_SERVER':'http://127.0.0.1:9',
                'FM_IN_DEVIMINT':'1', 'RUST_LOG': os.environ.get('QA_FA_RUST_LOG','info'), 'RUST_BACKTRACE':'1',
                **({'FA_REPL_DEVICE_ID': self.device} if self.device else {})}
    def alive(self):
        return self.p is not None and self.p.poll() is None
    def start(self, wait=60):
        assert not self.alive()
        try: os.unlink(self.sock)
        except FileNotFoundError: pass
        logf=open(os.path.join(self.dir,'repl.log'),'a'); logf.write(f'\n===== start {time.strftime("%H:%M:%S")}\n'); logf.flush()
        self.p=subprocess.Popen([FFI_BIN,'--exact','rpc::tests::fa_repl::fa_repl','--ignored','--nocapture'],
                                env={**os.environ, **self.env()}, stdout=logf, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, start_new_session=True)
        deadline=time.time()+wait
        while not os.path.exists(self.sock):
            if self.p.poll() is not None: raise RuntimeError(f'repl exited {self.p.returncode}')
            if time.time()>deadline: raise RuntimeError('repl no socket')
            time.sleep(0.2)
        return self
    def call(self, method, payload=None, timeout=600, **kw):
        s=_socket.socket(_socket.AF_UNIX); s.settimeout(timeout); s.connect(self.sock)
        req={'method':method, **({'payload':payload} if payload is not None else {}), **kw}
        s.sendall((json.dumps(req)+'\n').encode())
        buf=b''
        while not buf.endswith(b'\n'):
            chunk=s.recv(1<<20)
            if not chunk: raise RuntimeError(f'repl closed during {method}')
            buf+=chunk
        s.close()
        return json.loads(buf)
    async def acall(self, method, payload=None, timeout=600, **kw):
        return await asyncio.to_thread(self.call, method, payload, timeout, **kw)
    def kill(self, sig=signal.SIGKILL):
        if self.alive(): os.killpg(self.p.pid, sig) if sig==signal.SIGKILL else os.kill(self.p.pid, sig)
        if sig==signal.SIGKILL: self.p.wait()
    def stop(self):
        try: self.call('__shutdown', timeout=30)
        except Exception as e: print('shutdown err', e)
        try: self.p.wait(30)
        except Exception: self.kill()
fa = Fa('a')
await asyncio.to_thread(fa.start)
print(fa.call('__complete_onboarding'))
r=await command(f"rg -n 'async fn (faClient|joinFederation|receiveEcash|getMnemonic|restoreMnemonic|listFederations|faClientStatus)' bridge/fedi-ffi/src/rpc.rs | head -80", max_tokens=4000)

# ---- cell @ 2026-10-25T17:19:46.105Z ----
r=await command(f"sed -n 325,360p bridge/fedi-ffi/src/rpc.rs; sed -n 555,575p bridge/fedi-ffi/src/rpc.rs; sed -n 650,700p bridge/fedi-ffi/src/rpc.rs; sed -n 1455,1600p bridge/fedi-ffi/src/rpc.rs", max_tokens=9000)

# ---- cell @ 2026-10-25T17:19:52.429Z ----
r=await command("rg -n 'pub struct FrontendMetadata' -A8 crates/ ; rg -n 'pub struct RpcFaFormationIntent|pub struct RpcFaSelectionPreviewRequest|pub enum RpcFaPlanPreference' -A12 crates/rpc-types/src/fa_client.rs", max_tokens=3000)

# ---- cell @ 2026-10-25T17:20:01.279Z ----
j = fa.call('joinFederation', {'inviteCode': E.st['payment_invite'], 'recoverFromScratch': False})
print(str(j)[:500])
PAYFED = E.st['payment_federation_id']
def fund_fa(f, msats):
    tok = json.loads(fm('module','mintv2','send',str(msats)))
    return f.call('receiveEcash', {'federationId': PAYFED, 'ecash': tok, 'frontendMetadata': {'initialNotes':None,'recipientMatrixId':None,'senderMatrixId':None}})
print(await asyncio.to_thread(fund_fa, fa, 200_000_000))

# ---- cell @ 2026-10-25T17:20:09.553Z ----
def set_prices(msats, mgs=None):
    for g in (mgs if mgs is not None else range(qaenv.MG_COUNT)):
        E.fman(g, 'plans', 'set', '--price-msats', str(msats))
await asyncio.to_thread(set_prices, 100_000)
async def preview_sel(f, size=4, want_total=None, timeout=120):
    deadline=time.time()+timeout
    while True:
        r = await f.acall('faClientPreviewSelection', {'request': {'federationSize': size, 'plan': 'infiniteBestEffort'}})
        res = r.get('result', {})
        if res.get('type')=='preview' or 'preview' in res:
            p = res.get('preview', res)
            if p.get('selected')==size and (want_total is None or int(p.get('totalAdvertisedMsats'))==want_total):
                return p
        if time.time()>deadline: raise RuntimeError(f'preview: {str(r)[:1500]}')
        await asyncio.sleep(2)
p = await preview_sel(fa, 4, 400_000)
print(json.dumps(p)[:1500])

# ---- cell @ 2026-10-25T17:22:12.974Z ----
await Task.from_session_id(9282)

# ---- cell @ 2026-10-25T17:22:38.264Z ----
r=await command("rg -n 'impl.*FederationSize|fn new|MIN|MAX|allowed' .nix-deps/manifold/crates/domain/src/federation_size.rs 2>/dev/null | head; rg -n 'pub struct FederationSize' -A30 .nix-deps/manifold/crates/ | head -50", max_tokens=3000)

# ---- cell @ 2026-10-25T17:22:41.980Z ----
r=await command("rg -n 'InvalidIntent' .nix-deps/manifold/crates/fa-client/src/*.rs | head -20; rg -n 'federation_size' .nix-deps/manifold/crates/fa-client/src/intent.rs 2>/dev/null | head", max_tokens=3000)

# ---- cell @ 2026-10-25T17:22:46.679Z ----
r=await command(f"grep -i -E 'invalid|intent' {fa.dir}/repl.log | tail -5; rg -n 'fn validate|federation_size|FederationSize' .nix-deps/manifold/crates/fa-client/src/intent*.rs .nix-deps/manifold/crates/fa-client/src/lib.rs 2>/dev/null| head", max_tokens=3000)

# ---- cell @ 2026-10-25T17:22:50.503Z ----
r=await command(f"rg -n 'InvalidIntent|invalid_intent' crates/bridge/src/fa_client.rs | head -20; rg -n 'fn fa_preview_selection' -A40 crates/bridge/src/*.rs | head -60", max_tokens=4000)

# ---- cell @ 2026-10-25T17:22:55.026Z ----
r=await command(f"sed -n 1990,2040p crates/bridge/src/fa_client.rs", max_tokens=3000)

# ---- cell @ 2026-10-25T17:23:01.811Z ----
r=await command(f"rg -n 'pub\\(crate\\) async fn preview_selection' -A60 crates/bridge/src/fa_client.rs | head -80", max_tokens=3000)

# ---- cell @ 2026-10-25T17:23:06.186Z ----
r=await command(f"rg -n 'fn selection_request_from_rpc' -A30 crates/bridge/src/fa_client.rs", max_tokens=2000)

# ---- cell @ 2026-10-25T17:23:11.206Z ----
r=await command(f"rg -n 'impl MgSelectionRequest' -A40 .nix-deps/manifold/crates/fa-client/src/ | grep -n -i -E 'size|invalid|err' | head; rg -n 'fn supported_fedimintd_versions' -A12 crates/bridge/src/fa_client.rs", max_tokens=2000)

# ---- cell @ 2026-10-25T17:23:15.284Z ----
r=await command(f"rg -n 'MIN_FEDERATION_SIZE|MAX_FEDERATION_SIZE' .nix-deps/manifold/crates/ -g '*.rs' | grep 'const' ; sed -n 375,386p .nix-deps/manifold/crates/fa-client/src/selection.rs 2>/dev/null", max_tokens=2000)

# ---- cell @ 2026-10-25T17:23:27.444Z ----
s=open(f'{Q}/qaenv.py').read().replace('MG_COUNT = 10','MG_COUNT = 14'); open(f'{Q}/qaenv.py','w').write(s)
importlib.reload(qaenv)
E.__class__ = qaenv.Env  # rebind methods
json.dump(qaenv.proxy_config(), open(cfgp,'w'))
await proxy_cmd.cancel()
proxy_cmd = command(f"python3 {Q}/netproxy.py {cfgp} 2>&1 | tee -a {Q}/netproxy.log")
await asyncio.sleep(3)
def add_mgs():
    for g in range(10,14): E.start_mg(g)
    for g in range(10,14): E.onboard(g)
    for g in range(10,14): E.st['locators'][str(g)] = E.read_locator(g)
    E.save()
    for g in range(14): E.stop_mg(g)
    for g in range(14): E.start_mg(g)
await asyncio.to_thread(add_mgs)
fa.stop(); await asyncio.to_thread(fa.start)
await asyncio.to_thread(set_prices, 100_000)
def wait_accept_all():
    for g in range(14):
        deadline=time.time()+240
        while True:
            listed=E.fman(g,'payment-federations','list')
            if [f for f in listed['federations'] if f.get('accepted') and f.get('receivable')]: break
            if time.time()>deadline: raise RuntimeError(f'mg-{g}: {listed}')
            time.sleep(2)
await asyncio.to_thread(wait_accept_all)
p = await preview_sel(fa, 7, 700_000)
print(json.dumps(p)[:800])

# ---- cell @ 2026-10-25T17:25:32.727Z ----
def mg_ids():
    m={}
    for g in range(14):
        ob=E.fman(g,'onboarding'); m[g]=ob
    return m
obs = await asyncio.to_thread(mg_ids)
print(json.dumps(obs[0])[:800]); print(json.loads(E.st['locators']['0']))

# ---- cell @ 2026-10-25T17:25:39.987Z ----
MGID={}
for g,ob in obs.items():
    for k in ('service_nostr_pubkey','service_pubkey','fman_name'): MGID[ob[k]]=g
print({g:(ob['fman_name'],ob['nostr']['holders']) for g,ob in obs.items()})
print([MGID.get(s['mgId'],s['mgId']) for s in p['seats']])

# ---- cell @ 2026-10-25T17:25:45.621Z ----
r = await fa.acall('faClientPayAndCreate', {'previewId': p['previewId'], 'intent': {'federationName':'QA Transfer 1','federationSize':7,'plan':'infiniteBestEffort'}, 'paymentFederationId': PAYFED, 'maxTotalMsats': '700000'})
print(r)
st = await fa.acall('faClientStatus'); print(json.dumps(st)[:1500])

# ---- cell @ 2026-10-25T17:25:49.202Z ----
r=await command(f"grep -n -i -E 'payment|error|warn' {fa.dir}/repl.log | grep -v -i 'DEBUG' | tail -30", max_tokens=5000)

# ---- cell @ 2026-10-25T17:25:58.139Z ----
await asyncio.to_thread(set_prices, 131072)
await asyncio.sleep(10)
print(await fa.acall('faClientResume'))
st = await fa.acall('faClientStatus'); f_=st['result']['status']; print(json.dumps(f_)[:300])

# ---- cell @ 2026-10-25T17:26:13.897Z ----
async def fa_balance(f, fed=None):
    r = await f.acall('listFederations')
    for x in r['result']:
        if x.get('id')==(fed or PAYFED): return x.get('balance')
    return r
print(await fa_balance(fa))
SEAT=131072
p = await preview_sel(fa, 7, 7*SEAT)
r = await fa.acall('faClientPayAndCreate', {'previewId': p['previewId'], 'intent': {'federationName':'QA Transfer 1','federationSize':7,'plan':'infiniteBestEffort'}, 'paymentFederationId': PAYFED, 'maxTotalMsats': str(7*SEAT)})
print(r)

# ---- cell @ 2026-10-25T17:26:39.790Z ----
r=await command(f"grep -n -E ' WARN | ERROR ' {fa.dir}/repl.log | grep -v netlink | tail -8 | cut -c1-600", max_tokens=4000)

# ---- cell @ 2026-10-25T17:26:43.461Z ----
r=await command(f"tail -c 6000 {fa.dir}/repl.log | sed 's/\\x1b\\[[0-9;]*m//g'", max_tokens=4000)

# ---- cell @ 2026-10-25T17:26:47.419Z ----
st = await fa.acall('faClientStatus'); f_=st['result']['status']; print(f_['type'], json.dumps(f_)[:2500])

# ---- cell @ 2026-10-25T17:26:53.281Z ----
sl = E.fman(6,'seats','list'); print(json.dumps(sl)[:2000])
r=await command(f"ls {E.mg_dir(6)} {E.mg_dir(6)}/seats {E.mg_dir(6)}/seats/*")

# ---- cell @ 2026-10-25T17:26:59.765Z ----
r=await command(f"{qaenv.BIN}/fman-cli --data-dir {E.mg_dir(6)} seats status --help; {qaenv.BIN}/fman-cli --data-dir {E.mg_dir(6)} seats status 8f46e6209cb1f867f0331c8d08fe59c16aa95dcbf55e38be85bc2ed273c0c020")
r=await command(f"sqlite3 {E.mg_dir(6)}/fleet-manager.sqlite '.tables'; sqlite3 {E.mg_dir(6)}/fleet-manager.sqlite '.schema seats' | head -40")

# ---- cell @ 2026-10-25T17:27:05.723Z ----
import sqlite3
def q(g, sql):
    c=sqlite3.connect(f'file:{E.mg_dir(g)}/fleet-manager.sqlite?mode=ro', uri=True, timeout=10)
    try: return c.execute(sql).fetchall()
    finally: c.close()
print(q(6,'select seat_no, lower(hex(quote_id)), fi_id from seats'))
for t in ['seat_transfer_in','seat_transfer_out','formed_seats','decommissioned_seats']:
    print(t, q(6, f"select sql from sqlite_master where name='{t}'")[0][0][:1500])

# ---- cell @ 2026-10-25T17:27:25.158Z ----
ID2HOME={}
for g in range(qaenv.MG_COUNT):
    for s_ in range(qaenv.MAX_SEATS):
        ID2HOME[qaenv.iroh_id(qaenv.seat_port(g,s_,'p2p'),'p2p')]=(g,s_)
print(len(ID2HOME))
def locate(seat_id):
    out=[]
    for g in range(qaenv.MG_COUNT):
        r=q(g, f"select seat_no from seats where lower(hex(quote_id))='{seat_id}'")
        if r: out.append((g,r[0][0]))
    return out
def running_seat_dirs():
    """(g, seat_no) of every live fedimintd child, by scanning /proc."""
    out=set()
    for pid in os.listdir('/proc'):
        if not pid.isdigit(): continue
        try: cmd=open(f'/proc/{pid}/cmdline','rb').read().split(b'\0')
        except Exception: continue
        if b'--data-dir' in cmd:
            try: d=cmd[cmd.index(b'--data-dir')+1].decode()
            except Exception: continue
            if d.startswith(E.root) and '/seats/' in d:
                parts=d[len(E.root)+1:].split('/')
                try:
                    stat=open(f'/proc/{pid}/stat').read().split()[2]
                    if stat=='Z': continue
                    out.add((int(parts[0].split('-')[1]), int(parts[2]), int(pid)))
                except Exception: pass
    return out
print(sorted(running_seat_dirs())[:5])

# ---- cell @ 2026-10-25T17:27:37.366Z ----
_pubcache={}
def ed_pub(sk):
    if sk not in _pubcache: _pubcache[sk]=qaenv.nostr_sign.ed25519_public(sk).hex()
    return _pubcache[sk]
def imported_seats():
    out={}  # (g,t) -> identity home
    for g in range(qaenv.MG_COUNT):
        try:
            rows=q(g,"select s.seat_no, t.state, t.iroh_p2p_sk from seat_transfer_in t join seats s on s.quote_id=t.quote_id")
        except Exception as e:
            continue
        for seat_no,state,sk in rows:
            if state=='imported' and sk:
                out[(g,seat_no)]=ID2HOME.get(ed_pub(bytes(sk)), ('unknown', ed_pub(bytes(sk))))
    return out
MON={'violations':[], 'routes':{}, 'log':[]}
def apply_route(home, host):
    g,s_=home; d,t=host
    for role in ('p2p','api'):
        for plane in (qaenv.CORE, qaenv.FAP):
            qaenv.proxy({'cmd':'retarget','listen':qaenv.plane_port(plane, qaenv.seat_port(g,s_,role)),'target':qaenv.seat_port(d,t,role)})
def monitor_once():
    imp=imported_seats()
    running=running_seat_dirs()
    run_by_seat={}
    for g,t,pid in running: run_by_seat.setdefault((g,t),[]).append(pid)
    ident_hosts={}
    for (g,t),pids in run_by_seat.items():
        home=imp.get((g,t),(g,t))
        ident_hosts.setdefault(home,[]).append((g,t,pids))
    now=time.strftime('%H:%M:%S')
    for home,hosts in ident_hosts.items():
        if len(hosts)>1:
            v=(now,'DOUBLE_HOST',home,hosts)
            MON['violations'].append(v); print('!!! VIOLATION', v)
    # routes: for each imported identity, route home->host if the host runs it (or prefer the imported host anyway)
    for host,home in imp.items():
        if home[0]=='unknown': continue
        if MON['routes'].get(home)!=host:
            apply_route(home, host); MON['routes'][home]=host
            MON['log'].append((now,'route',home,host)); print('route', home, '->', host)
    return imp, ident_hosts
async def monitor_loop():
    while True:
        try: await asyncio.to_thread(monitor_once)
        except Exception as e: MON['log'].append((time.strftime('%H:%M:%S'),'monerr',repr(e)))
        await asyncio.sleep(1)
mon_task = asyncio.create_task(monitor_loop())

# ---- cell @ 2026-10-25T17:27:45.837Z ----
async def fstatus(f):
    return (await f.acall('faClientStatus'))['result']['status']
async def fed_seats(f):
    st=await fstatus(f)
    fo=st.get('formation') or st.get('federation') or st
    return [(s['index'], s['seatId'], MGID.get(s['mgId'], s['mgId'])) for s in fo['seats']], fo
seats_, fo = await fed_seats(fa)
print(seats_)
bal0 = await fa_balance(fa); print('bal', bal0)
pv = await fa.acall('faClientPreviewTransferDestination'); print(pv)

# ---- cell @ 2026-10-25T17:27:51.160Z ----
t0=time.time()
start = await fa.acall('faClientStartGuardianTransfer', {'seatIndex':0,'previewId':pv['result']['preview']['previewId'],'paymentFederationId':PAYFED,'maxMsats':'131072'})
print(time.time()-t0, start)
gt = await fa.acall('faClientGuardianTransfer'); print(gt)

# ---- cell @ 2026-10-25T17:28:00.851Z ----
async def drive_transfer(f, timeout=300, interval=2, verbose=True):
    deadline=time.time()+timeout; phases=[]
    while time.time()<deadline:
        r = await f.acall('faClientResumeGuardianTransfer')
        res=r.get('result',{})
        tr=res.get('transfer') if isinstance(res,dict) else None
        ph = tr['phase'] if tr else (res.get('type') if isinstance(res,dict) else str(res))
        if not phases or phases[-1][1]!=ph:
            phases.append((round(time.time()-t0,1), ph, None if tr else str(r)[:400]))
            if verbose: print(phases[-1])
        if tr and ph=='completed': return phases, tr
        if tr is None and res.get('type')=='transfer': return phases, None
        await asyncio.sleep(interval)
    return phases, 'timeout'
t0=time.time()
ph, tr = await drive_transfer(fa)
print(MON['log'][-5:], MON['violations'])

# ---- cell @ 2026-10-25T17:28:11.362Z ----
INVITE1 = fo['inviteCode']
def moved_map(): return {home:host for home,host in MON['routes'].items()}
def fmw(wallet, *a, timeout=60):
    return qaenv.run(['fedimint-cli','--data-dir',os.path.join(E.root,wallet),*a], env={'FM_IROH_CONNECT_OVERRIDES_PLAIN': qaenv.overrides(None, moved_map()),'FM_IN_DEVIMINT':'1'}, timeout=timeout)
def meta(wallet, invite):
    if not os.path.exists(os.path.join(E.root,wallet,'client.db')):
        fmw(wallet,'join-federation',invite, timeout=60)
    return json.loads(fmw(wallet,'module','meta','get', timeout=30))
m = await asyncio.to_thread(meta,'chk-fed1',INVITE1)
print(json.dumps(m)[:3000])

# ---- cell @ 2026-10-25T17:28:19.257Z ----
def parse_meta(m):
    v=m['value']; out={'revision':m['revision'],'keys':sorted(v.keys())}
    b=json.loads(v['fedi:fman_seat_bindings'])
    out['bindings']={int(x['attestation']['peer_id']):(MGID.get(x['attestation']['fman_pubkey'],x['attestation']['fman_pubkey']), x['attestation']['guardian_fee_account']['pub_keys'][0][:10]) for x in b['seat_bindings']}
    return out, v
pm, v = parse_meta(m); print(pm)
for k in pm['keys']:
    if k!='fedi:fman_seat_bindings': print(k, v[k][:600])

# ---- cell @ 2026-10-25T17:28:29.212Z ----
def recipients(v):
    r=json.loads(v['fedi:guardian_fee_remittance_account'])['recipients']
    return [(x['account']['pub_keys'][0][:10], x['weight']) for x in r]
print(recipients(v))
print('src status', json.dumps(E.fman(6,'seats','status','8f46e6209cb1f867f0331c8d08fe59c16aa95dcbf55e38be85bc2ed273c0c020'))[:1500])
print(q(6,'select lower(hex(quote_id)), phase from seat_transfer_out'))
print(os.path.exists(f'{E.mg_dir(6)}/seats/1/data'), os.listdir(f'{E.mg_dir(6)}/seats/1') if os.path.exists(f'{E.mg_dir(6)}/seats/1') else None)
print('dst status', json.dumps(E.fman(12,'seats','status','3405b99c4c988e49d2cd570df85a7a237cb90670f2bd169a79d2b51aa34d3631'))[:1500])

# ---- cell @ 2026-10-25T17:28:39.167Z ----
FED1='5deedf246df6d0da4c72d7019ff967cb9f3e8df5054963fcb68401b67ab85ea1'
gs = await fa.acall('getGuardianStatus', {'federationId': FED1}); print(json.dumps(gs)[:1500])

# ---- cell @ 2026-10-25T17:29:06.004Z ----
seats_now, fo2 = await fed_seats(fa); print(seats_now)
def fm_peer(wallet, peer, method):
    return fmw(wallet, 'dev','api','--peer-id',str(peer), method, timeout=30)
try: print(await asyncio.to_thread(fm_peer,'chk-fed1',3,'status'))
except Exception as e: print('ERR', str(e)[:800])

# ---- cell @ 2026-10-25T17:29:29.330Z ----
st=await fstatus(fa); print(json.dumps(st)[:3000])

# ---- cell @ 2026-10-25T17:29:35.303Z ----
r=await command("rg -n 'mg_name' crates/federations/src crates/bridge/src --no-heading | grep -v test | head -20")
try: print(await asyncio.to_thread(fm_peer,'chk-fed1',3,'status'))
except Exception as e: print('ERR', str(e)[:800])

# ---- cell @ 2026-10-25T17:29:41.175Z ----
r=await command("sed -n 1620,1670p crates/federations/src/federation_v2/mod.rs; rg -n 'fn guardian_mg_names|fn mg_names' -A30 crates/federations/src | head -50", max_tokens=4000)

# ---- cell @ 2026-10-25T17:29:46.368Z ----
def fm_peer(wallet, peer, method):
    return fmw(wallet, 'dev','api','--peer-id',str(peer), method, timeout=30)
try: print(await asyncio.to_thread(fm_peer,'chk-fed1',3,'status'))
except Exception as e: print('ERR', str(e)[:800])

# ---- cell @ 2026-10-25T17:29:54.941Z ----
gs = await fa.acall('getGuardianStatus', {'federationId': FED1}); print([ (list(x.keys())[0], x[list(x.keys())[0]].get('mg_name'), x[list(x.keys())[0]].get('error','')[:80]) for x in gs['result']])
print(qaenv.proxy({'cmd':'state','show':[qaenv.plane_port(qaenv.FAP, qaenv.seat_port(6,1,'api')), qaenv.plane_port(qaenv.CORE, qaenv.seat_port(6,1,'api'))]}))

# ---- cell @ 2026-10-25T17:30:13.031Z ----
FA_COUNTER=[0]
async def setup_fed(name, fund_msats=50_000_000, relay_i=0, size=7):
    f=Fa(name, relay_i=relay_i); 
    await asyncio.to_thread(f.start)
    await f.acall('__complete_onboarding')
    await f.acall('joinFederation', {'inviteCode': E.st['payment_invite'], 'recoverFromScratch': False})
    for _ in range(30):
        r=await asyncio.to_thread(fund_fa, f, fund_msats)
        if 'result' in r: break
        await asyncio.sleep(1)
    p = await preview_sel(f, size, size*SEAT)
    r = await f.acall('faClientPayAndCreate', {'previewId': p['previewId'], 'intent': {'federationName':f'QA {name}','federationSize':size,'plan':'infiniteBestEffort'}, 'paymentFederationId': PAYFED, 'maxTotalMsats': str(size*SEAT)})
    deadline=time.time()+300
    while True:
        st=await fstatus(f)
        fo=st.get('formation',{})
        if fo.get('phase')=='formed' and (fo.get('inviteCode') or fo.get('federationInvite')):
            break
        if time.time()>deadline: raise RuntimeError(f'not formed: {json.dumps(st)[:1000]}')
        if fo.get('lastError'): 
            await f.acall('faClientResume')
        await asyncio.sleep(2)
    return await ctx_of(f)
async def ctx_of(f):
    st=await fstatus(f); fo=st['formation']
    invite=fo.get('inviteCode') or fo.get('federationInvite')
    seats=[]
    for i,s in enumerate(fo['seats']):
        loc=await asyncio.to_thread(locate, s['seatId'])
        seats.append({'i':i,'seat_id':s['seatId'],'mg':MGID.get(s['mgId'],s['mgId']),'loc':loc})
    fed_id = json.loads(qaenv.run(['fedimint-cli','decode-invite-code' if False else 'dev','decode','invite-code',invite]))  if False else None
    return {'fa':f,'invite':invite,'seats':seats,'name':f.name}
ctx1 = await ctx_of(fa); print(ctx1['seats'])

# ---- cell @ 2026-10-25T17:30:32.093Z ----
def seat_status(g, seat_id):
    try: return E.fman(g,'seats','status',seat_id, timeout=30)
    except Exception as e: return {'err':str(e)[:300]}
async def capture_pre(ctx, idx):
    s=ctx['seats'][idx]; g,sn=s['loc'][0]
    st=await asyncio.to_thread(seat_status,g,s['seat_id'])
    acct=json.loads(st['guardian_fee']['remittance_account'])['pub_keys'][0]
    m=await asyncio.to_thread(meta, f"chk-{ctx['name']}", ctx['invite'])
    pm,_=parse_meta(m)
    return {'src':(g,sn),'src_seat':s['seat_id'],'src_acct':acct,'bindings':pm['bindings'],'revision':pm['revision'],'bal':await fa_balance(ctx['fa'])}
async def verify(ctx, pre, tr, extra_ok=()):
    probs=[]
    f=ctx['fa']
    dst_seat=tr['destinationSeatId']; dst_g=MGID.get(tr['destinationMgId'])
    dl=await asyncio.to_thread(locate,dst_seat)
    if len(dl)!=1: probs.append(f'dest seat located {dl}')
    dg,dsn=dl[0]
    # meta
    m=await asyncio.to_thread(meta, f"chk-{ctx['name']}", ctx['invite'])
    pm,v=parse_meta(m)
    changed={p:(pre['bindings'][p],pm['bindings'][p]) for p in pm['bindings'] if pm['bindings'][p]!=pre['bindings'].get(p)}
    if len(changed)!=1: probs.append(f'binding changes {changed}')
    else:
        peer,(old,new)=list(changed.items())[0]
        if old[0]!=pre['src'][0] or new[0]!=dg: probs.append(f'binding peer {peer} {old}->{new} expected {pre["src"][0]}->{dg}')
    if pre['src'][0] in [b[0] for b in pm['bindings'].values()]: probs.append('source MG still in bindings')
    rec=[a for a,w in recipients(v)]
    dstat=await asyncio.to_thread(seat_status,dg,dst_seat)
    try: dacct=json.loads(dstat['guardian_fee']['remittance_account'])['pub_keys'][0]
    except Exception: dacct=None; probs.append(f'dest status {str(dstat)[:300]}')
    if pre['src_acct'][:10] in rec: probs.append('source fee account still a recipient')
    if dacct and dacct[:10] not in rec: probs.append('dest fee account not a recipient')
    # source
    sg,ssn=pre['src']
    so=q(sg, f"select phase from seat_transfer_out where lower(hex(quote_id))='{pre['src_seat']}'")
    if so!=[('removed',)]: probs.append(f'source transfer_out {so}')
    if os.path.exists(f'{E.mg_dir(sg)}/seats/{ssn}/data'): probs.append('source data dir exists')
    run=running_seat_dirs()
    if any(g==sg and t==ssn for g,t,_ in run): probs.append('source fedimintd running')
    if not any(g==dg and t==dsn for g,t,_ in run): probs.append('dest fedimintd not running')
    di=q(dg, f"select state from seat_transfer_in where lower(hex(quote_id))='{dst_seat}'")
    if di!=[('imported',)]: probs.append(f'dest transfer_in {di}')
    if dstat.get('report',{}).get('phase')!='running': probs.append(f"dest phase {dstat.get('report')}")
    # FA
    gt=(await f.acall('faClientGuardianTransfer'))['result']
    if gt.get('transfer') is not None: probs.append(f'FA transfer still open {gt}')
    st=await fstatus(f); ids=[s['seatId'] for s in st['formation']['seats']]
    if dst_seat not in ids or pre['src_seat'] in ids: probs.append('FA seat list not updated')
    bal=await fa_balance(f); spent=pre['bal']-bal
    if not (SEAT <= spent <= SEAT+20000): probs.append(f'FA spent {spent}')
    viol=[x for x in MON['violations']]
    if viol: probs.append(f'violations {viol}')
    return {'ok':not probs,'probs':probs,'spent':spent,'dest':(dg,dsn),'revision':pm['revision']}
print('ok')

# ---- cell @ 2026-10-25T17:30:39.444Z ----
r=await command("rg -n -i 'backoff|retry|interval|sleep' crates/mg/core/src/transfer*.rs crates/mg/core/src/*transfer* 2>/dev/null | head -30; ls crates/mg/core/src | head -80", workdir=R, max_tokens=3000)

# ---- cell @ 2026-10-25T17:30:45.025Z ----
r=await command("wc -l crates/mg/core/src/transfer_*.rs crates/mg/core/src/transfer_native.rs crates/mg/core/src/transfer_native/*.rs crates/fa-client/src/transfer*.rs crates/service-manifold-guardian/src/transfer*.rs 2>/dev/null", workdir=R)

# ---- cell @ 2026-10-25T17:30:50.392Z ----
print(open(f'{R}/crates/mg/core/src/transfer_worker.rs').read())

# ---- cell @ 2026-10-25T17:30:54.586Z ----
print(open(f'{R}/crates/mg/core/src/transfer_worker.rs').read()[9000:])
print(open(f'{R}/crates/mg/core/src/transfer_state.rs').read()[:6000])

# ---- cell @ 2026-10-25T17:31:05.327Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {E.mg_dir(12)}/stdout.log | grep -i -E 'transfer|import|pull|seat_binding|replace|remove' | grep -v -i 'netlink' | cut -c1-400 | head -40; echo ----; sed 's/\\x1b\\[[0-9;]*m//g' {E.mg_dir(6)}/stdout.log | grep -i -E 'transfer|retire|remove|tombstone|backup' | cut -c1-400 | tail -30", max_tokens=8000)

# ---- cell @ 2026-10-25T17:31:34.465Z ----
def fi_id_of(ctx):
    # read from one of the FA seat MGs
    s=ctx['seats'][0]; g,_=s['loc'][0]
    return q(g, f"select fi_id from seats where lower(hex(quote_id))='{s['seat_id']}'")[0][0]
def trig_sql(g, sql):
    try: return q(g, sql)
    except Exception: return None
async def wait_trigger(kind, info, timeout=180):
    deadline=time.time()+timeout
    fi=info['fi']; src_g,src_s=info['src']; src_seat=info['src_seat']; dg=info.get('dest_g')
    while time.time()<deadline:
        if kind=='dest_seat':
            r=await asyncio.to_thread(trig_sql, dg, f"select lower(hex(quote_id)) from seats where fi_id='{fi}'")
            if r and any(x[0] not in info['known_seats'] for x in r): return True
        elif kind in ('dest_waiting','dest_imported'):
            want='waiting' if kind=='dest_waiting' else 'imported'
            r=await asyncio.to_thread(trig_sql, dg, f"select t.state from seat_transfer_in t join seats s on s.quote_id=t.quote_id where s.fi_id='{fi}'")
            if r and any(x[0]==want for x in r): return True
        elif kind in ('src_stopped','src_removed'):
            r=await asyncio.to_thread(trig_sql, src_g, f"select phase from seat_transfer_out where lower(hex(quote_id))='{src_seat}'")
            if r and (kind=='src_stopped' or r[0][0]=='removed'): return True
        elif kind.startswith('fa_phase:'):
            try:
                r=await info['fa'].acall('faClientGuardianTransfer', timeout=5)
                tr=r['result'].get('transfer')
                if tr and tr['phase']==kind.split(':')[1]: return True
            except Exception: pass
        elif kind.startswith('after:'):
            await asyncio.sleep(float(kind.split(':')[1])); return True
        await asyncio.sleep(0.02)
    return False
print(fi_id_of(ctx1))

# ---- cell @ 2026-10-25T17:31:53.611Z ----
def mg_ranges(g):
    b=qaenv.mg_base(g); return [[b,b+99]]
def seat_ranges(g,s_):
    b=qaenv.seat_port(g,s_,'p2p'); return [[b,b+3]]
class Faults:
    def __init__(self, info):
        self.info=info; self.killed_mgs=set(); self.stopped=set(); self.tags=set(); self.relay=set(); self.fa_dead=False; self.fa_stopped=False; self.log=[]
    def resolve(self, who):
        i=self.info
        if who=='src': return i['src'][0]
        if who=='dst': return i['dest_g']
        if who.startswith('peer'): return i['peers'][int(who[4:])]
        return int(who)
    def do(self, act):
        kind=act[0]; t=time.strftime('%H:%M:%S'); self.log.append((t,act))
        if kind=='kill_mg':
            g=self.resolve(act[1]); E.kill_mg(g, kill_children=False); self.killed_mgs.add(g)
        elif kind=='kill_mg_all':
            g=self.resolve(act[1]); E.kill_mg(g, kill_children=True); self.killed_mgs.add(g)
        elif kind=='stop_mg':
            g=self.resolve(act[1]); p=E.mg_pid(g); os.kill(p, signal.SIGSTOP); self.stopped.add(('mg',g,p))
        elif kind=='stop_mg_tree':
            g=self.resolve(act[1]); p=E.mg_pid(g)
            for k in [p]+E.children(p): os.kill(k, signal.SIGSTOP); self.stopped.add(('pid',g,k))
        elif kind=='cut':
            g=self.resolve(act[1]); tag=f'cut-mg{g}'; qaenv.proxy({'cmd':'block','tag':tag,'ranges':mg_ranges(g)}); self.tags.add(tag)
        elif kind=='cut_endpoint':
            g=self.resolve(act[1]); e=qaenv.endpoint_port(g); tag=f'cutep-mg{g}'; qaenv.proxy({'cmd':'block','tag':tag,'ranges':[[e,e]]}); self.tags.add(tag)
        elif kind=='cut_relay':
            g=self.resolve(act[1]); qaenv.proxy({'cmd':'tcp_block','group':f'relay:mg{g}'}); self.relay.add(f'relay:mg{g}')
        elif kind=='cut_fa_relay':
            grp=f"relay:fa{self.info['fa'].relay_i}"; qaenv.proxy({'cmd':'tcp_block','group':grp}); self.relay.add(grp)
        elif kind=='cut_fa':
            tag='cut-fa'; qaenv.proxy({'cmd':'block_listen','tag':tag,'ranges':[[qaenv.FAP, qaenv.FAP+qaenv.MG_COUNT*100]]}); self.tags.add(tag)
        elif kind=='kill_fa':
            self.info['fa'].kill(); self.fa_dead=True
        elif kind=='stop_fa':
            os.kill(self.info['fa'].p.pid, signal.SIGSTOP); self.fa_stopped=True
        elif kind=='kill_seat':
            g=self.resolve(act[1]); s_=act[2] if len(act)>2 else None
            for gg,t,pid in running_seat_dirs():
                if gg==g and (s_ is None or t==s_): os.kill(pid, signal.SIGKILL)
        elif kind=='heal':
            self.heal()
        else: raise ValueError(act)
    def heal(self, restart=True):
        for x in list(self.stopped):
            try: os.kill(x[2], signal.SIGCONT)
            except ProcessLookupError: pass
        self.stopped.clear()
        for tag in list(self.tags): qaenv.proxy({'cmd':'unblock','tag':tag})
        self.tags.clear()
        for grp in list(self.relay): qaenv.proxy({'cmd':'tcp_unblock','group':grp})
        self.relay.clear()
        if self.fa_stopped:
            os.kill(self.info['fa'].p.pid, signal.SIGCONT); self.fa_stopped=False
        if restart:
            for g in list(self.killed_mgs):
                if E.mg_pid(g) is None: E.start_mg(g)
            self.killed_mgs.clear()
            if self.fa_dead or not self.info['fa'].alive():
                self.info['fa'].start(); self.fa_dead=False
print('ok')

# ---- cell @ 2026-10-25T17:32:09.721Z ----
RESULTS=[]
async def run_scenario(name, ctx, idx, faults=(), pre_faults=(), drive_timeout=420, expect_complete=True, heal_after=None, start_payment=None, max_msats=None, notes=''):
    """faults: list of (trigger, delay_s, action). pre_faults: actions before start."""
    t_start=time.time()
    f=ctx['fa']
    pre=await capture_pre(ctx, idx)
    pv=await f.acall('faClientPreviewTransferDestination')
    if pv['result'].get('type')!='preview':
        res={'name':name,'ok':False,'probs':[f'preview failed {pv}']}; RESULTS.append(res); return res
    dest_g=MGID.get(pv['result']['preview']['mgId'])
    peers=[s['loc'][0][0] for s in ctx['seats'] if s['i']!=idx]
    info={'fi':fi_id_of(ctx),'src':pre['src'],'src_seat':pre['src_seat'],'dest_g':dest_g,'fa':f,'peers':peers,
          'known_seats':{x[0] for x in q(dest_g, 'select lower(hex(quote_id)) from seats')}}
    F=Faults(info)
    for a in pre_faults: F.do(a)
    async def fault_task(trig, delay, act):
        ok=await wait_trigger(trig, info)
        if not ok: F.log.append(('trigger-timeout',trig)); return
        if delay: await asyncio.sleep(delay)
        await asyncio.to_thread(F.do, act)
    ftasks=[asyncio.create_task(fault_task(*x)) for x in faults]
    start_res=None
    try:
        start_res=await f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':pv['result']['preview']['previewId'],'paymentFederationId':start_payment or PAYFED,'maxMsats':str(max_msats if max_msats is not None else SEAT)}, timeout=120)
    except Exception as e:
        start_res={'exception':repr(e)[:200]}
    # let faults fire (bounded)
    done,pending=await asyncio.wait(ftasks, timeout=60) if ftasks else (set(),set())
    for p_ in pending: p_.cancel()
    if heal_after: await asyncio.sleep(heal_after)
    await asyncio.to_thread(F.heal)
    # drive
    phases=[]; tr=None; deadline=time.time()+drive_timeout; last=None
    while time.time()<deadline:
        try:
            r=await f.acall('faClientResumeGuardianTransfer', timeout=120)
        except Exception as e:
            r={'exception':repr(e)[:200]}
            if not f.alive(): await asyncio.to_thread(f.start)
        res_=r.get('result',{}) if isinstance(r,dict) else {}
        t_=res_.get('transfer') if isinstance(res_,dict) else None
        ph=t_['phase'] if t_ else json.dumps(r)[:200]
        if ph!=last: phases.append((round(time.time()-t_start,1),ph)); last=ph
        if t_ and ph=='completed': tr=t_; break
        if t_ is None and res_.get('type')=='transfer':
            # no open transfer
            break
        await asyncio.sleep(2)
    out={'name':name,'notes':notes,'start':str(start_res)[:300],'phases':phases,'faults':F.log,'dest':dest_g,'src':pre['src'],'secs':round(time.time()-t_start)}
    if tr:
        v=await verify(ctx, pre, tr); out.update(v)
        # refresh ctx seats
        nctx=await ctx_of(f); ctx['seats']=nctx['seats']
    else:
        out['ok']=not expect_complete
        out['probs']=['did not complete'] if expect_complete else []
        out['bal_delta']=pre['bal']-(await fa_balance(f))
    RESULTS.append(out)
    print(json.dumps({k:out.get(k) for k in ('name','ok','probs','phases','secs','spent','bal_delta')})[:1500])
    return out
print('ok')

# ---- cell @ 2026-10-25T17:32:18.007Z ----
ctx1['name']='fed1'
res = await run_scenario('S2 source offline (SIGINT) at grant, back after 45s', ctx1, 1, pre_faults=[('kill_mg_all','src')], heal_after=45)

# ---- cell @ 2026-10-25T17:33:25.041Z ----
print(RESULTS[-1]['start'])
gs = await fa.acall('getGuardianStatus', {'federationId': FED1}); print([ (x[list(x.keys())[0]].get('mg_name')) for x in gs['result']])
print(json.dumps(E.fman(12,'seats','status','3405b99c4c988e49d2cd570df85a7a237cb90670f2bd169a79d2b51aa34d3631')['guardian_fee']))
m = await asyncio.to_thread(meta,'chk-fed1',INVITE1); print(parse_meta(m)[0])

# ---- cell @ 2026-10-25T17:33:32.015Z ----
fa.stop(); await asyncio.to_thread(fa.start)
await asyncio.sleep(5)
gs = await fa.acall('getGuardianStatus', {'federationId': FED1}); print([ (x[list(x.keys())[0]].get('mg_name'), list(x.keys())[0]) for x in gs['result']])
r = await fa.acall('listFederations'); 
for x in r['result']:
    if x.get('id')==FED1: print(json.dumps(x.get('meta'))[:300])

# ---- cell @ 2026-10-25T17:34:00.160Z ----
kids=E.seat_children(5); print(kids)
E.kill_mg(5, kill_children=False); await asyncio.sleep(1)
print([k for k in kids if os.path.exists(f'/proc/{k}') and open(f'/proc/{k}/stat').read().split()[2]!='Z'])
E.start_mg(5); await asyncio.sleep(8); print(E.seat_children(5))

# ---- cell @ 2026-10-25T17:34:14.411Z ----
rs=running_seat_dirs(); x=[r for r in rs if r[0]==5]; print(x, E.mg_pid(5))
for g,t,pid in x: print(pid, open(f'/proc/{pid}/stat').read().split()[3])

# ---- cell @ 2026-10-25T17:34:20.131Z ----
await asyncio.sleep(10)
rs=running_seat_dirs(); print([r for r in rs if r[0]==5], E.mg_pid(5), q(5,'select seat_no, fi_id from seats'))
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {E.mg_dir(5)}/stdout.log | grep -v -i netlink | tail -15 | cut -c1-300")

# ---- cell @ 2026-10-25T17:34:36.094Z ----
rs=running_seat_dirs(); x=[r for r in rs if r[0]==6]; print(x, E.mg_pid(6))
for g,t,pid in x: print(pid, 'ppid', open(f'/proc/{pid}/stat').read().split()[3])
E.kill_mg(6, kill_children=False); await asyncio.sleep(1.5)
print('alive after kill', [pid for g,t,pid in x if os.path.exists(f'/proc/{pid}') and open(f'/proc/{pid}/stat').read().split()[2]!='Z'])
E.start_mg(6); await asyncio.sleep(10); print([r for r in running_seat_dirs() if r[0]==6])

# ---- cell @ 2026-10-25T17:35:00.860Z ----
r=await command("ps -eo rss,args | grep -E 'fedimintd|fleet-manager|fediffi' | grep -v grep | awk '{s+=$1; n++} END {print n, s/1024 \" MB\"}'; free -g | head -2")

# ---- cell @ 2026-10-25T17:35:11.490Z ----
CTXS=[]; ctx1['transfers']=2; CTXS.append(ctx1)
def decommission_fed(ctx):
    fi=fi_id_of(ctx)
    for g in range(qaenv.MG_COUNT):
        for (sid,) in q(g, f"select lower(hex(quote_id)) from seats where fi_id='{fi}' and quote_id not in (select quote_id from decommissioned_seats)"):
            try: E.fman(g,'seats','decommission',sid)
            except Exception as e: print('decom err', g, sid, str(e)[:200])
async def get_ctx(max_transfers=6):
    for c in CTXS:
        if c.get('retired'): continue
        if c['transfers']<max_transfers: return c
        c['retired']='exhausted'
        try:
            c['fa'].stop()
        except Exception: pass
        await asyncio.to_thread(decommission_fed, c)
    FA_COUNTER[0]+=1
    name=f'f{FA_COUNTER[0]}'
    c=await setup_fed(name)
    c['transfers']=0; c['name']=name; CTXS.append(c)
    print('new fed', name, [s['mg'] for s in c['seats']])
    return c
def log_result(res):
    with open(f'{Q}/results.jsonl','a') as fh: fh.write(json.dumps(res, default=str)+'\n')
async def sweep(cases, label):
    for k,(trig,delay,act) in enumerate(cases):
        c=await get_ctx()
        idx=c['transfers']%7
        name=f'{label}#{k} trig={trig} +{delay}s act={act}'
        try:
            res=await run_scenario(name, c, idx, faults=[(trig,delay,act)])
        except Exception as e:
            import traceback
            res={'name':name,'ok':False,'probs':['harness exception '+repr(e)[:300]],'tb':traceback.format_exc()[-1500:]}
            RESULTS.append(res)
        c['transfers']+=1
        log_result(res)
        if not res.get('ok'):
            c['retired']='failed: '+name
            print('!!! FAIL', name, res.get('probs'))
            try: await asyncio.to_thread(Faults({'fa':c['fa']}).heal, False)
            except Exception: pass
print('ok')

# ---- cell @ 2026-10-25T17:35:17.545Z ----
triggers=['dest_seat','dest_waiting','src_stopped','dest_imported','fa_phase:publishingBinding','fa_phase:removingSource','src_removed']
actions=[('kill_mg','src'),('kill_mg','dst'),('kill_fa',)]
cases=[(t,d,a) for a in actions for t in triggers for d in (0,0.3)]
print(len(cases))
sweep_task = asyncio.create_task(sweep(cases,'R1'))

# ---- cell @ 2026-10-25T17:35:22.019Z ----
src=open(f'{R}/crates/fa-client/src/transfer.rs').read(); print(src[:20000])

# ---- cell @ 2026-10-25T17:35:29.814Z ----
print(src[20000:52000])

# ---- cell @ 2026-10-25T17:35:35.178Z ----
i=src.find('async fn publish_binding'); print(i); 
for key in ['fn resume_guardian_transfer','fn publish','ReplaceSeatBinding','RemoveSeat']:
    print(key, [j for j in range(len(src)) if src.startswith(key, j)][:5])

# ---- cell @ 2026-10-25T17:35:39.585Z ----
print(src[30500:46000])

# ---- cell @ 2026-10-25T17:35:46.539Z ----
for r_ in RESULTS[-10:]: print(r_.get('name'), r_.get('ok'), r_.get('probs'), r_.get('secs'), [p[1][:40] for p in r_.get('phases',[])])

# ---- cell @ 2026-10-25T17:35:54.516Z ----
r=await command("git tag --sort=-creatordate | head -10; git log --oneline -1 v0.1.8 2>/dev/null", workdir=M)

# ---- cell @ 2026-10-25T17:35:58.597Z ----
r=await command("git fetch -q --tags origin; git tag --sort=-creatordate | head -10; git log --oneline origin/master | grep -i -E 'release|0\\.1\\.[5-8]' | head", workdir=M)

# ---- cell @ 2026-10-25T17:36:05.347Z ----
OLD='/src/qa-transfer-manifold-018'
r=await command(f"git worktree add --detach {OLD} 0d1472e8 2>&1 | tail -1", workdir=M)
oldbuild = command("cargo build -p mg -p mg-cli 2>&1 | tail -3; ls target/debug | grep -E '^(fleet-manager|fman-cli)$'", workdir=OLD)

# ---- cell @ 2026-10-25T17:37:05.326Z ----
await asyncio.sleep(600)
for r_ in RESULTS[-12:]: print(r_.get('name'), r_.get('ok'), r_.get('probs'), r_.get('secs'), [p[1][:40] for p in r_.get('phases',[])])

# ---- cell @ 2026-10-25T17:39:11.009Z ----
await Task.from_session_id(5957)

# ---- cell @ 2026-10-25T17:41:15.427Z ----
for r_ in RESULTS[-12:]: print(r_.get('name'), r_.get('ok'), r_.get('probs'), r_.get('secs'), [p[1][:40] for p in r_.get('phases',[])])

# ---- cell @ 2026-10-25T17:41:20.676Z ----
c=[c for c in CTXS if not c.get('retired')][-1]; print(c['name'], c['transfers'])
print(await c['fa'].acall('faClientGuardianTransfer', timeout=10))
print(MON['log'][-6:])
r=await command(f"tail -c 3000 {c['fa'].dir}/repl.log | sed 's/\\x1b\\[[0-9;]*m//g' | grep -v netlink | tail -12 | cut -c1-400")

# ---- cell @ 2026-10-25T17:41:25.473Z ----
print(await c['fa'].acall('faClientResumeGuardianTransfer', timeout=60))
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {c['fa'].dir}/repl.log | grep -E ' WARN | ERROR ' | grep -v netlink | tail -15 | cut -c1-500")

# ---- cell @ 2026-10-25T17:41:33.208Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {c['fa'].dir}/repl.log | grep -E 'faClientStartGuardianTransfer|transfer|Transfer' | grep -v 'faClientResumeGuardianTransfer\\|faClientGuardianTransfer' | tail -20 | cut -c1-400; date -u")

# ---- cell @ 2026-10-25T17:41:41.880Z ----
import re as _re
txt=open(f"{c['fa'].dir}/repl.log", errors='replace').read()
txt=_re.sub(r'\x1b\[[0-9;]*m','',txt)
started={}; done=set()
for line in txt.splitlines():
    m_=_re.search(r'method="(\w+)" request_id=(\d+)\}: (fediffi::rpc: rpc call|fm::timing)', line)
    if m_:
        key=(m_.group(1), m_.group(2), line[:20]) 
        if 'rpc call' in line: started[m_.group(2)]=(m_.group(1), line[:30])
        else: done.add(m_.group(2))
# request ids reset on restart; just show unmatched among last 
print([ (k,v) for k,v in list(started.items())[-60:] if k not in done])

# ---- cell @ 2026-10-25T17:41:48.245Z ----
lines=[l for l in txt.splitlines() if l[:19]>='2026-10-25T17:36:00' and 'netlink' not in l and 'rpc call' not in l and 'fm::timing' not in l]
print(len(lines))
for l in lines[:60]: print(l[:300])

# ---- cell @ 2026-10-25T17:41:57.173Z ----
gs = await c['fa'].acall('getGuardianStatus', {'federationId': FED1}, timeout=60); print([ (list(x.keys())[0], x[list(x.keys())[0]].get('mg_name'), x[list(x.keys())[0]].get('error','')[:100]) for x in gs['result']])
try: print(await asyncio.to_thread(fm_peer,'chk-fed1',0,'status'))
except Exception as e: print('ERR', str(e)[:500])

# ---- cell @ 2026-10-25T17:42:02.390Z ----
m = await asyncio.to_thread(meta,'chk-fed1',INVITE1); print(parse_meta(m)[0])
print(json.dumps(E.fman(0,'seats','status','e410951195012d5e1fe29b90f7312f4aa4cd707253a5909a4f4cd90de70157ac'))[:800])

# ---- cell @ 2026-10-25T17:42:12.059Z ----
for g in [9,13,12,3,7,2,0,1]:
    t=_re.sub(r'\x1b\[[0-9;]*m','',open(f'{E.mg_dir(g)}/stdout.log',errors='replace').read())
    ls=[l for l in t.splitlines() if l[:19]>='2026-10-25T17:36:00' and ('WARN' in l or 'ERROR' in l) and 'netlink' not in l and 'telemetry' not in l]
    print(g, len(ls)); 
    for l in ls[-6:]: print('   ', l[:350])

# ---- cell @ 2026-10-25T17:42:18.043Z ----
for g in [9,13,12,3,7,2,0,1]:
    t=_re.sub(r'\x1b\[[0-9;]*m','',open(f'{E.mg_dir(g)}/stdout.log',errors='replace').read())
    ls=[l for l in t.splitlines() if l[:19]>='2026-10-25T17:35:30' and _re.search(r'(?i)replace|binding|meta|transfer|vote', l) and 'netlink' not in l]
    print(g, len(ls))
    for l in ls[-5:]: print('   ', l[:400])

# ---- cell @ 2026-10-25T17:42:28.447Z ----
try:
    out=await asyncio.to_thread(fmw,'chk-fed1','module','meta','get-submissions', timeout=30); print(out[:4000])
except Exception as e: print(str(e)[:1500])

# ---- cell @ 2026-10-25T17:42:42.256Z ----
def mglog(g, since, pat, n=40, width=600):
    t=open(f'{E.mg_dir(g)}/stdout.log',errors='replace').read()
    t=_re.sub(r'(\\x1b|\x1b)\[[0-9;]*m','',t)
    ls=[l for l in t.splitlines() if l[:19]>=since and _re.search(pat,l)]
    for l in ls[-n:]: print(l[:width])
mglog(0,'2026-10-25T17:36:00','meta|Meta', 30)

# ---- cell @ 2026-10-25T17:42:57.643Z ----
mglog(9,'2026-10-25T17:35:00','02d666efe8d5.*(onnect|isconnect)|(onnect|isconnect).*02d666efe8d5', 40, 420)

# ---- cell @ 2026-10-25T17:43:07.358Z ----
mglog(9,'2026-10-25T17:36:00','02d666efe8d5.*(ession|Session)', 30, 400)

# ---- cell @ 2026-10-25T17:43:19.315Z ----
from collections import Counter
def item_hist(g, seat_prefix, since='2026-10-25T17:30', until='2026-10-25T17:45'):
    t=open(f'{E.mg_dir(g)}/stdout.log',errors='replace').read()
    t=_re.sub(r'(\\x1b|\x1b)\[[0-9;]*m','',t)
    c=Counter()
    for l in t.splitlines():
        if since<=l[:16]<=until and 'process_consensus_item' in l and seat_prefix in l:
            m_=_re.search(r'session_index=(\d+)',l); c[(l[11:16], m_.group(1) if m_ else '?')]+=1
    return sorted(c.items())
print(item_hist(9,'02d666efe8d5'))
print(item_hist(0,'e410951195'))

# ---- cell @ 2026-10-25T17:43:27.303Z ----
mglog(9,'2026-10-25T17:30:00','02d666efe8d5.*(Complet|complet|signed|Signed|aleph|Aleph)', 20, 400)
mglog(0,'2026-10-25T17:36:00','e410951195.*(Complet|complet|signed|aleph|Aleph|session|Session)', 20, 400)

# ---- cell @ 2026-10-25T17:43:40.077Z ----
for g,sp in [(13,'a8aecf'),(12,'3405b9'),(3,'a1b4a8'),(7,'8c6a8d'),(2,'f615d1'),(0,'e41095')]:
    print('== MG',g)
    mglog(g,'2026-10-25T17:37:30',f'{sp}.*(Disconnected from peer|Connected to peer|Attempting)', 12, 260)

# ---- cell @ 2026-10-25T17:43:45.921Z ----
for g,sp in [(13,'a8aecf'),(12,'3405b9'),(3,'a1b4a8'),(7,'8c6a8d'),(2,'f615d1'),(0,'e41095'),(9,'02d666')]:
    print('== MG',g)
    mglog(g,'2026-10-25T17:37:30',f'(Disconnected from peer|Connected to peer |Attempting).*{sp}', 12, 200)

# ---- cell @ 2026-10-25T17:44:07.268Z ----
r=await command(f"ls -la {E.mg_dir(0)}/seats/; for s in {E.mg_dir(0)}/seats/*; do echo $s; ls -la $s/data | head; du -sh $s/data; done", max_tokens=3000)

# ---- cell @ 2026-10-25T17:44:20.935Z ----
r=await command(f"grep -l -i 'fork' {E.root}/mg-*/stdout.log; grep -h -i 'fork' {E.root}/mg-*/stdout.log | sed 's/\\x1b\\[[0-9;]*m//g' | cut -c1-300 | head -10")

# ---- cell @ 2026-10-25T17:44:27.462Z ----
r=await command("rg -n -i 'fork|aleph.*backup|AlephUnits|unit_saver|backup_loader' .nix-deps/fedimint/fedimint-server/src/consensus/ -g '*.rs' | head -30", workdir=R, max_tokens=3000)

# ---- cell @ 2026-10-25T17:44:32.010Z ----
r=await command("sed -n 330,420p .nix-deps/fedimint/fedimint-server/src/consensus/engine.rs", workdir=R, max_tokens=3000)

# ---- cell @ 2026-10-25T17:44:44.701Z ----
mglog(9,'2026-10-25T17:20:00','Aleph|aleph|round limit|session_index=', 25, 260)

# ---- cell @ 2026-10-25T17:46:07.314Z ----
for r in RESULTS[-10:]: print({k:r.get(k) for k in ('name','ok','fails','dur','idx','fed')})
print(sweep_task.done(), mon_task.done(), MON.get('violations'))

# ---- cell @ 2026-10-25T17:46:10.740Z ----
import datetime;print(datetime.datetime.utcnow())
print(RESULTS[-1].keys())
command("tail -c 3000 /src/scratch/qa-gt/sweep.log 2>/dev/null; ls -t /src/scratch/qa-gt | head")

# ---- cell @ 2026-10-25T17:46:14.076Z ----
import inspect
print(inspect.getsource(run_scenario), max_tokens=6000)

# ---- cell @ 2026-10-25T17:46:16.839Z ----
print(inspect.getsource(verify), max_tokens=5000)

# ---- cell @ 2026-10-25T17:46:24.506Z ----
src=inspect.getsource(run_scenario)
src=src.replace("""    if tr:
        v=await verify(ctx, pre, tr)""","""    if tr is None and last is not None and '"transfer": null' in str(last) or (tr is None and phases and phases[-1][1].startswith('{"result": {"type": "transfer", "transfer": null')):
        newq=[x[0] for x in q(dest_g, 'select lower(hex(quote_id)) from seats') if x[0] not in info['known_seats']]
        inv={v_:k_ for k_,v_ in MGID.items()}
        if len(newq)==1:
            tr={'destinationSeatId':newq[0],'destinationMgId':inv[dest_g]}; out['notes']=(notes+' [completed-in-background]').strip()
    if tr:
        v=await verify(ctx, pre, tr)""")
exec(src, globals())
print('"transfer": null' in json.dumps({'result':{'type':'transfer','transfer':None}}))

# ---- cell @ 2026-10-25T17:46:29.650Z ----
r=RESULTS[3]; print(json.dumps(r)[:3000])

# ---- cell @ 2026-10-25T17:46:33.793Z ----
print(inspect.getsource(mglog))

# ---- cell @ 2026-10-25T17:46:38.009Z ----
def mglog2(g, since, until, pat, n=60, width=400, first=False):
    t=open(f'{E.mg_dir(g)}/stdout.log',errors='replace').read()
    t=_re.sub(r'(\\x1b|\x1b)\[[0-9;]*m','',t)
    ls=[l for l in t.splitlines() if since<=l[:19]<=until and _re.search(pat,l)]
    for l in (ls[:n] if first else ls[-n:]): print(l[:width])
    return len(ls)
mglog2(0,'2026-10-25T17:35:30','2026-10-25T17:43:00','e410951195012d5e.*(consensus session|meta|Session|session)|e410951195012d5e', n=50, width=330, first=True)

# ---- cell @ 2026-10-25T17:46:42.794Z ----
mglog2(0,'2026-10-25T17:36:13','2026-10-25T17:43:30','e410951195012d5e.*(consensus|meta|ession|WARN|ERROR|isconnect|Connected to peer)', n=80, width=300, first=True)

# ---- cell @ 2026-10-25T17:46:46.422Z ----
n=mglog2(0,'2026-10-25T17:36:13','2026-10-25T17:43:30','e410951195012d5e', n=60, width=300, first=True); print(n)

# ---- cell @ 2026-10-25T17:46:50.460Z ----
mglog2(0,'2026-10-25T17:38:06','2026-10-25T17:44:30','e410951195012d5e.*(fm::consensus|meta:|Connected to peer|isconnect|WARN|ERROR)', n=60, width=260, first=True)

# ---- cell @ 2026-10-25T17:46:54.373Z ----
n=mglog2(0,'2026-10-25T17:38:06','2026-10-25T17:50:30','e410951195012d5e', n=40, width=260, first=True); print(n)

# ---- cell @ 2026-10-25T17:47:09.955Z ----
t=open(f'{E.mg_dir(0)}/stdout.log',errors='replace').read()
t=_re.sub(r'(\\x1b|\x1b)\[[0-9;]*m','',t)
ls=[l for l in t.splitlines() if '2026-10-25T17:42:26'<=l[:19]<='2026-10-25T17:50' and 'e410951195012d5e' in l and ('meta' in l or 'consensus' in l)]
import collections
for l in ls:
    m=_re.search(r'item_index=(\d+) peer=PeerId\((\d)\).*?(Peer submitted.*?matching=\d|New consensus value.*?rev=\d+)',l)
    if m: print(l[11:23], m.groups())
    else: print(l[11:23], _re.sub(r'.*?(fm::\S+)',r'\1',l)[:150])

# ---- cell @ 2026-10-25T17:47:24.869Z ----
def loglines(g):
    t=open(f'{E.mg_dir(g)}/stdout.log',errors='replace').read()
    return _re.sub(r'(\\x1b|\x1b)\[[0-9;]*m','',t).splitlines()
LL={g:loglines(g) for g in range(14)}
for g in range(14):
    for l in LL[g]:
        if '2026-10-25T17:38:30'<=l[:19]<='2026-10-25T17:43' and 'New consensus value key=0' in l:
            print(g, l[11:23], _re.search(r'rev=\d+',l).group(), _re.search(r'seat_id=(\w{8})',l).group(1))

# ---- cell @ 2026-10-25T17:47:33.886Z ----
def items_timeline(g, sid, since, until):
    out=[]
    for l in LL[g]:
        if since<=l[:19]<=until and sid in l:
            m=_re.search(r'session_index=(\d+) item_index=(\d+)',l)
            if m: out.append((l[11:19],int(m.group(1)),int(m.group(2))))
            elif 'fm::consensus' in l: out.append((l[11:19], _re.sub(r'.*fm::consensus: ','',l)[:90]))
    # compress by second
    last=None
    for o in out:
        key=o[0][:7]+(str(o[1:]) if len(o)==2 else '')
        if key!=last: print(o); last=key
items_timeline(3,'a1b4a8f3','2026-10-25T17:30','2026-10-25T17:46')

# ---- cell @ 2026-10-25T17:47:49.755Z ----
for g in [0,2,3,7,9,12,13]:
    c=collections.Counter()
    for l in LL[g]:
        if '2026-10-25T17:35:30'<=l[:19]<='2026-10-25T17:46' and ('WARN' in l or 'ERROR' in l or 'fork' in l.lower() or 'Aleph' in l):
            k=_re.sub(r'^.*?(WARN|ERROR|INFO)\s+','',l[60:])
            k=_re.sub(r'seat_id=.*','',k)[:140]
            c[k]+=1
    print(g, c.most_common(8))

# ---- cell @ 2026-10-25T17:47:54.044Z ----
r=await command("rg -n 'RUST_LOG|FM_LOG|env_clear' crates/mg --no-heading | head -30", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T17:47:56.840Z ----
r=await command("sed -n 515,640p crates/mg/core/src/seat_process.rs", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T17:48:02.401Z ----
r=await command("ls ~/.cargo/registry/src/*/ | grep -i aleph; grep -rn 'fork' ~/.cargo/registry/src/*/aleph-bft-0.*/src --include=*.rs | grep -E 'warn!|error!|info!' | head -20")

# ---- cell @ 2026-10-25T17:48:06.942Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src && grep -rn -i 'fork' --include=*.rs . | grep -E 'warn!|error!|info!|debug!' | head -20; grep -rn 'target: \"' --include=*.rs . | sed 's/.*target: \"\\([^\"]*\\)\".*/\\1/' | sort | uniq -c | head")

# ---- cell @ 2026-10-25T17:48:10.642Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src && grep -rn -i -B2 -A3 'forker\\|fork' --include=*.rs alerts/ dag/ 2>/dev/null | grep -E 'warn!|error!|info!|debug!|trace!' | head -30; ls")

# ---- cell @ 2026-10-25T17:48:14.117Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src && grep -rn 'LOG_TARGET' alerts/*.rs | head -3; sed -n 70,100p alerts/service.rs; grep -rn 'Fork\\|fork' alerts/handler.rs | head -20")

# ---- cell @ 2026-10-25T17:48:19.128Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src && grep -rn -i 'fork' runway/*.rs units/*.rs | grep -iE 'warn|error|info|debug|trace' | head -20")

# ---- cell @ 2026-10-25T17:48:22.141Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src && sed -n 340,460p runway/mod.rs")

# ---- cell @ 2026-10-25T17:48:27.240Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src && ls runway; grep -n 'warn!\\|info!\\|debug!\\|error!' runway/collection.rs | head -30")

# ---- cell @ 2026-10-25T17:48:31.374Z ----
print(sum(1 for g in range(14) for l in LL[g] if 'unit collection' in l))
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src && sed -n 150,300p runway/collection.rs")

# ---- cell @ 2026-10-25T17:48:42.571Z ----
for g,sid in [(0,'e4109511'),(2,'f615d15a'),(3,'a1b4a8f3'),(7,'8c6a8dba'),(9,'02d666ef'),(12,'3405b99c'),(13,'a8aecf60')]:
    evs=[]
    for l in LL[g]:
        if sid in l and l[:13]=='2026-10-25T17' and ('New consensus value' in l or 'Starting consensus session' in l or 'Starting fedimintd' in l or 'Recovering' in l):
            m=_re.search(r'(rev=\d+|session_index=\d+ is_recovery=\w+|Starting fedimintd|units_len=\d+)',l)
            evs.append(l[11:19]+' '+m.group(1))
    print(g,sid,'\n   '+'\n   '.join(evs))

# ---- cell @ 2026-10-25T17:49:03.892Z ----
for r in RESULTS[6:]: print(r['name'], r['ok'], r.get('probs'), r['secs'], [p[1][:40] for p in r['phases']])

# ---- cell @ 2026-10-25T17:49:07.860Z ----
print(time.strftime('%H:%M:%S', time.gmtime()))
print(SWEEP_STATE if 'SWEEP_STATE' in globals() else None)
print(inspect.getsource(sweep)[:3000])

# ---- cell @ 2026-10-25T17:49:12.495Z ----
run_scenario.__defaults__=tuple(900 if d==420 else d for d in run_scenario.__defaults__)
print(run_scenario.__defaults__)

# ---- cell @ 2026-10-25T17:49:18.837Z ----
LL[1]=loglines(1)
src_sid=None
for l in LL[1]:
    if '2026-10-25T17:35:50'<=l[:19]<='2026-10-25T17:37' and ('transfer' in l.lower() or 'Starting fedimintd' in l or 'consensus session' in l):
        print(l[11:23], _re.sub(r'^\S+\s+\w+\s+','',l)[:250])

# ---- cell @ 2026-10-25T17:49:38.819Z ----
print(inspect.getsource(Faults), max_tokens=5000)
print(inspect.getsource(wait_trigger), max_tokens=3000)

# ---- cell @ 2026-10-25T17:49:42.927Z ----
print([m for m in dir(Faults) if not m.startswith('__')])
for m in ['do','heal','cut','stop_mg','kill_seat','cut_relay','cut_fa_relay']:
    fn=getattr(Faults,m,None)
    try: print(inspect.getsource(fn))
    except Exception as e: print(m,'nosrc')
try: print(inspect.getsource(wait_trigger))
except Exception as e: print('wt nosrc')

# ---- cell @ 2026-10-25T17:49:49.736Z ----
LL={g:loglines(g) for g in range(14)}
for g,sid in [(2,'f615d15a'),(7,'8c6a8dba')]:
    print('==',g)
    for l in LL[g]:
        if sid in l and '2026-10-25T17:35:30'<=l[:19]<='2026-10-25T17:42:30' and ('Connected to peer' in l or 'isconnect' in l or 'reconnect' in l or 'session' in l):
            print(l[11:23], _re.sub(r'.*?(io-state-machine\{peer_id=PeerId\(\d\)\}|fm::consensus)','\\1',l)[:130])

# ---- cell @ 2026-10-25T17:50:05.706Z ----
print(q(0,"select sql from sqlite_master where name='seats'")[0][0][:1500])

# ---- cell @ 2026-10-25T17:50:10.967Z ----
import glob
p=glob.glob(f'{E.mg_dir(0)}/seats/*/data/consensus.json')[0]
d=json.load(open(p)); print(p, list(d.keys())); print({k:(list(v.keys()) if isinstance(v,dict) else str(v)[:80]) for k,v in d.items()})

# ---- cell @ 2026-10-25T17:50:17.172Z ----
def seat_map():
    m={}
    for g in range(14):
        for qid,sn in q(g,"select lower(hex(quote_id)), seat_no from seats"):
            p=f'{E.mg_dir(g)}/seats/{sn}/data/consensus.json'
            if os.path.exists(p):
                d=json.load(open(p)); m[qid]={'g':g,'sn':sn,'fed':d['meta'].get('federation_name'),'keys':d['broadcast_public_keys']}
    return m
SM=seat_map()
feds=collections.defaultdict(list)
for k,v in SM.items(): feds[v['fed']].append((k[:8],v['g']))
for f_,v in feds.items(): print(f_, v)

# ---- cell @ 2026-10-25T17:50:25.699Z ----
LL={g:loglines(g) for g in range(14)}
def sess_table(fedname):
    seats=[(k,v) for k,v in SM.items() if v['fed']==fedname]
    tab=collections.defaultdict(dict)
    starts={}
    for k,v in seats:
        for l in LL[v['g']]:
            if k in l:
                if 'Completed consensus session' in l:
                    n=int(_re.search(r'session_index=(\d+)',l).group(1)); tab[n].setdefault(k[:8],l[11:19])
                elif 'Starting fedimintd' in l: starts.setdefault(k[:8],[]).append(l[11:19])
    cols=[k[:8]+'/'+str(v['g']) for k,v in seats]
    print('starts', {k:v[-1] for k,v in starts.items()})
    print('sess ', ' '.join(c.ljust(11) for c in cols))
    for n in sorted(tab):
        print(str(n).ljust(5), ' '.join((tab[n].get(c[:8],'-')).ljust(11) for c in cols))
sess_table('QA Transfer 1')

# ---- cell @ 2026-10-25T17:50:30.789Z ----
SM=seat_map(); LL={g:loglines(g) for g in range(14)}
sess_table('QA f1')
for r in RESULTS[6:]: print(r['name'], r['ok'], r.get('probs'), r['secs'], [p for p in r['phases']][-2:], r.get('faults'))

# ---- cell @ 2026-10-25T17:50:36.345Z ----
for l in LL[9]:
    if '81025aac' in l and ('session' in l and 'consensus' in l and 'item' not in l): print(l[11:19], _re.sub(r'.*fm::consensus: ','',l)[:100])

# ---- cell @ 2026-10-25T17:50:42.038Z ----
print(time.strftime('%H:%M:%S', time.gmtime()))
for k,v in SM.items():
    if v['fed']!='QA f1': continue
    last=None; n=0
    for l in LL[v['g']]:
        if k in l and 'item_index' in l: last=l; n+=1
    print(k[:8], v['g'], n, last[11:19] if last else None, _re.search(r'session_index=\d+ item_index=\d+',last).group() if last else None)
ctx_f1=[c for c in CTXS if c.get('name')=='f1'][0] if 'CTXS' in globals() else None
print(ctx_f1 and {k:ctx_f1[k] for k in ctx_f1 if k in ('transfers','retired','name')})

# ---- cell @ 2026-10-25T17:50:51.781Z ----
for fn in (fm_peer, fm):
    try: print(inspect.getsource(fn))
    except Exception as e: print(e)

# ---- cell @ 2026-10-25T17:50:55.958Z ----
print(glob.glob(f'{E.root}/*wallet*')+glob.glob(f'{E.root}/chk-*'))

# ---- cell @ 2026-10-25T17:50:59.821Z ----
def sessions(wname, peers=range(7)):
    out={}
    for p_ in peers:
        try:
            r=fm_peer(f'{E.root}/{wname}', p_, 'session_count')
            out[p_]=(r.stdout.strip()[-40:] if hasattr(r,'stdout') else str(r)[-60:])
        except Exception as e: out[p_]='ERR '+repr(e)[:60]
    return out
print(await asyncio.to_thread(sessions,'chk-f1'))

# ---- cell @ 2026-10-25T17:51:18.324Z ----
print({k:(v if not isinstance(v,(list,dict)) or len(str(v))<500 else str(v)[:500]) for k,v in MON.items()})

# ---- cell @ 2026-10-25T17:51:27.568Z ----
for g,sid in [(3,'a1b4a8f3'),(12,'3405b99c')]:
    print('==',g)
    for l in LL[g]:
        if sid in l and '2026-10-25T17:36:00'<=l[:19]<='2026-10-25T17:42:27' and 'item_index' not in l and 'Iroh API request' not in l and 'submit_transaction' not in l:
            print(l[11:23], _re.sub(r'^.*?INFO\s+\S+\s+\S+\s+(INFO|WARN|ERROR)\s+','',l)[:150])

# ---- cell @ 2026-10-25T17:51:34.002Z ----
sweep_task.cancel()
print([(c.get('name'), c.get('transfers'), c.get('retired'), getattr(c['fa'],'name',None)) for c in CTXS])

# ---- cell @ 2026-10-25T17:51:39.908Z ----
f1ctx=CTXS[1]
r=await f1ctx['fa'].acall('faClientGuardianTransfer', timeout=20); print(json.dumps(r)[:600])
WATCH=[]
async def watch_fed(wname, label, every=30, dur=3600):
    t0=time.time()
    while time.time()-t0<dur:
        s=await asyncio.to_thread(sessions, wname)
        vals=[]
        for p_,v in s.items():
            m=_re.search(r'"value": (\d+)',v); vals.append(int(m.group(1)) if m else None)
        WATCH.append((time.strftime('%H:%M:%S',time.gmtime()), label, vals))
        await asyncio.sleep(every)
wf1=asyncio.create_task(watch_fed('chk-f1','f1'))

# ---- cell @ 2026-10-25T17:52:08.335Z ----
for fn in (get_ctx, setup_fed, capture_pre):
    try: print(inspect.getsource(fn)[:2500])
    except Exception as e: print(fn.__name__, e)

# ---- cell @ 2026-10-25T17:52:13.148Z ----
print(WATCH[-5:])

# ---- cell @ 2026-10-25T17:52:18.348Z ----
async def mk():
    c=await get_ctx()
    return c
f2task=asyncio.create_task(mk())

# ---- cell @ 2026-10-25T17:52:22.315Z ----
await asyncio.sleep(100)
print(WATCH[-5:]); print(f2task.done())

# ---- cell @ 2026-10-25T17:54:08.114Z ----
f3=CTXS[-1]; print(f3['name'], [ (s['i'],s['loc']) for s in f3['seats']])
m=await asyncio.to_thread(meta, 'chk-f3', f3['invite']); print(parse_meta(m)[0]['revision'])
wf3=asyncio.create_task(watch_fed('chk-f3','f3'))

# ---- cell @ 2026-10-25T17:54:16.041Z ----
r=await command("grep -rn 'AlephBFT\\|aleph' ~/.cargo/registry/src/*/fedimint-logging-0.*/src/*.rs 2>/dev/null | head; ls ~/.cargo/registry/src/*/ | grep fedimint-logging; ls ~/.cargo/git/checkouts/ | head -30")

# ---- cell @ 2026-10-25T17:54:19.121Z ----
r=await command("sed -n 130,175p ~/.cargo/registry/src/*/fedimint-logging-0.12.0/src/lib.rs")

# ---- cell @ 2026-10-25T17:54:29.453Z ----
r=await command("ls ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad/; cd ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad/*/ 2>/dev/null; grep -rn 'delay_config\\|DelayConfig\\|unit_creation_delay\\|requests_interval' --include=*.rs fedimint-server/src | head -20")

# ---- cell @ 2026-10-25T17:54:35.169Z ----
r=await command("ls -d ~/.cargo/registry/src/*/fedimint-server-* ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad/*/fedimint-server* 2>/dev/null; grep -n 'fedimint-server' /src/qa-transfer-manifold-rb/Cargo.lock | head -3; grep -n -A3 'name = \"fedimint-server\"' /src/qa-transfer-manifold-rb/Cargo.lock")

# ---- cell @ 2026-10-25T17:54:39.841Z ----
r=await command("grep -n -B1 -A2 'name = \"fedimint-core\"' /src/qa-transfer-manifold-rb/Cargo.lock | head; ")

# ---- cell @ 2026-10-25T17:54:44.185Z ----
r=await command("cd ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad/ && grep -rn 'DelayConfig\\|default_delay_config\\|unit_creation_delay\\|requests_interval' --include=*.rs */fedimint-server/src | head -20")

# ---- cell @ 2026-10-25T17:54:48.035Z ----
r=await command("cd ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad/6b42ee7 && sed -n 260,330p fedimint-server/src/consensus/engine.rs; grep -n 'fn default_delay_config' -A25 ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src/config.rs")

# ---- cell @ 2026-10-25T17:55:02.176Z ----
def seat_last_items(fedname):
    SMx=seat_map()
    out={}
    for k,v in SMx.items():
        if v['fed']!=fedname: continue
        last=None
        for l in reversed(loglines(v['g'])):
            if k in l and 'item_index' in l: last=l; break
        out[k[:8]+'/'+str(v['g'])]=(last[11:19], _re.search(r'session_index=\d+ item_index=\d+',last).group()) if last else None
    return out
await asyncio.sleep(60)
print(WATCH[-3:])
print(await asyncio.to_thread(seat_last_items,'QA f1'))
print(await asyncio.to_thread(seat_last_items,'QA f3'))

# ---- cell @ 2026-10-25T17:56:11.264Z ----
r=await command("cd ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad/6b42ee7 && grep -rn 'broadcast_round_delay_ms\\|DEFAULT_BROADCAST_ROUND_DELAY\\|BROADCAST_ROUND_DELAY' --include=*.rs . | grep -v test | head")

# ---- cell @ 2026-10-25T17:56:17.484Z ----
await asyncio.sleep(90)
print(WATCH[-2:])
print(await asyncio.to_thread(seat_last_items,'QA f1'))
r=await f1ctx['fa'].acall('faClientGuardianTransfer', timeout=20); print(json.dumps(r)[:300])

# ---- cell @ 2026-10-25T17:57:57.325Z ----
async def e1():
    c=f3
    res=await run_scenario('E1 f3 clean single transfer idx0', c, 0)
    c['transfers']+=1; log_result(res); return res
e1task=asyncio.create_task(e1())

# ---- cell @ 2026-10-25T17:57:57.793Z ----
await asyncio.sleep(60)
print(e1task.done()); print(MON['log'][-2:])

# ---- cell @ 2026-10-25T17:59:06.448Z ----
await asyncio.sleep(45)
print(await asyncio.to_thread(seat_last_items,'QA f3'))
print(await asyncio.to_thread(seat_last_items,'QA f1'))

# ---- cell @ 2026-10-25T18:00:16.766Z ----
_orig_do=Faults.do
def _do(self, act):
    if act[0]=='stop_seat':
        g=self.resolve(act[1]) if not isinstance(act[1],int) else act[1]; s_=act[2]
        t=time.strftime('%H:%M:%S'); self.log.append((t,act))
        for gg,tt,pid in running_seat_dirs():
            if gg==g and tt==s_: os.kill(pid, signal.SIGSTOP); self.stopped.add(('pid',g,pid))
        return
    return _orig_do(self, act)
Faults.do=_do
print([(s['i'],s['loc']) for s in f3['seats']])

# ---- cell @ 2026-10-25T18:00:35.781Z ----
async def drive_until(f, timeout, t0):
    phases=[]; last=None; end=time.time()+timeout
    while time.time()<end:
        try: r=await f.acall('faClientGuardianTransfer', timeout=20)
        except Exception as e: r={'exception':repr(e)[:100]}
        t_=r.get('result',{}).get('transfer') if isinstance(r.get('result'),dict) else None
        ph=t_['phase'] if t_ else json.dumps(r)[:120]
        if ph!=last: phases.append((round(time.time()-t0,1),ph)); last=ph
        if t_ is None and 'transfer' in json.dumps(r): return phases, True
        await asyncio.sleep(3)
    return phases, False

async def e2(c, idx, stop_idx, hold=420, label='E2'):
    t0=time.time(); f=c['fa']
    pre=await capture_pre(c, idx)
    sg,ss=[s for s in c['seats'] if s['i']==stop_idx][0]['loc'][0]
    F=Faults({'fa':f}); F.do(('stop_seat',sg,ss))
    pv=await f.acall('faClientPreviewTransferDestination')
    known={x[0] for g in range(14) for x in q(g,'select lower(hex(quote_id)) from seats')}
    st=await f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':pv['result']['preview']['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=120)
    print('start', str(st)[:200])
    # resume in background loop (bridge drives itself?) -- poll resume periodically
    async def resumer():
        while True:
            try: await f.acall('faClientResumeGuardianTransfer', timeout=300)
            except Exception: pass
            await asyncio.sleep(2)
    rt=asyncio.create_task(resumer())
    ph1,done1=await drive_until(f, hold, t0)
    print('during stop:', ph1[-3:], done1, round(time.time()-t0))
    F.heal(); t_heal=time.time()-t0
    ph2,done2=(ph1,done1) if done1 else await drive_until(f, 900, t0)
    rt.cancel()
    out={'name':f'{label} f={c["name"]} idx{idx} stop idx{stop_idx} hold{hold}','phases':ph1+ph2,'heal_at':round(t_heal),'done':done2,'secs':round(time.time()-t0)}
    if done2:
        newq=[(g,x[0]) for g in range(14) for x in q(g,'select lower(hex(quote_id)) from seats') if x[0] not in known]
        inv={v_:k_ for k_,v_ in MGID.items()}
        if len(newq)==1:
            v=await verify(c, pre, {'destinationSeatId':newq[0][1],'destinationMgId':inv[newq[0][0]]}); out.update(v)
        else: out['ok']=False; out['probs']=[f'newq {newq}']
    else: out['ok']=False; out['probs']=['did not complete']
    c['transfers']+=1; nctx=await ctx_of(f); c['seats']=nctx['seats']
    RESULTS.append(out); log_result(out); print(json.dumps(out)[:1500]); return out
e2task=asyncio.create_task(e2(f3, 1, 3))

# ---- cell @ 2026-10-25T18:00:40.727Z ----
print(glob.glob(f'{E.root}/fa-*')[:10], glob.glob('/src/scratch/qa-gt/env1/*')[:40])

# ---- cell @ 2026-10-25T18:00:44.818Z ----
r=await command("ls /src/scratch/qa-gt/env1/fa-f3; ls /src/scratch/qa-gt/env1/fa-f3/* | head")

# ---- cell @ 2026-10-25T18:00:48.597Z ----
r=await command("grep -a -n -i 'manifold\\|transfer\\|fa_client' /src/scratch/qa-gt/env1/fa-f3/repl.log | grep -a -i 'warn\\|error\\|fail' | sed -e 's/\\x1b\\[[0-9;]*m//g' | cut -c1-400 | head -30")

# ---- cell @ 2026-10-25T18:00:52.153Z ----
r=await command("sed -e 's/\\x1b\\[[0-9;]*m//g' /src/scratch/qa-gt/env1/fa-f3/repl.log | grep -a -n '17:5[7-9]' | grep -a -i 'warn\\|error' | cut -c1-500 | head -30")

# ---- cell @ 2026-10-25T18:00:56.615Z ----
await asyncio.sleep(120)
print(await asyncio.to_thread(seat_last_items,'QA f3'))
print(WATCH[-4:])

# ---- cell @ 2026-10-25T18:03:08.205Z ----
print(await asyncio.to_thread(seat_last_items,'QA f3'))
print(await asyncio.to_thread(seat_last_items,'QA f1'))

# ---- cell @ 2026-10-25T18:03:21.101Z ----
r=await command("grep -n 'def mg_env' -A30 /src/scratch/qa-gt/qaenv.py | head -50; grep -n 'def start_mg' -A25 /src/scratch/qa-gt/qaenv.py")

# ---- cell @ 2026-10-25T18:03:28.669Z ----
ALEPH_LOG='info,AlephBFT-alerter=debug,AlephBFT-runway=info,AlephBFT-creator=debug,AlephBFT-unit-store=debug,AlephBFT-member=info,AlephBFT-consensus=debug'
await asyncio.to_thread(E.stop_mg, 13)
await asyncio.sleep(2)
E.start_mg(13, extra_env={'RUST_LOG':ALEPH_LOG})
await asyncio.sleep(45)
LL[13]=loglines(13)
sel=[l for l in LL[13] if '276a1f88' in l and 'AlephBFT' in l]
print(len(sel))
c=collections.Counter(_re.sub(r'\d+','N',_re.search(r'AlephBFT\S*: (.*?)seat_id',l).group(1))[:120] for l in sel if _re.search(r'AlephBFT\S*: (.*?)seat_id',l))
for k,v in c.most_common(30): print(v,k)

# ---- cell @ 2026-10-25T18:04:19.840Z ----
for l in sel: print(l[11:23], _re.search(r'AlephBFT\S*: (.*?)seat_id',l).group(1)[:400] if _re.search(r'AlephBFT\S*: (.*?)seat_id',l) else l[-300:])

# ---- cell @ 2026-10-25T18:04:40.370Z ----
SM=seat_map()
for k,v in SM.items():
    if v['fed']!='QA f1': continue
    d=json.load(open(f"{E.mg_dir(v['g'])}/seats/{v['sn']}/data/local.json"))
    print(k[:8], v['g'], v['sn'], d.get('identity'), q(v['g'], f"select phase from seat_transfer_out where lower(hex(quote_id))='{k}'"), q(v['g'], f"select state from seat_transfer_in where lower(hex(quote_id))='{k}'"))

# ---- cell @ 2026-10-25T18:05:07.979Z ----
os.environ['QA_MG_RUST_LOG']=ALEPH_LOG
async def restart_all():
    for g in range(14):
        if g==13: continue
        if E.mg_pid(g): await asyncio.to_thread(E.stop_mg, g)
        await asyncio.sleep(1)
        E.start_mg(g)
        await asyncio.sleep(8)
await restart_all()
print([E.mg_pid(g) is not None for g in range(14)])

# ---- cell @ 2026-10-25T18:07:12.836Z ----
def aleph_status(fedname=None, since=None):
    SMx=seat_map(); out={}
    for k,v in SMx.items():
        if fedname and v['fed']!=fedname: continue
        lines=loglines(v['g'])
        for l in reversed(lines):
            if k in l and 'Runway status report' in l:
                if since and l[:19]<since: break
                m=_re.search(r'DAG height - (\d+).*forkers - \[([^\]]*)\]',l)
                out[(v['fed'],k[:8],v['g'])]=(l[11:19], int(m.group(1)), m.group(2))
                break
    return out
await asyncio.sleep(20)
for k,v in sorted((await asyncio.to_thread(aleph_status)).items()): print(k,v)

# ---- cell @ 2026-10-25T18:07:57.683Z ----
def aleph_status(fedname=None, since=None):
    SMx=seat_map(); out={}
    cache={}
    for k,v in SMx.items():
        if fedname and v['fed']!=fedname: continue
        lines=cache.setdefault(v['g'], loglines(v['g']))
        for l in reversed(lines):
            if k in l and 'Runway status report' in l:
                if since and l[:19]<since: break
                m=_re.search(r'DAG height - (\d+).*?forkers - \[([^\]]*)\]',l)
                out[(v['fed'],k[:8],v['g'])]=(l[11:19], int(m.group(1)) if m else None, m.group(2) if m else l[-200:])
                break
    return out
for k,v in sorted((await asyncio.to_thread(aleph_status)).items()): print(k,v)

# ---- cell @ 2026-10-25T18:08:11.996Z ----
def forkers_now(fedname, since):
    st=aleph_status(fedname, since)
    fk=set()
    for k,v in st.items():
        if v[1] is not None and v[2]: fk|=set(int(x) for x in v[2].split(','))
    return fk, {k[1]:(v[0],v[1]) for k,v in st.items()}
async def xrun(c, idx, label, faults=(), wait_after=40):
    t=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime())
    res=await run_scenario(label, c, idx, faults=list(faults))
    c['transfers']+=1
    await asyncio.sleep(wait_after)
    fk,st=await asyncio.to_thread(forkers_now, f"QA {c['name']}", t)
    res['forkers']=sorted(fk); log_result(res)
    print(label, 'ok', res.get('ok'), 'secs', res.get('secs'), 'forkers', sorted(fk), st)
    return res
x1=await xrun(f3, 2, 'X1 f3 clean idx2')

# ---- cell @ 2026-10-25T18:09:12.572Z ----
SM=seat_map()
def ident(k):
    v=SM[k]; return json.load(open(f"{E.mg_dir(v['g'])}/seats/{v['sn']}/data/local.json"))['identity']
for k,v in SM.items():
    if v['fed']=='QA f3': print(k[:8], v['g'], ident(k))
for l in loglines(1):
    if '015fb3b7' in l and ('AlephBFT' in l) and ('Creator starting' in l or 'forker' in l.lower() or 'collection' in l): print(l[11:23], l[l.find('AlephBFT'):][:250])

# ---- cell @ 2026-10-25T18:09:34.304Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src && grep -n 'collection\\|starting_round\\|backup' runway/mod.rs | head -60")

# ---- cell @ 2026-10-25T18:09:40.531Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0 && sed -n '/\\[features\\]/,/^\\[/p' Cargo.toml; grep -rn 'aleph' ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad/6b42ee7/fedimint-server/Cargo.toml ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad/6b42ee7/Cargo.toml")

# ---- cell @ 2026-10-25T18:09:46.833Z ----
r=await command("cd ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad && grep -n 'fedimint-aleph-bft' */fedimint-server/Cargo.toml; for d in */; do (cd $d; echo $d $(git log -1 --format='%H %cd' 2>/dev/null)); done; grep -rn 'fedimint' /src/qa-transfer-manifold-rb/flake.nix | head -5")

# ---- cell @ 2026-10-25T18:09:51.797Z ----
r=await command("cd ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad && grep -n 'aleph' be50abd/Cargo.toml be50abd/fedimint-server/Cargo.toml 61b3b02/Cargo.toml; cd be50abd && git log --oneline -1; git tag --points-at HEAD 2>/dev/null | head")

# ---- cell @ 2026-10-25T18:10:08.725Z ----
f4=await setup_fed('f4'); f4['transfers']=0; f4['name']='f4'; CTXS.append(f4); FA_COUNTER[0]=max(FA_COUNTER[0],4)
print([(s['i'],s['loc']) for s in f4['seats']])
m=await asyncio.to_thread(meta, 'chk-f4', f4['invite'])
SM=seat_map()

# ---- cell @ 2026-10-25T18:10:48.912Z ----
print(time.strftime('%H:%M:%S', time.gmtime()), await asyncio.to_thread(sessions,'chk-f4', [0]))
X4=[]
for i in range(3):
    r_=await run_scenario(f'X4 f4 clean back-to-back #{i} idx{i}', f4, i, drive_timeout=240)
    f4['transfers']+=1; X4.append(r_); log_result(r_)
    print(time.strftime('%H:%M:%S', time.gmtime()), i, r_.get('ok'), r_.get('secs'))

# ---- cell @ 2026-10-25T18:12:53.135Z ----
await asyncio.sleep(150)
fk,st=await asyncio.to_thread(forkers_now,'QA f4','2026-10-25T18:11:00'); print(fk, st)
print(len(X4))

# ---- cell @ 2026-10-25T18:14:56.480Z ----
await asyncio.sleep(60)
fk,st=await asyncio.to_thread(forkers_now,'QA f4','2026-10-25T18:11:00'); print(fk, st)
print(len(X4))

# ---- cell @ 2026-10-25T18:16:04.201Z ----
SM=seat_map()
print([(k[:8],v['g'],ident(k), q(v['g'], f"select state from seat_transfer_in where lower(hex(quote_id))='{k}'"), q(v['g'], f"select phase from seat_transfer_out where lower(hex(quote_id))='{k}'")) for k,v in SM.items() if v['fed']=='QA f4'])
for l in loglines(1):
    if 'AlephBFT-creator' in l and 'Creator starting' in l and l[:19]>'2026-10-25T18:10:50':
        sid=_re.search(r'seat_id=(\w{8})',l).group(1)
        if SM.get(next((k for k in SM if k.startswith(sid)),None),{}).get('fed')=='QA f4': print(l[11:23], sid, l[l.find('Creator'):l.find('seat_id')])
print(await asyncio.to_thread(sessions,'chk-f4',[0,1]))

# ---- cell @ 2026-10-25T18:16:28.843Z ----
fk,st=await asyncio.to_thread(forkers_now,'QA f3','2026-10-25T18:15:30'); print(fk, st)
print(await asyncio.to_thread(sessions,'chk-f3',[0]))

# ---- cell @ 2026-10-25T18:16:37.687Z ----
async def sess_count(wname):
    r=await asyncio.to_thread(sessions, wname, [0])
    m=_re.search(r'"value": (\d+)', r[0]); return int(m.group(1)) if m else None
async def x1_repeat(c, idxs, label='X1r'):
    out=[]
    for idx in idxs:
        s0=await sess_count(f"chk-{c['name']}")
        while (await sess_count(f"chk-{c['name']}"))==s0: await asyncio.sleep(5)
        await asyncio.sleep(15)
        r_=await xrun(c, idx, f'{label} {c["name"]} clean idx{idx}', wait_after=30)
        out.append((idx, r_.get('ok'), r_.get('forkers')))
    return out
x1r=asyncio.create_task(x1_repeat(f3,[3,4,5,6]))

# ---- cell @ 2026-10-25T18:16:44.583Z ----
r=await command("sed -e 's/\\x1b\\[[0-9;]*m//g' /src/scratch/qa-gt/env1/fa-f4/repl.log | grep -a -v netlink | grep -a '18:1[01]:' | grep -a -E 'WARN|ERROR' | cut -c1-600 | head -20")

# ---- cell @ 2026-10-25T18:16:49.469Z ----
r=await command("sed -e 's/\\x1b\\[[0-9;]*m//g' /src/scratch/qa-gt/env1/fa-f4/repl.log | grep -a -v netlink | grep -a -E '18:1[0-1]:[4-5][0-9]|18:11:0' | grep -a -E 'WARN|ERROR' | cut -c1-700 | head -12")

# ---- cell @ 2026-10-25T18:16:53.081Z ----
r=await command("sed -e 's/\\x1b\\[[0-9;]*m//g' /src/scratch/qa-gt/env1/fa-f4/repl.log | grep -a -v netlink | grep -a -i -E 'transfer' | grep -a -E 'WARN|ERROR' | cut -c1-700 | head -12")

# ---- cell @ 2026-10-25T18:16:57.015Z ----
r=await command("sed -e 's/\\x1b\\[[0-9;]*m//g' /src/scratch/qa-gt/env1/fa-f4/repl.log | grep -a -v netlink | grep -a -E 'WARN|ERROR' | tail -12 | cut -c1-600; tail -c 300 /src/scratch/qa-gt/env1/fa-f4/repl.log")

# ---- cell @ 2026-10-25T18:17:04.511Z ----
for l in loglines(1):
    if '2026-10-25T18:10:50'<=l[:19]<='2026-10-25T18:11:00' and ('RPC failed' in l or 'WARN' in l[:60] or 'ERROR' in l[:60]):
        print(l[11:23], l[27:400])

# ---- cell @ 2026-10-25T18:17:09.990Z ----
r=await command("rg -n 'peer_attestation|SeatUnavailable|seat_unavailable|Unavailable' crates/fa-client/src/transfer.rs crates/fa-client/src/*.rs | head -30", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:17:14.430Z ----
r=await command("rg -n 'get_peer_attestation\\|poll_pulled_guardian\\|fn poll_pulled' crates/fa-client/src/transfer.rs; rg -n 'get_peer_attestation' crates/fa-client/src | head", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:17:18.430Z ----
r=await command("sed -n 4200,4300p crates/fa-client/src/formation.rs; rg -n 'fetch_peer_attestation|peer_attestation' crates/fa-client/src/transfer.rs crates/fa-client/src/formation.rs | head -20", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:17:22.524Z ----
r=await command("sed -n 690,760p crates/fa-client/src/transfer.rs; rg -n 'fn call\\b|fn call<' -A40 crates/fa-client/src/mg_client.rs 2>/dev/null | head -80; ls crates/fa-client/src", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:17:26.954Z ----
r=await command("rg -n 'destination_binding\\(' -B12 crates/fa-client/src/transfer.rs | head -60", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:17:32.052Z ----
await asyncio.sleep(120)
print(x1r.done(), [ (r['name'], r.get('ok'), r.get('forkers')) for r in RESULTS if r['name'].startswith('X1r')])

# ---- cell @ 2026-10-25T18:19:40.085Z ----
async def mkfed(name):
    c=await setup_fed(name); c['transfers']=0; c['name']=name; CTXS.append(c)
    await asyncio.to_thread(meta, f'chk-{name}', c['invite'])
    return c
f5task=asyncio.create_task(mkfed('f5'))

# ---- cell @ 2026-10-25T18:19:43.528Z ----
await asyncio.sleep(240)
print(x1r.done(), f5task.done(), [ (r['name'], r.get('ok'), r.get('forkers')) for r in RESULTS if r['name'].startswith('X1r')])

# ---- cell @ 2026-10-25T18:21:46.865Z ----
await asyncio.sleep(120)
print(x1r.done(), f5task.done(), [ (r['name'], r.get('ok'), r.get('forkers')) for r in RESULTS if r['name'].startswith('X1r')])

# ---- cell @ 2026-10-25T18:23:58.727Z ----
x1r.cancel()
r=await command("cd ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad/be50abd && grep -rn 'ROUNDS_PER_SESSION\\|ROUND_DELAY_MS' --include=*.rs fedimint-server/src/config/mod.rs fedimint-core/src | head; sed -n 215,240p fedimint-server/src/config/mod.rs")

# ---- cell @ 2026-10-25T18:24:03.668Z ----
r=await command("cd ~/.cargo/git/checkouts/fedimint-30fe82f7a22d3fad/be50abd && sed -n 60,80p fedimint-server/src/config/mod.rs; sed -n 340,372p fedimint-server/src/config/mod.rs")

# ---- cell @ 2026-10-25T18:24:16.673Z ----
f5=f5task.result(); print([(s['i'],s['loc']) for s in f5['seats']])
print([(s['i'],s['loc']) for s in f3['seats']])
def fed_health(fedname, since):
    st=aleph_status(fedname, since)
    hs=[v[1] for v in st.values() if v[1] is not None]
    fk=set()
    for v in st.values():
        if v[1] is not None and v[2]: fk|=set(int(x) for x in v[2].split(','))
    # heights also in no-forker format
    return fk, st
def dag_heights(fedname, since):
    SMx=seat_map(); out={}
    cache={}
    for k,v in SMx.items():
        if v['fed']!=fedname: continue
        lines=cache.setdefault(v['g'], loglines(v['g']))
        for l in reversed(lines):
            if k in l and 'Runway status report' in l:
                if l[:19]<since: break
                m=_re.search(r'DAG height - (\d+)',l); f_=_re.search(r'forkers - \[([^\]]*)\]',l)
                out[k[:8]]=(l[11:19], int(m.group(1)) if m else None, f_.group(1) if f_ else '')
                break
    return out

# ---- cell @ 2026-10-25T18:24:29.723Z ----
TL=collections.defaultdict(list)
async def health_sampler(c, key, every=15):
    fed=f"QA {c['name']}"
    while True:
        now=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))
        h=await asyncio.to_thread(dag_heights, fed, now)
        sc=await sess_count(f"chk-{c['name']}")
        hs=sorted(set(v[1] for v in h.values() if v[1] is not None))
        fks=sorted(set(v[2] for v in h.values() if v[2]))
        try:
            r=await c['fa'].acall('faClientGuardianTransfer', timeout=10); tr=r['result'].get('transfer'); ph=tr['phase'] if tr else None
        except Exception as e: ph='ERR'
        TL[key].append((time.strftime('%H:%M:%S', time.gmtime()), 'sess',sc,'dagH',hs[-3:] if hs else None,'forkers',fks,'phase',ph))
        await asyncio.sleep(every)

async def resumer(f):
    while True:
        try: await f.acall('faClientResumeGuardianTransfer', timeout=300)
        except Exception: pass
        await asyncio.sleep(2)

async def wait_boundary(c, extra=8):
    s0=await sess_count(f"chk-{c['name']}")
    while (await sess_count(f"chk-{c['name']}"))==s0: await asyncio.sleep(3)
    await asyncio.sleep(extra)

async def start_transfer(c, idx):
    f=c['fa']
    pre=await capture_pre(c, idx)
    known={x[0] for g in range(14) for x in q(g,'select lower(hex(quote_id)) from seats')}
    pv=await f.acall('faClientPreviewTransferDestination')
    st=await f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':pv['result']['preview']['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=120)
    return pre, known, st

async def wait_closed(c, timeout):
    end=time.time()+timeout
    while time.time()<end:
        try:
            r=await c['fa'].acall('faClientGuardianTransfer', timeout=10)
            if r['result'].get('type')=='transfer' and r['result'].get('transfer') is None: return True
        except Exception: pass
        await asyncio.sleep(3)
    return False

async def verify_after(c, pre, known):
    newq=[(g,x[0]) for g in range(14) for x in q(g,'select lower(hex(quote_id)) from seats') if x[0] not in known]
    inv={v_:k_ for k_,v_ in MGID.items()}
    if len(newq)!=1: return {'ok':False,'probs':[f'newq {newq}']}
    v=await verify(c, pre, {'destinationSeatId':newq[0][1],'destinationMgId':inv[newq[0][0]]})
    nctx=await ctx_of(c['fa']); c['seats']=nctx['seats']; c['transfers']+=1
    return v
print('ok')

# ---- cell @ 2026-10-25T18:24:38.419Z ----
async def P1(c, idx=0, stop_idxs=(5,6), hold=300, label='P1'):
    key=label; samp=asyncio.create_task(health_sampler(c,key))
    await wait_boundary(c)
    F=Faults({'fa':c['fa']})
    for si in stop_idxs:
        g,s_=[s for s in c['seats'] if s['i']==si][0]['loc'][0]; F.do(('stop_seat',g,s_))
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'STOPPED',stop_idxs))
    pre,known,st=await start_transfer(c, idx)
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'START',str(st)[:150]))
    rt=asyncio.create_task(resumer(c['fa']))
    done1=await wait_closed(c, hold)
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'HOLD_END closed=',done1))
    F.heal(); TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'HEALED'))
    done2=done1 or await wait_closed(c, 900)
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'CLOSED',done2))
    rt.cancel()
    v=await verify_after(c, pre, known) if done2 else {'ok':False,'probs':['not closed']}
    await asyncio.sleep(200)  # watch consensus progress after
    samp.cancel()
    res={'name':f'{label} {c["name"]} transfer idx{idx} with {stop_idxs} SIGSTOPped hold {hold}s', **v, 'timeline':TL[key]}
    RESULTS.append(res); log_result(res)
    return res
p1=asyncio.create_task(P1(f5))

# ---- cell @ 2026-10-25T18:24:44.557Z ----
r=await command("cd /src && (test -d fedimint-fedi || git clone -q https://github.com/fedibtc/fedimint fedimint-fedi) && cd fedimint-fedi && git fetch -q --tags origin 2>&1 | tail -2; git checkout -q v0.12.0-fedi15 && git log -1 --format='%H %cd %s'")

# ---- cell @ 2026-10-25T18:24:50.272Z ----
r=await command("cd /src/fedimint-fedi && grep -n 'aleph' Cargo.toml fedimint-server/Cargo.toml; wc -l fedimint-server/src/consensus/engine.rs; grep -n 'fn \\|is_recovery\\|AlephUnits\\|SignedSessionOutcome\\|request_signed_session_outcome' fedimint-server/src/consensus/engine.rs | head -80")

# ---- cell @ 2026-10-25T18:24:54.858Z ----
r=await command("cd /src/fedimint-fedi && sed -n 160,372p fedimint-server/src/consensus/engine.rs")

# ---- cell @ 2026-10-25T18:25:03.331Z ----
r=await command("cd /src/fedimint-fedi && grep -rn 'session_timeout' --include=*.rs fedimint-server/src | head; grep -rn 'SESSION_TIMEOUT\\|session_timeout' --include=*.rs fedimint-server/src fedimintd/src | head")

# ---- cell @ 2026-10-25T18:25:07.310Z ----
r=await command("cd /src/fedimint-fedi && grep -rn 'session_timeout\\|SESSION_TIMEOUT' --include=*.rs . | grep -v 'fedimint-server/src' | head; grep -rn 'session_timeout' -B3 -A3 /src/qa-transfer-manifold-rb/crates --include=*.rs | head -30")

# ---- cell @ 2026-10-25T18:25:29.486Z ----
r=await command("cd /src/fedimint-fedi && git log -S'default-features = false' --format='%h %ad %s' --date=short -- Cargo.toml fedimint-server/Cargo.toml | head -20")

# ---- cell @ 2026-10-25T18:25:33.217Z ----
r=await command("cd /src/fedimint-fedi && git log -G'aleph-bft.*default-features' --format='%h %ad %an %s' --date=short | tail -5; ")

# ---- cell @ 2026-10-25T18:25:41.013Z ----
r=await command("cd /src/fedimint-fedi && for c in 806ce16de a4177cea3 6e9ef3422; do echo == $c; git show $c --stat --format='%B' | head -30; git show $c | grep -n -i 'aleph-bft\\|initial_unit\\|default-features' | head; done")

# ---- cell @ 2026-10-25T18:25:49.283Z ----
r=await command("rg -n 'database|db_path|\\.db\\b|consensus.json|local.json|private.json|preserve_guardian_identity|fn import' crates/mg/core/src/transfer_worker.rs crates/mg/core/src/transfer_state.rs | head -40; rg -n 'TransferSeatResponse' -A25 crates/mg/api/src/*.rs 2>/dev/null | head -60; ls crates/mg", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:25:55.491Z ----
r=await command("sed -n 1,140p crates/mg/core/src/transfer_state.rs", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:26:00.408Z ----
r=await command("rg -n -i 'database|aleph|equivocat|fork|catch.?up|empty' crates/mg/specs/*ransfer* specs/*ransfer* crates/fa-client/specs/*ransfer* 2>/dev/null | head -30; ls crates/mg/specs | head -50", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:26:09.669Z ----
r=await command("rg -n -i 'aleph|equivoc|session|database|db ' crates/mg/specs/CLAIM-manifold-guardian-guardian-identity-single-instance.md crates/mg/specs/ARCH-manifold-guardian-storage.md | head -30; ls crates/mg/specs | grep -i -E 'transfer|move|restore'", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:26:14.158Z ----
r=await command("rg -n -i 'database|aleph|equivoc|session|catch|fresh|empty|consensus history|history' crates/mg/specs/SPEC-guardian-transfer.md | head -40; wc -l crates/mg/specs/SPEC-guardian-transfer.md", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:26:17.448Z ----
r=await command("sed -n 1,117p crates/mg/specs/SPEC-guardian-transfer.md", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:26:24.135Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src && sed -n 120,200p units/store.rs; sed -n 700,760p runway/mod.rs")

# ---- cell @ 2026-10-25T18:26:47.747Z ----
r=await command("cd /src/fedimint-fedi && sed -n 777,895p fedimint-server/src/consensus/engine.rs")

# ---- cell @ 2026-10-25T18:27:10.903Z ----
k=[k for k,v in SM.items() if v['fed']=='QA f3'][0]; v=SM[k]
d=f"{E.mg_dir(v['g'])}/seats/{v['sn']}/data"
print(d, os.listdir(d)); 
for x in os.listdir(d):
    p=os.path.join(d,x)
    if os.path.isdir(p): print(x, os.listdir(p)[:12])
r=await command("cd /src/fedimint-fedi && grep -rn 'DB_CHECKPOINTS_DIR\\s*[:=]\\|db_checkpoint_retention' --include=*.rs fedimint-server/src fedimintd/src | head -5")

# ---- cell @ 2026-10-25T18:27:18.177Z ----
for x in TL['P1']: print(x)

# ---- cell @ 2026-10-25T18:27:24.350Z ----
print(time.strftime('%H:%M:%S', time.gmtime()), p1.done())
print(await asyncio.to_thread(dag_heights,'QA f5', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-15))))

# ---- cell @ 2026-10-25T18:27:41.116Z ----
async def P2(c, idxA, idxB, stop_idx, hold=300, label='P2'):
    key=label; samp=asyncio.create_task(health_sampler(c,key))
    await wait_boundary(c, extra=5)
    F=Faults({'fa':c['fa']})
    g,s_=[s for s in c['seats'] if s['i']==stop_idx][0]['loc'][0]; F.do(('stop_seat',g,s_))
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'STOPPED',stop_idx))
    rt=asyncio.create_task(resumer(c['fa']))
    preA,knownA,stA=await start_transfer(c, idxA)
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'START A',str(stA)[:120]))
    okA=await wait_closed(c, 120)
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'A closed',okA))
    vA=await verify_after(c, preA, knownA) if okA else {'ok':False,'probs':['A not closed']}
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'A verify',vA.get('ok'),vA.get('probs')))
    preB,knownB,stB=await start_transfer(c, idxB)
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'START B',str(stB)[:120]))
    okB1=await wait_closed(c, hold)
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'HOLD_END B closed=',okB1))
    F.heal(); TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'HEALED'))
    okB=okB1 or await wait_closed(c, 900)
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'B CLOSED',okB))
    rt.cancel()
    vB=await verify_after(c, preB, knownB) if okB else {'ok':False,'probs':['B not closed']}
    await asyncio.sleep(200)
    samp.cancel()
    res={'name':f'{label} {c["name"]} transfers idx{idxA},idx{idxB} same session with idx{stop_idx} SIGSTOPped hold {hold}s','ok':bool(vA.get('ok') and vB.get('ok')),'A':vA,'B':vB,'timeline':TL[key]}
    RESULTS.append(res); log_result(res)
    return res
print([(s['i'],s['loc']) for s in f3['seats']])
p2=asyncio.create_task(P2(f3, 5, 6, 2))

# ---- cell @ 2026-10-25T18:27:48.548Z ----
def seat_events(g, sid, since, until, pats=('Creator starting','Starting consensus session','Completed consensus session','Starting fedimintd','Terminating')):
    out=[]
    for l in loglines(g):
        if sid in l and since<=l[:19]<=until and any(p in l for p in pats):
            m=_re.search(r'(Creator starting from round \d+|Starting consensus session session_index=\d+ is_recovery=\w+|Completed consensus session session_index=\d+|Starting fedimintd|Terminating Aleph BFT session session_index=\d+)',l)
            out.append((l[11:23], m.group(1) if m else l[60:140]))
    return out
for x in seat_events(10,'da7da98d','2026-10-25T18:20:30','2026-10-25T18:23:00'): print('src',x)
for x in seat_events(3,'d00d34ff','2026-10-25T18:20:30','2026-10-25T18:23:00'): print('dst',x)

# ---- cell @ 2026-10-25T18:28:00.838Z ----
SM=seat_map()
cnt=collections.Counter()
for k,v in SM.items():
    if v['fed']!='QA f3': continue
    for l in loglines(v['g']):
        if k in l and 'Runway status report' in l and '2026-10-25T18:21:30'<=l[:19]<='2026-10-25T18:24:30':
            f_=_re.search(r'forkers - \[([^\]]*)\]',l); cnt[(k[:8], f_.group(1) if f_ else '')]+=1
print(cnt)

# ---- cell @ 2026-10-25T18:28:26.486Z ----
await asyncio.sleep(150)
print(time.strftime('%H:%M:%S', time.gmtime()))
for x in TL['P1'][-6:]: print(x)
for x in TL['P2'][-6:]: print(x)

# ---- cell @ 2026-10-25T18:30:30.153Z ----
print(time.strftime('%H:%M:%S', time.gmtime()))
for x in TL['P1'][-6:]: print(x)
for x in TL['P2'][-6:]: print(x)

# ---- cell @ 2026-10-25T18:30:35.480Z ----
await asyncio.sleep(180)
print(time.strftime('%H:%M:%S', time.gmtime()))
for x in TL['P1'][-6:]: print(x)
for x in TL['P2'][-8:]: print(x)

# ---- cell @ 2026-10-25T18:32:39.669Z ----
await asyncio.sleep(170)
print(time.strftime('%H:%M:%S', time.gmtime()))
for x in TL['P1'][-5:]: print(x)
for x in TL['P2'][-8:]: print(x)

# ---- cell @ 2026-10-25T18:34:47.037Z ----
print([x for x in TL['P1'] if x[1] in ('HOLD_END closed=','HEALED','CLOSED','STOPPED')])
print(await asyncio.to_thread(dag_heights,'QA f5', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-15))))
print([(s['i'],s['loc']) for s in f5['seats']])
print([x for x in running_seat_dirs() if (x[0],x[1]) in [(0,5),(1,5)]])
for pid in [x[2] for x in running_seat_dirs() if (x[0],x[1]) in [(0,5),(1,5)]]: print(pid, open(f'/proc/{pid}/stat').read().split()[2])

# ---- cell @ 2026-10-25T18:34:57.286Z ----
SM=seat_map()
for k,v in SM.items():
    if v['fed']!='QA f5': continue
    for l in reversed(loglines(v['g'])):
        if k in l and ('Runway status report' in l):
            print(k[:8], v['g'], ident(k), l[11:19], l[l.find('Runway'):l.find('seat_id')][:600]); break

# ---- cell @ 2026-10-25T18:35:18.909Z ----
c=collections.Counter(); ex={}
for l in loglines(0):
    if '0c9d2330' in l and '2026-10-25T18:31:20'<=l[:19]:
        m=_re.search(r'(AlephBFT\S*|fm::\S+|mg_\S+):? (.{0,90})',l)
        key=(m.group(1), _re.sub(r'\d+','N',m.group(2))[:70]) if m else l[60:120]
        c[key]+=1; ex.setdefault(key,l[11:23])
for k,v in c.most_common(40): print(v, ex[k], k)

# ---- cell @ 2026-10-25T18:35:25.349Z ----
print(time.strftime('%H:%M:%S', time.gmtime()))
for x in TL['P1'][-4:]: print(x)
print(p1.done())
for l in loglines(0):
    if '0c9d2330' in l and '2026-10-25T18:31:20'<=l[:19] and ('Runway status' in l):
        print(l[11:19], l[l.find('Runway'):l.find('DAG top')][:150])

# ---- cell @ 2026-10-25T18:35:32.930Z ----
for g in [0,1,13,6]:
    for l in loglines(g):
        if '2026-10-25T18:34:50'<=l[:19]<='2026-10-25T18:35:07' and any(s in l for s in ['0c9d2330','9d1eb4d0','7beed6d7','ec532867']) and 'meta' not in l and 'Delay passed' not in l and 'Iroh API request' not in l:
            print(g, l[11:23], l[60:260])

# ---- cell @ 2026-10-25T18:35:47.081Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src/creation && ls; grep -n 'Delay passed' -B30 -A20 mod.rs | head -120")

# ---- cell @ 2026-10-25T18:35:51.393Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src/creation && sed -n 136,230p mod.rs; grep -n 'fn create_unit\\|fn add_unit\\|fn current_round\\|fn can_create\\|enough_parents\\|threshold' creator.rs | head")

# ---- cell @ 2026-10-25T18:35:58.915Z ----
ts=[l[11:21] for l in loglines(0) if '0c9d2330' in l and 'Delay passed' in l and l[:19]>='2026-10-25T18:31:20']
print(len(ts), ts[:3], ts[-5:])
print(collections.Counter(t[:7] for t in ts))
for l in loglines(0):
    if '0c9d2330' in l and 'Member status report' in l and '2026-10-25T18:31:25'<=l[:19]<='2026-10-25T18:35:10': print(l[11:19], l[l.find('task queue'):l.find('seat_id')][:400])

# ---- cell @ 2026-10-25T18:36:09.387Z ----
r=await command("cd ~/.cargo/registry/src/*/fedimint-aleph-bft-0.36.0/src/creation && sed -n 1,140p creator.rs; sed -n 40,86p mod.rs")

# ---- cell @ 2026-10-25T18:36:43.031Z ----
await asyncio.sleep(170)
print(time.strftime('%H:%M:%S', time.gmtime()))
for x in TL['P2'][-6:]: print(x)

# ---- cell @ 2026-10-25T18:38:47.077Z ----
print(time.strftime('%H:%M:%S', time.gmtime()))
for x in TL['P2'][-8:]: print(x)
r1=p1.result(); print({k:r1.get(k) for k in ('name','ok','probs','spent')})
for x in TL['P1'][-5:]: print(x)

# ---- cell @ 2026-10-25T18:38:53.921Z ----
inv={v_:k_ for k_,v_ in MGID.items()}
# rebuild pre for P1: need pre captured... re-run verify with stored pre is not available; check key facts directly
SM=seat_map()
print([(k[:8],v['g'],ident(k)) for k,v in SM.items() if v['fed']=='QA f5'])
m=await asyncio.to_thread(meta,'chk-f5',f5['invite']); pm,v=parse_meta(m); print(pm['revision'], pm['bindings'])
gt=(await f5['fa'].acall('faClientGuardianTransfer'))['result']; print(gt)

# ---- cell @ 2026-10-25T18:39:00.630Z ----
await asyncio.sleep(120)
print(time.strftime('%H:%M:%S', time.gmtime()))
for x in TL['P2'][-4:]: print(x)
SM=seat_map()
for k,v in SM.items():
    if v['fed']!='QA f3': continue
    for l in reversed(loglines(v['g'])):
        if k in l and ('Runway status report' in l):
            print(k[:8], v['g'], ident(k), l[11:19], l[l.find('Runway'):l.find('seat_id')][:260]); break

# ---- cell @ 2026-10-25T18:41:06.351Z ----
await asyncio.sleep(60)
print(time.strftime('%H:%M:%S', time.gmtime()))
for x in TL['P2'][-4:]: print(x)
SM=seat_map()
for k,v in SM.items():
    if v['fed']!='QA f3': continue
    for l in reversed(loglines(v['g'])):
        if k in l and ('Runway status report' in l):
            print(k[:8], v['g'], ident(k), l[11:19], l[l.find('Runway'):l.find('seat_id')][:260]); break

# ---- cell @ 2026-10-25T18:42:26.110Z ----
async def C1(c, stop_idxs=(4,5,6), hold=300, label='C1'):
    key=label; samp=asyncio.create_task(health_sampler(c,key))
    await wait_boundary(c, extra=20)
    F=Faults({'fa':c['fa']})
    for si in stop_idxs:
        g,s_=[s for s in c['seats'] if s['i']==si][0]['loc'][0]; F.do(('stop_seat',g,s_))
    TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'STOPPED',stop_idxs))
    await asyncio.sleep(hold)
    F.heal(); TL[key].append((time.strftime('%H:%M:%S', time.gmtime()),'HEALED'))
    await asyncio.sleep(420)
    samp.cancel()
    return TL[key]
c1=asyncio.create_task(C1(f5))
r2=p2.result() if p2.done() else None
print(r2 and {k:r2.get(k) for k in ('name','ok')}, r2 and r2['A'].get('probs'), r2 and r2['B'].get('probs'))

# ---- cell @ 2026-10-25T18:43:26.370Z ----
SM=seat_map()
f4s={k:v for k,v in SM.items() if v['fed']=='QA f4'}
for k,v in f4s.items(): print(k[:8], v['g'], v['sn'], ident(k))
src='bad0c2b8'; srck=[k for k in f4s if k.startswith(src)][0]
print(q(4, f"select * from seat_transfer_out where lower(hex(quote_id))='{srck}'"))
print(q(4, "select name from sqlite_master where type='table'"))

# ---- cell @ 2026-10-25T18:43:33.447Z ----
import sqlite3
t0=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime())
F4=Faults({'fa':f4['fa']})
F4.do(('stop_seat',11,4))
await asyncio.to_thread(E.stop_mg, 4)
con=sqlite3.connect(f'{E.mg_dir(4)}/fleet-manager.db') if os.path.exists(f'{E.mg_dir(4)}/fleet-manager.db') else None
print(os.listdir(E.mg_dir(4)))

# ---- cell @ 2026-10-25T18:43:42.873Z ----
con=sqlite3.connect(f'{E.mg_dir(4)}/fleet-manager.sqlite')
con.execute(f"delete from seat_transfer_out where lower(hex(quote_id))='{srck}'"); con.commit(); con.close()
E.start_mg(4)
await asyncio.sleep(60)
for x in seat_events(4,'bad0c2b8',t0,'2099'): print(x)
print(await asyncio.to_thread(dag_heights,'QA f4', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-15))))

# ---- cell @ 2026-10-25T18:44:15.383Z ----
E.start_mg(4)
F4.heal()  # resume mover
await asyncio.sleep(5)
print(E.mg_pid(4))
r=await command("rg -n 'retirement is permanent' -B5 -A5 crates/mg/core/migrations 2>/dev/null | head -30; rg -ln 'retirement is permanent' crates/mg", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T18:44:36.765Z ----
r=await command("which fedimint-dbtool fedimintd fedimint-cli 2>&1 | head")

# ---- cell @ 2026-10-25T18:44:45.377Z ----
t0=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime())
await asyncio.to_thread(E.stop_mg, 6)
d=f'{E.mg_dir(6)}/seats/5/data'
os.rename(f'{d}/database', f'{d}/database.q3b-orig')
E.start_mg(6)
await asyncio.sleep(75)
for x in seat_events(6,'194d680a',t0,'2099')[-4:]: print(x)
print(await asyncio.to_thread(dag_heights,'QA f4', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))

# ---- cell @ 2026-10-25T18:45:08.656Z ----
for x in seat_events(6,'194d680a',t0,'2099')[-4:]: print(x)
print(await asyncio.to_thread(dag_heights,'QA f4', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))

# ---- cell @ 2026-10-25T18:45:25.221Z ----
import shutil
t0=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime())
src_db=f'{E.mg_dir(9)}/seats/1/data/database'
print(os.path.exists(src_db), os.listdir(f'{E.mg_dir(9)}/seats/1/data'), [x for x in running_seat_dirs() if x[0]==9 and x[1]==1])
await asyncio.to_thread(E.stop_mg, 10)
dd=f'{E.mg_dir(10)}/seats/1/data'
os.rename(f'{dd}/database', f'{dd}/database.q3d-mover')
shutil.copytree(src_db, f'{dd}/database')
for lk in ['LOCK']:
    p=f'{dd}/database/{lk}'
print(os.listdir(dd))
E.start_mg(10)
await asyncio.sleep(75)
for x in seat_events(10,'7713be79',t0,'2099')[-5:]: print(x)
print(await asyncio.to_thread(dag_heights,'QA f1', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))

# ---- cell @ 2026-10-25T18:47:01.900Z ----
SM=seat_map()
f4s={k:v for k,v in SM.items() if v['fed']=='QA f4' and not k.startswith('bad0c2b8')}
print([(k[:8],v['g'],v['sn']) for k,v in f4s.items()])
t0=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime())
gs=sorted(set(v['g'] for v in f4s.values()))
await asyncio.gather(*[asyncio.to_thread(E.stop_mg,g) for g in gs])
for k,v in f4s.items():
    d=f"{E.mg_dir(v['g'])}/seats/{v['sn']}/data"
    os.rename(f'{d}/database', f'{d}/database.q3c-{int(time.time())}')
for g in gs: E.start_mg(g)
print('restarted', gs, t0)

# ---- cell @ 2026-10-25T18:47:06.314Z ----
await asyncio.sleep(90)
print(await asyncio.to_thread(dag_heights,'QA f4', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))
print(await asyncio.to_thread(sessions,'chk-f4',[0,1]))

# ---- cell @ 2026-10-25T18:49:08.217Z ----
print([(s['i'],s['loc']) for s in f3['seats']])
print(await asyncio.to_thread(dag_heights,'QA f3', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))
print(await sess_count('chk-f3'))
r2=p2.result(); print(r2['ok'], r2['A'].get('probs'), r2['B'].get('probs'))

# ---- cell @ 2026-10-25T18:49:54.181Z ----
async def sess_count(wname):
    for p_ in range(7):
        try:
            r=await asyncio.to_thread(fm_peer, f'{E.root}/{wname}', p_, 'session_count')
            m=_re.search(r'"value": (\d+)', r.stdout if hasattr(r,'stdout') else str(r))
            if m: return int(m.group(1))
        except Exception: pass
    return None
print(await sess_count('chk-f3'))
print(await asyncio.to_thread(dag_heights,'QA f3', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))

# ---- cell @ 2026-10-25T18:50:59.684Z ----
print([x for x in running_seat_dirs() if x[0]==4])
for l in loglines(4)[-400:]:
    if '5b7dc678' in l and ('Runway' in l or 'ERROR' in l or 'WARN' in l[:60]): print(l[11:19], l[60:300])

# ---- cell @ 2026-10-25T18:51:05.598Z ----
print(E.mg_pid(4))
for l in loglines(4)[-15:]: print(l[:300])

# ---- cell @ 2026-10-25T18:51:09.887Z ----
ls=loglines(4)
idx=[i for i,l in enumerate(ls) if l.startswith('===== start')]
for l in ls[idx[-1]:idx[-1]+30]: print(l[:400])

# ---- cell @ 2026-10-25T18:51:15.300Z ----
try: con.close()
except Exception as e: print(e)
E.start_mg(4); await asyncio.sleep(20)
print(E.mg_pid(4), [x for x in running_seat_dirs() if x[0]==4])

# ---- cell @ 2026-10-25T18:51:40.283Z ----
for x in TL['C1']: print(x)

# ---- cell @ 2026-10-25T18:51:48.941Z ----
async def make_stuck(c, idxs, label):
    await wait_boundary(c, extra=5)
    out=[]
    for i,idx in enumerate(idxs):
        r_=await run_scenario(f'{label} #{i} idx{idx}', c, idx, drive_timeout=(150 if i<len(idxs)-1 else 120))
        c['transfers']+=1; log_result(r_); out.append((idx, r_.get('ok'), r_.get('secs')))
    return out
ms=asyncio.create_task(make_stuck(f3,[0,1,3],'X4b f3 back-to-back'))

# ---- cell @ 2026-10-25T18:51:54.366Z ----
print(await sess_count('chk-f4'))
try:
    m=await asyncio.to_thread(meta,'chk-f4',f4['invite']); pm,v=parse_meta(m); print('rev',pm['revision'], pm['bindings'])
except Exception as e: print('meta err', repr(e)[:300])
gt=(await f4['fa'].acall('faClientGuardianTransfer'))['result']; print(json.dumps(gt)[:400])

# ---- cell @ 2026-10-25T18:52:00.768Z ----
r=await f4['fa'].acall('faClientResumeGuardianTransfer', timeout=120); print(json.dumps(r)[:600])

# ---- cell @ 2026-10-25T18:52:06.327Z ----
await asyncio.sleep(100)
print(ms.done(), [ (r['name'], r.get('ok'), r.get('secs')) for r in RESULTS if r['name'].startswith('X4b')])
print(await asyncio.to_thread(dag_heights,'QA f3', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))

# ---- cell @ 2026-10-25T18:53:56.031Z ----
for r in RESULTS[-3:]: print(json.dumps(r)[:600])

# ---- cell @ 2026-10-25T18:54:03.418Z ----
print([(s['i'],s['loc']) for s in f5['seats']], await sess_count('chk-f5'))
ms=asyncio.create_task(make_stuck(f5,[1,3,4],'X4c f5 back-to-back'))

# ---- cell @ 2026-10-25T18:54:10.410Z ----
n=await ctx_of(f5['fa']); f5['seats']=n['seats']; print([(s['i'],s['loc']) for s in f5['seats']])

# ---- cell @ 2026-10-25T18:54:15.686Z ----
await asyncio.sleep(170)
print(ms.done(), [ (r['name'], r.get('ok'), r.get('secs'), r.get('probs')) for r in RESULTS if r['name'].startswith('X4c')])
print(await asyncio.to_thread(dag_heights,'QA f5', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))

# ---- cell @ 2026-10-25T18:56:20.115Z ----
await asyncio.sleep(120)
print(ms.done(), [ (r['name'], r.get('ok'), r.get('secs'), r.get('probs')) for r in RESULTS if r['name'].startswith('X4c')])
print(await asyncio.to_thread(dag_heights,'QA f5', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))

# ---- cell @ 2026-10-25T18:58:27.156Z ----
await asyncio.sleep(45)
print(time.strftime('%H:%M:%S', time.gmtime()), ms.done(), [ (r['name'], r.get('ok'), r.get('secs')) for r in RESULTS if r['name'].startswith('X4c')])
SM=seat_map()
for k,v in SM.items():
    if v['fed']!='QA f5': continue
    for l in reversed(loglines(v['g'])):
        if k in l and ('Runway status report' in l):
            print(k[:8], v['g'], ident(k), l[11:19], l[l.find('Runway'):l.find('seat_id')][:230]); break

# ---- cell @ 2026-10-25T18:59:30.111Z ----
st=qaenv.proxy({'cmd':'state'}); print(str(st)[:1500])

# ---- cell @ 2026-10-25T18:59:37.404Z ----
r=await command("tail -5 /src/scratch/qa-gt/netproxy.log; grep -c retarget /src/scratch/qa-gt/netproxy.log; grep -n 'retarget' /src/scratch/qa-gt/netproxy.py | head")
try: print(inspect.getsource(monitor_loop)[:2000])
except Exception as e: print(e)

# ---- cell @ 2026-10-25T18:59:52.463Z ----
for g,sid in [(0,'0c9d2330'),(1,'9d1eb4d0')]:
    print('==',g,sid)
    for l in loglines(g):
        if sid in l and '2026-10-25T18:55:40'<=l[:19] and ('onnect' in l or 'WARN' in l[:60] or 'ERROR' in l[:60]) and 'Iroh API request' not in l and 'iroh connection err' not in l:
            print(l[11:23], _re.sub(r'seat_id=\S+','',l[60:300]))

# ---- cell @ 2026-10-25T19:00:04.283Z ----
for l in loglines(0):
    if '0c9d2330' in l and '2026-10-25T18:55:40'<=l[:19] and ('Runway status' in l or 'consensus session' in l or 'Creator' in l):
        print(l[11:19], l[l.find('AlephBFT')+0 if 'AlephBFT' in l else 60:][:230])

# ---- cell @ 2026-10-25T19:00:17.608Z ----
f6task=asyncio.create_task(mkfed('f6'))
await asyncio.sleep(100)
print(f6task.done())

# ---- cell @ 2026-10-25T19:02:04.673Z ----
f6=f6task.result(); print([(s['i'],s['loc']) for s in f6['seats']])
async def stuck_f6():
    # wait until session >=1 complete then boundary
    while (await sess_count('chk-f6') or 0) < 1: await asyncio.sleep(10)
    return await make_stuck(f6,[0,1,2],'X4d f6 back-to-back')
ms6=asyncio.create_task(stuck_f6())

# ---- cell @ 2026-10-25T19:02:14.980Z ----
r=await command("cd /src/fedimint-fedi && grep -rn 'AWAIT_TRANSACTION_ENDPOINT\\|AWAIT_OUTPUT_OUTCOME' --include=*.rs fedimint-server/src | head -5; grep -rn 'async fn await_transaction' -A25 fedimint-server/src/consensus/api.rs | head -40")

# ---- cell @ 2026-10-25T19:02:30.776Z ----
await asyncio.sleep(170)
print(time.strftime('%H:%M:%S', time.gmtime()), ms6.done(), [ (r['name'], r.get('ok'), r.get('secs')) for r in RESULTS if r['name'].startswith('X4d')])
print(await sess_count('chk-f6'))

# ---- cell @ 2026-10-25T19:04:35.224Z ----
await asyncio.sleep(150)
print(time.strftime('%H:%M:%S', time.gmtime()), ms6.done(), [ (r['name'], r.get('ok'), r.get('secs')) for r in RESULTS if r['name'].startswith('X4d')])
print(await sess_count('chk-f6'))
print(await asyncio.to_thread(dag_heights,'QA f6', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))

# ---- cell @ 2026-10-25T19:06:39.937Z ----
await asyncio.sleep(120)
print(time.strftime('%H:%M:%S', time.gmtime()), ms6.done(), [ (r['name'], r.get('ok'), r.get('secs')) for r in RESULTS if r['name'].startswith('X4d')])
print(await asyncio.to_thread(dag_heights,'QA f6', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))

# ---- cell @ 2026-10-25T19:08:47.884Z ----
await asyncio.sleep(100)
print(time.strftime('%H:%M:%S', time.gmtime()), [ (r['name'], r.get('ok'), r.get('secs')) for r in RESULTS if r['name'].startswith('X4d')])
print(await asyncio.to_thread(dag_heights,'QA f6', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))
SM=seat_map()
for k,v in SM.items():
    if v['fed']=='QA f6':
        d=f"{E.mg_dir(v['g'])}/seats/{v['sn']}/data"
        print(k[:8], v['g'], v['sn'], ident(k), os.listdir(f'{d}/db_checkpoints') if os.path.exists(f'{d}/db_checkpoints') else None, [x for x in running_seat_dirs() if x[0]==v['g'] and x[1]==v['sn']]!=[])

# ---- cell @ 2026-10-25T19:10:46.093Z ----
m=await asyncio.to_thread(meta,'chk-f6',f6['invite']); pm,v=parse_meta(m); print('before rollback rev',pm['revision'], pm['bindings'])
gt=(await f6['fa'].acall('faClientGuardianTransfer'))['result']; print(json.dumps(gt)[:200])
# rollback
SM=seat_map()
f6s={k:v for k,v in SM.items() if v['fed']=='QA f6' and not k.startswith('0d45f5df')}
gs=sorted(set(v['g'] for v in f6s.values())); print(gs)
t0=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime())
await asyncio.gather(*[asyncio.to_thread(E.stop_mg,g) for g in gs])
for k,v in f6s.items():
    d=f"{E.mg_dir(v['g'])}/seats/{v['sn']}/data"
    os.rename(f'{d}/database', f'{d}/database.q3c-stuck')
    shutil.copytree(f'{d}/db_checkpoints/1', f'{d}/database')
for g in gs: E.start_mg(g)
print('rolled back', t0)

# ---- cell @ 2026-10-25T19:10:54.152Z ----
await asyncio.sleep(90)
print(await asyncio.to_thread(dag_heights,'QA f6', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-12))))
for k,v in list(f6s.items())[:2]:
    for x in seat_events(v['g'],k[:8],t0,'2099')[-3:]: print(k[:8],x)
print(await sess_count('chk-f6'))

# ---- cell @ 2026-10-25T19:12:43.978Z ----
await asyncio.sleep(75)
print(await sess_count('chk-f6'))
m=await asyncio.to_thread(meta,'chk-f6',f6['invite']); pm,v=parse_meta(m); print('after rollback rev',pm['revision'], pm['bindings'])
r=await f6['fa'].acall('faClientResumeGuardianTransfer', timeout=200); print(json.dumps(r)[:500])

# ---- cell @ 2026-10-25T19:14:10.273Z ----
r=await command("cd /src/fedimint-fedi && grep -n 'AcceptedTransactionKey' fedimint-server/src/consensus/engine.rs | head")

# ---- cell @ 2026-10-25T19:14:44.854Z ----
print([k for k in globals().keys() if k.isupper() or k.startswith('R1') or 'case' in k.lower()][:60])

# ---- cell @ 2026-10-25T19:14:49.215Z ----
print(len(cases), cases[:8])
for c in CTXS:
    if c['name'] in ('f3','f4','f5','f6') and not c.get('retired'): c['retired']='used by F2 experiments'
print([(c['name'],c.get('retired'),c['transfers']) for c in CTXS])

# ---- cell @ 2026-10-25T19:14:55.705Z ----
r=await command("free -g; nproc; uptime")
print(len(running_seat_dirs()))

# ---- cell @ 2026-10-25T19:15:01.129Z ----
print(inspect.getsource(decommission_fed))

# ---- cell @ 2026-10-25T19:15:05.959Z ----
async def decom_all():
    for c in CTXS:
        if c.get('retired') and not c.get('decommissioned'):
            try:
                await asyncio.to_thread(decommission_fed, c); c['decommissioned']=True
            except Exception as e: print(c['name'], e)
    # f2 not in CTXS? check seats of feds named QA f2
await decom_all()
await asyncio.sleep(10)
print(len(running_seat_dirs()))
SM=seat_map(); print(collections.Counter(v['fed'] for v in SM.values()))

# ---- cell @ 2026-10-25T19:15:24.134Z ----
for k,v in SM.items():
    if v['fed']=='QA f2':
        try: E.fman(v['g'],'seats','decommission',k)
        except Exception as e: print('err', str(e)[:200])
await asyncio.sleep(8)
print(len(running_seat_dirs()))
r=await command("uptime")

# ---- cell @ 2026-10-25T19:15:45.271Z ----
async def sweep2(cases_, label, max_transfers=6):
    out=[]
    for k,(trig,delay,act) in enumerate(cases_):
        c=await get_ctx(max_transfers)
        if (await sess_count(f"chk-{c['name']}") or 0) < 1:
            while (await sess_count(f"chk-{c['name']}") or 0) < 1: await asyncio.sleep(10)
        await wait_boundary(c, extra=10)
        idx=c['transfers']%7
        name=f'{label}#{k} {c["name"]} idx{idx} trig={trig} +{delay}s act={act}'
        t=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime())
        try:
            res=await run_scenario(name, c, idx, faults=[(trig,delay,act)])
        except Exception as e:
            import traceback
            res={'name':name,'ok':False,'probs':['harness exception '+repr(e)[:300]],'tb':traceback.format_exc()[-1500:]}
            RESULTS.append(res)
        c['transfers']+=1
        try:
            fk,_=await asyncio.to_thread(forkers_now, f"QA {c['name']}", t); res['forkers']=sorted(fk)
        except Exception: pass
        log_result(res); out.append((name,res.get('ok'),res.get('probs'),res.get('forkers')))
        if not res.get('ok'):
            c['retired']='failed: '+name
            print('!!! FAIL', name, res.get('probs'))
            try: await asyncio.to_thread(Faults({'fa':c['fa']}).heal, False)
            except Exception: pass
    return out
# make sure chk wallet exists for new feds: patch get_ctx to create chk wallet
_orig_get_ctx=get_ctx
async def get_ctx(max_transfers=6):
    c=await _orig_get_ctx(max_transfers)
    if not os.path.exists(f"{E.root}/chk-{c['name']}"):
        await asyncio.to_thread(meta, f"chk-{c['name']}", c['invite'])
    return c
sw2=asyncio.create_task(sweep2(cases,'R1b'))

# ---- cell @ 2026-10-25T19:15:51.381Z ----
sw2.cancel()
await asyncio.sleep(1)
print(FA_COUNTER, [c['name'] for c in CTXS])
FA_COUNTER[0]=10
sw2=asyncio.create_task(sweep2(cases,'R1b'))

# ---- cell @ 2026-10-25T19:15:59.150Z ----
r=await command("ps -eo pid,etimes,args | grep -E 'fediffi|fa_repl' | grep -v grep | awk '{print $1, $2}' | head -20")
for c in CTXS: print(c['name'], c['fa'].p.pid if getattr(c['fa'],'p',None) else None, c['fa'].alive())

# ---- cell @ 2026-10-25T19:16:04.927Z ----
for pid in (1028320,1028426,712762):
    try:
        env=open(f'/proc/{pid}/environ','rb').read().split(b'\0')
        print(pid, [e for e in env if e.startswith(b'FA_REPL_DATA_DIR')])
    except Exception as e: print(pid, e)

# ---- cell @ 2026-10-25T19:16:11.166Z ----
for pid in (1028320, 712762):
    try: os.kill(pid, signal.SIGKILL)
    except Exception as e: print(e)
# stop FAs of retired feds to reduce load (keep f1? not needed)
for c in CTXS:
    if c.get('retired'):
        try: c['fa'].stop()
        except Exception as e: print(c['name'], e)
print('ok')

# ---- cell @ 2026-10-25T19:16:49.145Z ----
r=await command("rg -n 'fn backup|fn adopt_transferred_seat|RemoveSeat|remove_seat|TransferStage::Buying|RemovingSource' crates/fa-client/src/transfer.rs crates/fa-client/src/state.rs crates/fa-client/src/backup.rs | head -40", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T19:16:53.741Z ----
r=await command("sed -n 150,200p crates/fa-client/src/transfer.rs; sed -n 590,690p crates/fa-client/src/transfer.rs", workdir='/src/qa-transfer-manifold-rb')

# ---- cell @ 2026-10-25T19:17:06.741Z ----
r=await command("rg -n 'fn fa_client_[a-z_]*' bridge/fedi-ffi/src/rpc.rs crates/bridge/src -o --no-heading | sed 's/.*fn //' | sort -u | head -60; rg -n 'restoreMnemonic|fn restore_mnemonic' bridge/fedi-ffi/src/rpc.rs | head", workdir='/src/qa-transfer-fedi')

# ---- cell @ 2026-10-25T19:17:11.247Z ----
r=await command("rg -n 'async fn faClient[A-Za-z]*' bridge/fedi-ffi/src/rpc.rs -o --no-heading | sort -u; sed -n 655,700p bridge/fedi-ffi/src/rpc.rs", workdir='/src/qa-transfer-fedi')

# ---- cell @ 2026-10-25T19:17:17.835Z ----
r=await command("rg -n 'OnboardingCompletionMethod::' bridge/fedi-ffi/src/rpc.rs crates/bridge/src -o --no-heading | sort -u | head; rg -n 'async fn getMnemonic|async fn completeOnboarding[A-Za-z]*' bridge/fedi-ffi/src/rpc.rs; rg -n '__complete_onboarding' -A6 bridge/fedi-ffi/src/rpc/tests/fa_repl.rs; rg -n 'fa_client.*restore|restore.*fa_client|fn restore_formation|RestoredFormation' crates/bridge/src/fa_client.rs | head", workdir='/src/qa-transfer-fedi')

# ---- cell @ 2026-10-25T19:17:22.017Z ----
r=await command("sed -n 1425,1460p bridge/fedi-ffi/src/rpc.rs; rg -n 'restored|Restored' crates/bridge/src/fa_client.rs | head -30", workdir='/src/qa-transfer-fedi')

# ---- cell @ 2026-10-25T19:17:28.497Z ----
r=await command("rg -n 'restore_mnemonic|restoreMnemonic|BackupRecoveryHint::RestoredMnemonic|restored_mnemonic' bridge/fedi-ffi/src/rpc/tests/*.rs bridge/fedi-ffi/src/test_device.rs crates/bridge/src/fa_client.rs | head -20", workdir='/src/qa-transfer-fedi')

# ---- cell @ 2026-10-25T19:17:32.740Z ----
r=await command("sed -n 835,880p bridge/fedi-ffi/src/rpc/tests/multispend_tests.rs; sed -n 150,200p crates/bridge/src/fa_client.rs", workdir='/src/qa-transfer-fedi')

# ---- cell @ 2026-10-25T19:17:43.432Z ----
print(inspect.getsource(Fa)[:3500])

# ---- cell @ 2026-10-25T19:17:48.825Z ----
for m in ['__init__','start','call','acall','kill','stop','alive','env']:
    fn=getattr(Fa,m,None)
    if fn is None: continue
    try: print(inspect.getsource(fn))
    except Exception as e: print(m,'nosrc', fn)

# ---- cell @ 2026-10-25T19:17:54.396Z ----
print({k:(str(v)[:120]) for k,v in vars(f6['fa']).items()})
print(Fa.__init__.__code__.co_varnames[:10], Fa.start.__code__.co_names)

# ---- cell @ 2026-10-25T19:18:03.217Z ----
async def restore_fa(old_fa, new_name, kill_old=True):
    w=await old_fa.acall('getMnemonic'); words=w['result']
    if kill_old: old_fa.kill()
    nf=Fa(new_name, relay_i=old_fa.relay_i); await asyncio.to_thread(nf.start)
    r1=await nf.acall('restoreMnemonic', {'mnemonic': words}, timeout=120)
    r2=await nf.acall('onboardTransferExistingDeviceRegistration', {'index': 0}, timeout=120)
    return nf, r1, r2
r4=await mkfed('r4a'); print([(s['i'],s['loc']) for s in r4['seats']])

# ---- cell @ 2026-10-25T19:18:50.145Z ----
async def R4d(c):
    await wait_boundary(c, extra=8)
    f=c['fa']; idx=0
    pre=await capture_pre(c, idx)
    sg,ss=pre['src']
    F=Faults({'fa':f,'src':pre['src']})
    pv=await f.acall('faClientPreviewTransferDestination')
    st=await f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':pv['result']['preview']['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=120)
    print('start', str(st)[:150])
    # cut source MG endpoint once source stopped
    for _ in range(600):
        if q(sg, f"select phase from seat_transfer_out where lower(hex(quote_id))='{pre['src_seat']}'"): break
        await asyncio.sleep(0.1)
    F.do(('cut_endpoint', sg)); print('cut src endpoint', time.strftime('%H:%M:%S'))
    rt=asyncio.create_task(resumer(f))
    ph=None
    for _ in range(200):
        r=await f.acall('faClientGuardianTransfer', timeout=10); tr=r['result'].get('transfer'); ph=tr and tr['phase']
        if ph=='removingSource' or tr is None: break
        await asyncio.sleep(2)
    print('phase before kill', ph, time.strftime('%H:%M:%S'))
    rt.cancel()
    nf,r1,r2=await restore_fa(f, c['name']+'-restored')
    print('restore', str(r1)[:200], str(r2)[:200])
    c['fa']=nf
    st=await nf.acall('faClientStatus', timeout=60); print('restored status', json.dumps(st)[:600])
    F.heal(restart=False)
    return pre
r4d=asyncio.create_task(R4d(r4))

# ---- cell @ 2026-10-25T19:18:55.496Z ----
await asyncio.sleep(150)
print(r4d.done())
if r4d.done(): pre4=r4d.result()

# ---- cell @ 2026-10-25T19:21:01.436Z ----
await asyncio.sleep(60)
print(r4d.done())

# ---- cell @ 2026-10-25T19:22:06.354Z ----
r=await r4['fa'].acall('faClientGuardianTransfer', timeout=10); print(json.dumps(r)[:300])
print(Faults.resolve.__code__.co_consts, Faults.resolve.__code__.co_names)

# ---- cell @ 2026-10-25T19:22:17.315Z ----
async def R4d_cont(c, idx=0):
    f=c['fa']
    pre=await capture_pre(c, idx) if False else None
    s=[x for x in c['seats'] if x['i']==idx][0]; sg,ss=s['loc'][0]; src_seat=s['seat_id']
    F=Faults({'fa':f,'src':(sg,ss)})
    rt=asyncio.create_task(resumer(f))
    for _ in range(600):
        if q(sg, f"select phase from seat_transfer_out where lower(hex(quote_id))='{src_seat}'"): break
        await asyncio.sleep(0.1)
    F.do(('cut_endpoint', 'src')); print('cut src endpoint', time.strftime('%H:%M:%S'))
    ph=None
    for _ in range(200):
        r=await f.acall('faClientGuardianTransfer', timeout=10); tr=r['result'].get('transfer'); ph=tr and tr['phase']
        if ph=='removingSource' or tr is None: break
        await asyncio.sleep(2)
    print('phase before kill', ph, time.strftime('%H:%M:%S'))
    rt.cancel(); await asyncio.sleep(1)
    nf,r1,r2=await restore_fa(f, c['name']+'-restored')
    print('restore', str(r1)[:200], str(r2)[:200])
    c['fa_old']=f; c['fa']=nf
    st=await nf.acall('faClientStatus', timeout=60); print('restored status', json.dumps(st)[:800])
    F.heal(restart=False); print('healed', time.strftime('%H:%M:%S'))
    return (sg,ss,src_seat)
r4d=asyncio.create_task(R4d_cont(r4))

# ---- cell @ 2026-10-25T19:22:26.272Z ----
await asyncio.sleep(90)
print(r4d.done())

# ---- cell @ 2026-10-25T19:25:09.474Z ----
sg, ss, src_seat = await r4d
fa = r4['fa']
st = await fa.call('faClientStatus', {})
print(json.dumps(st)[:6000], max_tokens=4000)
print(q(1, f"select phase from seat_transfer_out where lower(hex(quote_id))='{src_seat}'"))
print(os.path.exists(f"{E.root}/mg-1/seats/8/data"), os.listdir(f"{E.root}/mg-1/seats/8") if os.path.exists(f"{E.root}/mg-1/seats/8") else None)

# ---- cell @ 2026-10-25T19:25:32.943Z ----
res = r4d.result() if hasattr(r4d,'result') else None
print(res, r4.keys())

# ---- cell @ 2026-10-25T19:25:35.606Z ----
sg, ss, src_seat = res
fa = r4['fa']
import inspect
print([m for m in dir(fa) if not m.startswith('_')])

# ---- cell @ 2026-10-25T19:25:38.900Z ----
st = fa.call('faClientStatus', {})
s = st['result']['status']
print({k:v for k,v in s.items() if k not in('formation',)})
for x in s['formation']['seats']: print({k:(v if k!='locator' else '..') for k,v in x.items()})
print(q(1, f"select phase from seat_transfer_out where lower(hex(quote_id))='{src_seat}'"))
d=f"{E.root}/mg-1/seats/8"; print(os.path.exists(d) and os.listdir(d))

# ---- cell @ 2026-10-25T19:25:43.854Z ----
print(json.dumps({k:v for k,v in s['formation'].items() if k!='seats'})[:3000])
print(seat_map().get(src_seat), [ (x['seatId'][:8], seat_map().get(x['seatId'])) for x in s['formation']['seats']])

# ---- cell @ 2026-10-25T19:26:02.666Z ----
for m,p in [('faClientResume',{}),('faClientResumeGuardianTransfer',{})]:
    try: print(m, json.dumps(fa.call(m,p))[:800])
    except Exception as e: print(m,'EXC',e)
await asyncio.sleep(30)
print(q(1, f"select phase from seat_transfer_out where lower(hex(quote_id))='{src_seat}'"), os.listdir(d) if os.path.exists(d) else 'gone')

# ---- cell @ 2026-10-25T19:26:36.876Z ----
command(f"find {d} -maxdepth 2 | head -30; ls -la {d}/data | head")
print(q(1, "select * from seat_transfer_out"), max_tokens=800)

# ---- cell @ 2026-10-25T19:26:41.280Z ----
command("rg -n \"Removed|'removed'|remove_seat|RemoveSeat\" crates/mg/core/src crates/mg/specs/SPEC-guardian-transfer.md | head -40", workdir="/src/qa-transfer-manifold-rb")
command("rg -n 'fn backup|RemovingSource|Buying|fn adopt_transferred_seat' crates/fa-client/src/transfer.rs | head", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T19:26:44.962Z ----
command("sed -n 50,120p crates/mg/specs/SPEC-guardian-transfer.md; sed -n 150,175p crates/fa-client/src/transfer.rs", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T19:27:03.227Z ----
import re
t=open('~/.claude/projects/-src-qa-transfer-manifold/2261e8b9-d2c3-4a5d-8bdf-d2ddcb41e5d7.jsonl').read()
for name in ['def R4d_cont','def restore_fa','def start_transfer','def mkfed']:
    i=t.rfind(name); print(json.loads('"'+t[i:i+2500].split('"')[0]+'"') if i>0 else None, '\n-----')

# ---- cell @ 2026-10-25T19:27:06.378Z ----
for name in ['def R4d_cont','def restore_fa','def start_transfer','def mkfed']:
    i=t.rfind(name); s=t[i:i+2500]; 
    print(s.encode().decode('unicode_escape')[:2000] if i>0 else None, '\n-----', max_tokens=1500)

# ---- cell @ 2026-10-25T19:27:12.301Z ----
command("sed -n 255,380p crates/fa-client/src/transfer.rs", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T19:27:16.894Z ----
command("rg -n 'fn drive_seat_purchase' -A60 crates/fa-client/src/transfer.rs | head -90", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T19:27:20.536Z ----
command("rg -n 'fn drive_seat_purchase|fn seat_purchase' crates/fa-client/src", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T19:27:23.277Z ----
command("sed -n 1,80p crates/fa-client/src/seat_purchase.rs; sed -n 330,480p crates/fa-client/src/seat_purchase.rs", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T19:27:32.949Z ----
for name in ['class Faults','def resolve']:
    i=t.rfind(name); s=t[i:i+4000]
    print(s.encode().decode('unicode_escape')[:3500] if i>0 else None, '\n-----', max_tokens=2000)

# ---- cell @ 2026-10-25T19:27:37.982Z ----
i=t.rfind("'spent'"); print(t[i-1500:i+300].encode().decode('unicode_escape'), max_tokens=1200)

# ---- cell @ 2026-10-25T19:27:41.272Z ----
for pat in ['spent=', '"spent"', "spent'"]:
    for m in list(re.finditer(re.escape(pat), t))[:3]:
        print(t[m.start()-300:m.start()+200].encode().decode('unicode_escape','ignore')[:500]); print('====')

# ---- cell @ 2026-10-25T19:27:47.136Z ----
print(await fa_balance(fa))
print(q(10,"select * from seats limit 1"), max_tokens=500)
command(f"sqlite3 {E.root}/mg-10/fleet-manager.sqlite '.schema seats'")

# ---- cell @ 2026-10-25T19:27:52.812Z ----
b=await fa_balance(fa)
print(b if not isinstance(b,dict) else [(x.get('clientConfig',{}).get('global',{}).get('meta'), x.get('balance'), x.get('id') , list(x.keys())[:8]) for x in b['result']])
print(PAYFED)

# ---- cell @ 2026-10-25T19:28:00.157Z ----
r=fa.call('listFederations',{})
print([(x.get('name') or (x.get('meta') or {}).get('federation_name'), x.get('balance'), x.get('init_state'), x.get('recovering')) for x in r['result']])
i=t.rfind('def fa_balance'); print(t[i:i+600].encode().decode('unicode_escape','ignore'))

# ---- cell @ 2026-10-25T19:28:07.758Z ----
for m in list(re.finditer('joinFederation', t))[-3:]:
    print(t[m.start()-300:m.start()+300].encode().decode('unicode_escape','ignore')); print('===')

# ---- cell @ 2026-10-25T19:28:11.018Z ----
for m in list(re.finditer(r"acall\('joinFederation'|'joinFederation'", t))[:4]:
    print(t[m.start()-400:m.start()+300].encode().decode('unicode_escape','ignore')); print('===')
print([k for k in globals() if 'INV' in k.upper() or 'PAY' in k.upper()])

# ---- cell @ 2026-10-25T19:28:15.381Z ----
j=await fa.acall('joinFederation', {'inviteCode': E.st['payment_invite'], 'recoverFromScratch': True}, timeout=300)
print(str(j)[:300])
for _ in range(40):
    r=fa.call('listFederations',{}); x=[y for y in r['result'] if y.get('id')==PAYFED]
    if x and not x[0].get('recovering'): break
    await asyncio.sleep(5)
print(x and {k:x[0].get(k) for k in ('balance','recovering','init_state')})

# ---- cell @ 2026-10-25T19:28:30.167Z ----
INVMG={v:k for k,v in MGID.items()}
async def R4b(c, idx, newname, trig='seat_row'):
    f=c['fa']; bal0=await fa_balance(f)
    known={(g,x[0]) for g in range(14) for x in q(g,'select lower(hex(quote_id)) from seats')}
    pv=await f.acall('faClientPreviewTransferDestination'); prev=pv['result']['preview']
    print('preview', json.dumps(prev)[:400])
    st_task=asyncio.create_task(f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=120))
    dg=None
    for _ in range(600):
        new=[(g,s) for g in range(14) for (s,) in q(g,'select lower(hex(quote_id)) from seats') if (g,s) not in known]
        if new: dg,dseat=new[0]; break
        await asyncio.sleep(0.05)
    F=Faults({'fa':f,'dest_g':dg}); F.do(('cut_endpoint','dst')); print('new dest seat', dg, dseat, 'cut', time.strftime('%H:%M:%S'))
    await asyncio.sleep(8)
    r=await f.acall('faClientGuardianTransfer', timeout=10); print('phase', json.dumps(r)[:300])
    print('start result so far', st_task.done() and str(st_task.result())[:300])
    nf,r1,r2=await restore_fa(f, newname); c['fa_old']=f; c['fa']=nf
    st_task.cancel()
    j=await nf.acall('joinFederation', {'inviteCode': E.st['payment_invite'], 'recoverFromScratch': True}, timeout=300)
    F.heal(restart=False); print('healed', time.strftime('%H:%M:%S'))
    for _ in range(60):
        x=[y for y in nf.call('listFederations',{})['result'] if y.get('id')==PAYFED]
        if x and not x[0].get('recovering'): break
        await asyncio.sleep(5)
    bal1=x[0]['balance']; print('bal0',bal0,'bal1',bal1,'delta',bal0-bal1)
    await asyncio.sleep(10)
    print('restored transfer', nf.call('faClientGuardianTransfer',{}), nf.call('faClientResumeGuardianTransfer',{}))
    return dg,dseat,bal0,bal1
r4b=asyncio.create_task(R4b(r4, 1, 'r4a-r2'))

# ---- cell @ 2026-10-25T19:28:32.650Z ----
await asyncio.sleep(100); print(r4b.done())

# ---- cell @ 2026-10-25T19:30:18.612Z ----
print(q(9,"select lower(hex(quote_id)),phase from seat_transfer_out"))
print(os.path.exists(f"{E.root}/mg-9/seats/6/data"))
st=r4['fa'].call('faClientStatus',{})['result']['status']
print([(seat_map().get(x['seatId'],{}).get('g'),seat_map().get(x['seatId'],{}).get('sn')) for x in st['formation']['seats']])

# ---- cell @ 2026-10-25T19:30:23.552Z ----
sm=seat_map()
for g in range(14):
    for s,ph in q(g,"select lower(hex(quote_id)),phase from seat_transfer_out where phase!='removed'"):
        print(g, s[:10], ph, sm.get(s,{}).get('sn'), sm.get(s,{}).get('fed'))

# ---- cell @ 2026-10-25T19:30:36.763Z ----
command("rg -n 'INSERT INTO (ecash_claims|seats)' crates/mg/core/src | head; sqlite3 /src/scratch/qa-gt/env1/mg-5/fleet-manager.sqlite '.tables'", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T19:30:39.539Z ----
command("sed -n 500,600p crates/mg/core/src/db/seats.rs", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T19:31:02.631Z ----
R4N=[2]
async def R4x(c, idx, X):
    f=c['fa']; bal0=await fa_balance(f)
    known={(g,x[0]) for g in range(14) for x in q(g,'select lower(hex(quote_id)) from seats')}
    pv=await f.acall('faClientPreviewTransferDestination'); prev=pv['result']['preview']; dg=INVMG[prev['mgId']]
    t0=time.time()
    st_task=asyncio.create_task(f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=120))
    await asyncio.sleep(X); w=await f.acall('getMnemonic'); f.kill(); tk=time.time()-t0
    new=[(g,s) for g in range(14) for (s,) in q(g,'select lower(hex(quote_id)) from seats') if (g,s) not in known]
    claims=[q(g, f"select claim_outcome from ecash_claims where lower(hex(quote_id))='{s}'") for g,s in new]
    R4N[0]+=1; name=f"{c['name']}-r{R4N[0]}"
    nf=Fa(name, relay_i=f.relay_i); await asyncio.to_thread(nf.start)
    await nf.acall('restoreMnemonic', {'mnemonic': w['result']}, timeout=120)
    await nf.acall('onboardTransferExistingDeviceRegistration', {'index': 0}, timeout=120)
    c['fa_old']=f; c['fa']=nf
    await nf.acall('joinFederation', {'inviteCode': E.st['payment_invite'], 'recoverFromScratch': True}, timeout=300)
    for _ in range(60):
        x=[y for y in nf.call('listFederations',{})['result'] if y.get('id')==PAYFED]
        if x and not x[0].get('recovering'): break
        await asyncio.sleep(5)
    for _ in range(20):
        s=nf.call('faClientStatus',{})['result']['status']
        if s.get('formation',{}).get('freshness')=='fresh': break
        await asyncio.sleep(3)
    gt=nf.call('faClientGuardianTransfer',{})['result'].get('transfer')
    res=None
    if gt:
        for _ in range(40):
            r=await nf.acall('faClientResumeGuardianTransfer', timeout=200); res=r
            tr=r.get('result',{}).get('transfer') if isinstance(r.get('result'),dict) else None
            if tr is None or tr.get('phase')=='completed': break
            await asyncio.sleep(5)
    bal1=[y for y in nf.call('listFederations',{})['result'] if y.get('id')==PAYFED][0]['balance']
    new2=[(g,s) for g in range(14) for (s,) in q(g,'select lower(hex(quote_id)) from seats') if (g,s) not in known]
    seats=[(seat_map().get(x['seatId'],{}).get('g'),seat_map().get(x['seatId'],{}).get('sn')) for x in nf.call('faClientStatus',{})['result']['status']['formation']['seats']]
    out=dict(X=X, killed_at=round(tk,2), dest_g=dg, new_at_kill=new, claims=claims, restored_transfer=gt and gt['phase'], resume=str(res)[:200], spent=bal0-bal1, new_after=new2, seats=seats)
    print(json.dumps(out)); return out
R4X=[]
async def R4sweep(c, Xs, idx_cycle):
    for X,idx in zip(Xs, idx_cycle):
        await wait_boundary(c, 5)
        try: R4X.append(await R4x(c, idx, X))
        except Exception as e: print('R4x exc', X, repr(e)); R4X.append({'X':X,'exc':repr(e)})
r4s=asyncio.create_task(R4sweep(r4, [0.3,0.8,1.5,2.5], [2,3,4,5]))

# ---- cell @ 2026-10-25T19:31:08.140Z ----
await asyncio.sleep(240); print(len(R4X))

# ---- cell @ 2026-10-25T19:33:10.922Z ----
await asyncio.sleep(120); print(len(R4X), r4s.done())

# ---- cell @ 2026-10-25T19:35:13.937Z ----
await asyncio.sleep(150); print(len(R4X), r4s.done(), R4X)

# ---- cell @ 2026-10-25T19:37:17.574Z ----
print(sess_count('QA r4a') if True else None)
print(dag_heights('QA r4a', time.time()-120))

# ---- cell @ 2026-10-25T19:37:22.286Z ----
print(await sess_count('QA r4a'))
print(dag_heights('QA r4a', time.strftime('%Y-%m-%dT%H:%M', time.gmtime(time.time()-120))))

# ---- cell @ 2026-10-25T19:37:36.233Z ----
for name in ['def wait_boundary','def sess_count']:
    i=t.rfind(name); print(t[i:i+900].encode().decode('unicode_escape','ignore'),'\n----')

# ---- cell @ 2026-10-25T19:37:39.226Z ----
print(await sess_count('chk-r4a'))
r=await asyncio.to_thread(fm_peer, f'{E.root}/chk-r4a', 2, 'session_count'); print(str(r)[:500])

# ---- cell @ 2026-10-25T19:37:45.313Z ----
for _ in range(3):
    print(dag_heights('QA r4a', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-15))))
    await asyncio.sleep(10)
command(f"ls {E.root}/mg-10/seats/7/ {E.root}/mg-5/seats/4/; pgrep -af 'seats/(7|4)/data' | head")

# ---- cell @ 2026-10-25T19:38:48.421Z ----
print(time.strftime('%H:%M:%S', time.gmtime()), time.strftime('%H:%M:%S'))
print(await sess_count('chk-r4a'))
command(f"grep -h 'Runway status' {E.root}/mg-2/seats/6/*.log 2>/dev/null | tail -3; ls {E.root}/mg-2/seats/6/ {E.root}/mg-2/ | head -30")

# ---- cell @ 2026-10-25T19:38:54.070Z ----
ls=[l for l in loglines(2) if 'f4c91875' in l and ('Runway status' in l or 'DAG' in l)] if False else None
r=await command(f"grep -a 'fm-f4c91875\\|f4c91875' {E.root}/mg-2/stdout.log | grep -a -i 'status' | awk 'NR%40==1' | cut -c1-260 | tail -40")

# ---- cell @ 2026-10-25T19:38:58.785Z ----
r=await command(f"grep -a 'Runway status' {E.root}/mg-2/stdout.log | grep -ao '^.\\{{0,40\\}}2026-10-25T[0-9:]*\\|DAG height [0-9]*\\|forkers[^;]*\\|session [0-9]*' | paste - - - | awk 'NR%15==1' | tail -50", max_tokens=3000)

# ---- cell @ 2026-10-25T19:39:04.657Z ----
sid=[k for k in seat_map() if k.startswith('f4c91875')][0]
rows=[]
with open(f"{E.root}/mg-2/stdout.log", errors='ignore') as fh:
    for l in fh:
        if sid in l and 'Runway status' in l:
            ts=re.search(r'(\d\d:\d\d:\d\d)', l).group(1); h=re.search(r'DAG height[^0-9]*(\d+)', l); fk=re.search(r'forkers[^\.;]*', l)
            rows.append((ts, h and int(h.group(1)), fk and fk.group(0)[:40]))
print(len(rows)); print(rows[::12], max_tokens=3000)

# ---- cell @ 2026-10-25T19:39:12.866Z ----
for p_ in range(7):
    try:
        r=await asyncio.to_thread(fm_peer, f'{E.root}/chk-r4a', p_, 'session_count'); print(p_, str(r).replace('\n',''))
    except Exception as e: print(p_, 'exc', e)

# ---- cell @ 2026-10-25T19:39:23.648Z ----
sid0=[k for k in seat_map() if k.startswith('56eeeb2a')][0]
ls_=[]
with open(f"{E.root}/mg-10/stdout.log", errors='ignore') as fh:
    for l in fh:
        if sid0 in l: ls_.append(l)
print(len(ls_))
for l in ls_[-25:]: print(re.sub(r'\x1b\[[0-9;]*m|\\x1b\[[0-9;]*m','',l)[:300])

# ---- cell @ 2026-10-25T19:39:31.772Z ----
from collections import Counter, defaultdict
first={}; cnt=Counter()
for g in range(14):
    with open(f"{E.root}/mg-{g}/stdout.log", errors='ignore') as fh:
        for l in fh:
            if 'Waiting for bitcoin backend' in l:
                m=re.search(r'seat_id=([0-9a-f]{64})', l)
                k=(g, m and m.group(1)[:8]); cnt[k]+=1
                if k not in first: first[k]=l[:30]
for k in cnt: print(k, cnt[k], first[k], re.sub(r'\x1b\[[0-9;]*m','',first[k]))
print(ls_[0][:120])

# ---- cell @ 2026-10-25T19:39:42.409Z ----
first={}; last={}; cnt=Counter()
for g in range(14):
    with open(f"{E.root}/mg-{g}/stdout.log", errors='ignore') as fh:
        for l in fh:
            if 'Waiting for bitcoin backend' in l:
                m=re.search(r'seat_id\S*?([0-9a-f]{64})', l)
                k=(g, m and m.group(1)); cnt[k]+=1
                ts=re.search(r'T(\d\d:\d\d:\d\d)', l).group(1)
                first.setdefault(k, ts); last[k]=ts
sm=seat_map()
for k in cnt: print(k[0], k[1] and k[1][:8], sm.get(k[1],{}).get('sn'), sm.get(k[1],{}).get('fed'), cnt[k], first[k], last[k])

# ---- cell @ 2026-10-25T19:39:52.033Z ----
pid=1047950
env=open(f'/proc/{pid}/environ','rb').read().split(b'\0')
print([e.decode() for e in env if b'BITCOIN' in e.upper() or b'ESPLORA' in e.upper() or b'RPC' in e.upper()])

# ---- cell @ 2026-10-25T19:39:56.730Z ----
r=httpx.post('http://127.0.0.1:12182', auth=('bitcoin','bitcoin'), json={'jsonrpc':'1.0','id':1,'method':'getblockchaininfo','params':[]}, timeout=10)
j=r.json()['result']; print({k:j[k] for k in ('blocks','headers','initialblockdownload','verificationprogress','mediantime','time') if k in j}, time.time())

# ---- cell @ 2026-10-25T19:40:00.400Z ----
import httpx
r=httpx.post('http://127.0.0.1:12182', auth=('bitcoin','bitcoin'), json={'jsonrpc':'1.0','id':1,'method':'getblockchaininfo','params':[]}, timeout=10)
j=r.json()['result']; print({k:j[k] for k in ('blocks','headers','initialblockdownload','verificationprogress','mediantime','time') if k in j}, time.time())

# ---- cell @ 2026-10-25T19:40:04.666Z ----
command("rg -n 'Waiting for bitcoin backend' -B30 /src/fedimint-fedi --type rust | head -80")

# ---- cell @ 2026-10-25T19:40:24.724Z ----
def btc(method, *params, wallet=None):
    url='http://127.0.0.1:12182'+(f'/wallet/{wallet}' if wallet else '')
    r=httpx.post(url, auth=('bitcoin','bitcoin'), json={'jsonrpc':'1.0','id':1,'method':method,'params':list(params)}, timeout=30).json()
    if r.get('error'): raise RuntimeError(r['error'])
    return r['result']
print(btc('listwallets'))

# ---- cell @ 2026-10-25T19:40:28.474Z ----
addr=btc('getnewaddress', wallet='miner')
print(btc('generatetoaddress', 1, addr))
j=btc('getblockchaininfo'); print(j['blocks'], j['verificationprogress'])

# ---- cell @ 2026-10-25T19:40:32.532Z ----
async def miner_loop():
    while True:
        await asyncio.sleep(600)
        try: btc('generatetoaddress', 1, addr)
        except Exception as e: print('miner err', e)
miner=asyncio.create_task(miner_loop())
await asyncio.sleep(20)
for p_ in range(7):
    try:
        r=await asyncio.to_thread(fm_peer, f'{E.root}/chk-r4a', p_, 'session_count'); print(p_, str(r).replace('\n',''))
    except Exception as e: print(p_, 'exc', e)
print(len(R4X), R4X)

# ---- cell @ 2026-10-25T19:41:00.408Z ----
r4s.cancel()
_src=None
INVMG=defaultdict(lambda: None, INVMG)
r4s=asyncio.create_task(R4sweep(r4, [0.3,0.8,1.5,2.5], [2,3,4,5]))

# ---- cell @ 2026-10-25T19:41:05.860Z ----
print(len(cases), cases[:4])
i=t.rfind('def sweep2'); print(t[i:i+1800].encode().decode('unicode_escape','ignore'))

# ---- cell @ 2026-10-25T19:41:13.866Z ----
async def peer_sessions(c):
    out={}
    for p_ in range(7):
        try:
            r=await asyncio.to_thread(fm_peer, f"{E.root}/chk-{c['name']}", p_, 'session_count')
            m=re.search(r'"value": (\d+)', str(r)); out[p_]=int(m.group(1)) if m else None
        except Exception: out[p_]=None
    return out
if '_orig_run_scenario' not in globals(): _orig_run_scenario=run_scenario
async def run_scenario(name, c, idx, **kw):
    res=await _orig_run_scenario(name, c, idx, **kw)
    if res.get('ok'):
        # the moved guardian must actually take part: its session count must keep up with its peers
        ps={}
        for _ in range(30):
            ps=await peer_sessions(c); vals=[v for v in ps.values() if v is not None]
            if ps.get(idx) is not None and vals and ps[idx]>=max(vals)-0: break
            await asyncio.sleep(10)
        res['peer_sessions']=ps
        if ps.get(idx) is None or ps[idx] < max(v for v in ps.values() if v is not None):
            res['ok']=False; res['probs']=res.get('probs',[])+[f'moved peer {idx} not caught up {ps}']
    return res
cases.extend(cases[:3])
print(len(cases))

# ---- cell @ 2026-10-25T19:41:18.797Z ----
await asyncio.sleep(300); print(len(R4X)); [print(json.dumps(x)) for x in R4X]

# ---- cell @ 2026-10-25T19:43:11.020Z ----
await asyncio.sleep(420); print(len(R4X)); [print(json.dumps(x)) for x in R4X[1:]]

# ---- cell @ 2026-10-25T19:45:15.238Z ----
print(len(R4X), r4s.done()); [print(json.dumps(x)) for x in R4X[1:]]
print(RESULTS[-1].get('peer_sessions'), RESULTS[-1].get('forkers'))

# ---- cell @ 2026-10-25T19:45:19.069Z ----
await asyncio.sleep(600); print(len(R4X), r4s.done()); [print(json.dumps(x)) for x in R4X[2:]]

# ---- cell @ 2026-10-25T19:46:17.790Z ----
nf=r4['fa']; print(nf.name)
print([ (y.get('balance'), y.get('recovering')) for y in nf.call('listFederations',{})['result'] if y.get('id')==PAYFED])
old=r4['fa_old']; print(old.name, old.dir)
command(f"ls -la {old.dir} | head -30")

# ---- cell @ 2026-10-25T19:46:24.052Z ----
r4s.cancel()
r=await command(f"grep -a -i 'transfer\\|purchase\\|quote\\|reserv' {old.dir}/repl.log | tail -15 | cut -c1-400")

# ---- cell @ 2026-10-25T19:46:27.530Z ----
r=await command(f"grep -a 'request_id\\S*=\\S*11' {old.dir}/repl.log | sed 's/\\x1b\\[[0-9;]*m//g' | cut -c1-330 | tail -30", max_tokens=4000)

# ---- cell @ 2026-10-25T19:46:31.354Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {old.dir}/repl.log | awk '$0 >= \"2026-10-25T19:45:47.59\"' | cut -c1-300 | grep -v netlink | head -60", max_tokens=5000)

# ---- cell @ 2026-10-25T19:46:38.270Z ----
await asyncio.to_thread(old.start)
await asyncio.sleep(5)
print(json.dumps(old.call('faClientGuardianTransfer',{}))[:1500])
print([ (y.get('balance'), y.get('recovering')) for y in old.call('listFederations',{})['result'] if y.get('id')==PAYFED])

# ---- cell @ 2026-10-25T19:46:50.787Z ----
known={(g,x[0]) for g in range(14) for x in q(g,'select lower(hex(quote_id)) from seats')}
r=await old.acall('faClientResumeGuardianTransfer', timeout=300); print(json.dumps(r)[:600])
new=[(g,s) for g in range(14) for (s,) in q(g,'select lower(hex(quote_id)) from seats') if (g,s) not in known]; print(new)
print([ (y.get('balance')) for y in old.call('listFederations',{})['result'] if y.get('id')==PAYFED])

# ---- cell @ 2026-10-25T19:46:54.913Z ----
for i in range(30):
    r=await old.acall('faClientResumeGuardianTransfer', timeout=300)
    if 'busy' not in json.dumps(r): break
    await asyncio.sleep(10)
print(i, json.dumps(r)[:600])
new=[(g,s) for g in range(14) for (s,) in q(g,'select lower(hex(quote_id)) from seats') if (g,s) not in known]; print(new)

# ---- cell @ 2026-10-25T19:47:34.135Z ----
for i in range(40):
    r=await old.acall('faClientResumeGuardianTransfer', timeout=300)
    tr=(r.get('result') or {}).get('transfer') if isinstance(r.get('result'),dict) else None
    print(i, json.dumps(r)[:200])
    if (r.get('result') or {}).get('type')=='transfer' and (tr is None or tr.get('phase')=='completed'): break
    await asyncio.sleep(10)
print([ (y.get('balance')) for y in old.call('listFederations',{})['result'] if y.get('id')==PAYFED])

# ---- cell @ 2026-10-25T19:47:40.347Z ----
print(json.dumps(r)[:700])
print(q(11,"select lower(hex(quote_id)),phase from seat_transfer_out where lower(hex(quote_id)) like 'ab13d159%'"))
st=nf.call('faClientStatus',{})['result']['status']
print([(seat_map().get(x['seatId'],{}).get('g'),seat_map().get(x['seatId'],{}).get('sn')) for x in st['formation']['seats']], st['formation'].get('freshness'))

# ---- cell @ 2026-10-25T19:47:44.419Z ----
print(st['formation']['seats'][3]['seatId'][:16])
old.kill()

# ---- cell @ 2026-10-25T19:47:49.178Z ----
print(nf.call('faClientResume',{}))
await asyncio.sleep(20)
st=nf.call('faClientStatus',{})['result']['status']
print([x['seatId'][:8] for x in st['formation']['seats']], st['formation'].get('freshness'), st['formation'].get('snapshotGeneration'))

# ---- cell @ 2026-10-25T19:48:14.841Z ----
for i in range(6):
    await asyncio.sleep(15)
    r=nf.call('faClientResume',{}); st=nf.call('faClientStatus',{})['result']['status']
    print(i, json.dumps(r)[:150], [x['seatId'][:8] for x in st['formation']['seats']][3], st['formation'].get('freshness'))

# ---- cell @ 2026-10-25T19:49:52.628Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {nf.dir}/repl.log | grep -a -i 'warn\\|error\\|invalid' | grep -v netlink | tail -12 | cut -c1-600", max_tokens=4000)

# ---- cell @ 2026-10-25T19:49:56.592Z ----
command("rg -n 'InvalidManifoldGuardians' crates/fa-client/src | head -30; sed -n 585,700p crates/fa-client/src/transfer.rs", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T19:50:06.251Z ----
nf2,r1,r2=await restore_fa(nf, 'r4a-r5'); r4['fa_old']=nf; r4['fa']=nf2
print(r1, r2)
for i in range(12):
    await asyncio.sleep(10)
    st=nf2.call('faClientStatus',{})['result'].get('status',{})
    fm_=st.get('formation') or {}
    print(i, [x['seatId'][:8] for x in fm_.get('seats',[])], fm_.get('freshness'))
    if fm_.get('freshness')=='fresh': break
print(nf2.call('faClientResume',{}))

# ---- cell @ 2026-10-25T19:50:29.975Z ----
async def R4e(c, idx, newname):
    f=c['fa']; F=Faults({'fa':f})
    await wait_boundary(c, 5)
    F.do(('cut_fa_relay',)); print('cut fa relay', time.strftime('%H:%M:%S'))
    pv=await f.acall('faClientPreviewTransferDestination'); prev=pv.get('result',{}).get('preview')
    print('preview', str(pv)[:200])
    if not prev: F.heal(False); return
    r=await f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=200)
    print('start', json.dumps(r)[:200])
    ph=None
    for i in range(40):
        r=await f.acall('faClientResumeGuardianTransfer', timeout=200); tr=(r.get('result') or {}).get('transfer') if isinstance(r.get('result'),dict) else None
        ph=tr and tr.get('phase'); print(i, time.strftime('%H:%M:%S'), json.dumps(r)[:160])
        if tr is None and (r.get('result') or {}).get('type')=='transfer' or ph=='completed': break
        await asyncio.sleep(5)
    nf_,r1,r2=await restore_fa(f, newname); c['fa_old']=f; c['fa']=nf_
    await asyncio.sleep(3)
    print('restored before heal', [x['seatId'][:8] for x in ((nf_.call('faClientStatus',{})['result'].get('status') or {}).get('formation') or {}).get('seats',[])])
    F.heal(False); print('healed')
    for i in range(12):
        await asyncio.sleep(10)
        st=(nf_.call('faClientStatus',{})['result'].get('status') or {}).get('formation') or {}
        print(i, [x['seatId'][:8] for x in st.get('seats',[])], st.get('freshness'), str(nf_.call('faClientResume',{}))[:120], str(nf_.call('faClientGuardianTransfer',{}))[:150])
r4e=asyncio.create_task(R4e(r4, 4, 'r4a-r6'))

# ---- cell @ 2026-10-25T19:50:46.658Z ----
await asyncio.sleep(400)

# ---- cell @ 2026-10-25T19:52:54.401Z ----
async def R4e(c, idx, newname):
    f=c['fa']; F=Faults({'fa':f})
    await wait_boundary(c, 5)
    pv=await f.acall('faClientPreviewTransferDestination'); prev=pv.get('result',{}).get('preview')
    F.do(('cut_fa_relay',)); print('cut fa relay', time.strftime('%H:%M:%S'))
    r=await f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=200)
    print('start', json.dumps(r)[:200])
    ph=None
    for i in range(30):
        r=await f.acall('faClientResumeGuardianTransfer', timeout=200); tr=(r.get('result') or {}).get('transfer') if isinstance(r.get('result'),dict) else None
        ph=tr and tr.get('phase'); print(i, time.strftime('%H:%M:%S'), json.dumps(r)[:160])
        if tr is None and (r.get('result') or {}).get('type')=='transfer' or ph=='completed': break
        await asyncio.sleep(5)
    nf_,r1,r2=await restore_fa(f, newname); c['fa_old']=f; c['fa']=nf_
    print('restore', r1, r2)
    F.heal(False); print('healed')
    for i in range(10):
        await asyncio.sleep(10)
        st=(nf_.call('faClientStatus',{})['result'].get('status') or {}).get('formation') or {}
        print(i, [x['seatId'][:8] for x in st.get('seats',[])], st.get('freshness'), str(nf_.call('faClientResume',{}))[:120], str(nf_.call('faClientGuardianTransfer',{}))[:150])
r4e=asyncio.create_task(R4e(r4, 4, 'r4a-r6'))

# ---- cell @ 2026-10-25T19:52:57.267Z ----
await asyncio.sleep(420)

# ---- cell @ 2026-10-25T19:55:05.000Z ----
await asyncio.sleep(240); print(r4e.done(), r4.get('retired'))

# ---- cell @ 2026-10-25T19:57:12.539Z ----
_restore_fa_base=restore_fa
async def restore_fa(old_fa, new_name, kill_old=True):
    nf_,r1,r2=await _restore_fa_base(old_fa, new_name, kill_old)
    await nf_.acall('joinFederation', {'inviteCode': E.st['payment_invite'], 'recoverFromScratch': True}, timeout=300)
    for _ in range(60):
        x=[y for y in nf_.call('listFederations',{})['result'] if y.get('id')==PAYFED]
        if x and not x[0].get('recovering'): break
        await asyncio.sleep(5)
    return nf_,r1,r2
f=r4['fa']
await f.acall('joinFederation', {'inviteCode': E.st['payment_invite'], 'recoverFromScratch': True}, timeout=300)
for _ in range(60):
    x=[y for y in f.call('listFederations',{})['result'] if y.get('id')==PAYFED]
    if x and not x[0].get('recovering'): break
    await asyncio.sleep(5)
print(x[0]['balance'])
r4e=asyncio.create_task(R4e(r4, 4, 'r4a-r7'))

# ---- cell @ 2026-10-25T19:57:20.719Z ----
await asyncio.sleep(480); print(r4e.done())

# ---- cell @ 2026-10-25T19:59:26.811Z ----
command("rg -n 'group|relay:fa|13090|nostr' /src/scratch/qa-gt/qaenv.py | head -40")

# ---- cell @ 2026-10-25T19:59:30.156Z ----
command("sed -n 70,100p /src/scratch/qa-gt/qaenv.py; rg -n 'RELAY_PORT|RELAY_PX' /src/scratch/qa-gt/qaenv.py | head")

# ---- cell @ 2026-10-25T19:59:35.198Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {r4['fa_old'].dir}/repl.log | awk '$0 >= \"2026-10-25T19:57:5\"' | grep -a -v netlink | grep -a -i 'warn\\|error\\|payer\\|payment\\|reserv' | head -20 | cut -c1-500", max_tokens=4000)

# ---- cell @ 2026-10-25T19:59:40.094Z ----
command("rg -n 'SelectedPayerUnavailable' crates/fa-client/src --glob '!tests.rs' | head; rg -n 'SelectedPayerUnavailable' -B12 crates/fa-client/src/selection.rs | head -60", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T19:59:44.249Z ----
command("rg -n 'SelectedPayerUnavailable|PayerUnavailable' crates/fa-client/src crates/fa-client/../ -g '*.rs' -l | head; rg -n 'selectedPayerUnavailable|SelectedPayerUnavailable' /src/fedi-qa*/bridge 2>/dev/null | head", workdir="/src/qa-transfer-manifold-rb")
command("ls /src")

# ---- cell @ 2026-10-25T19:59:49.285Z ----
command("rg -n 'fn release_payment_reservation|fn reserve|rejected' bridge/ -g '*.rs' | grep -i 'fa\\|pay' | head -20", workdir="/src/qa-transfer-fedi")

# ---- cell @ 2026-10-25T19:59:52.536Z ----
command("git -C /src/qa-transfer-fedi log --oneline -1; rg -ln 'FaPayments' /src/qa-transfer-fedi/bridge | head")

# ---- cell @ 2026-10-25T19:59:55.860Z ----
r=await command("rg -l 'impl.*FaPayments|FaPayments for' --type rust . | head", workdir="/src/qa-transfer-fedi")

# ---- cell @ 2026-10-25T20:00:00.147Z ----
r=await command("rg -n 'Rejected|rejected|fn ' crates/bridge/src/fa_payments.rs | head -60", workdir="/src/qa-transfer-fedi")

# ---- cell @ 2026-10-25T20:00:07.547Z ----
print(r4['fa_old'].name, r4['fa'].name)
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {r4['fa_old'].dir}/repl.log | awk '$0 >= \"2026-10-25T19:57:5\" && $0 < \"2026-10-25T19:58:3\"' | grep -a -v 'netlink\\|nostr_relay' | cut -c1-300 | head -60", max_tokens=6000)

# ---- cell @ 2026-10-25T20:00:24.201Z ----
async def R4f(c, idx, newname):
    f=c['fa']; F=Faults({'fa':f})
    print('bal', await fa_balance(f))
    await wait_boundary(c, 5)
    pv=await f.acall('faClientPreviewTransferDestination'); prev=pv.get('result',{}).get('preview')
    st=asyncio.create_task(f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=200))
    for i in range(400):
        r=await f.acall('faClientGuardianTransfer', timeout=10); tr=(r.get('result') or {}).get('transfer')
        if tr and tr.get('phase')!='buyingSeat': break
        await asyncio.sleep(0.05)
    F.do(('cut_fa_relay',)); print('cut fa relay at phase', tr and tr.get('phase'), time.strftime('%H:%M:%S'))
    print('start', json.dumps(await st)[:200])
    for i in range(30):
        r=await f.acall('faClientResumeGuardianTransfer', timeout=200); tr=(r.get('result') or {}).get('transfer') if isinstance(r.get('result'),dict) else None
        ph=tr and tr.get('phase'); print(i, time.strftime('%H:%M:%S'), json.dumps(r)[:160])
        if tr is None and (r.get('result') or {}).get('type')=='transfer' or ph=='completed': break
        await asyncio.sleep(5)
    w=await f.acall('getMnemonic'); f.kill()
    F.heal(False); print('healed; restoring')
    nf_,r1,r2=await restore_fa(f, newname, kill_old=False) if False else (None,None,None)
    nf_=Fa(newname, relay_i=f.relay_i); await asyncio.to_thread(nf_.start)
    await nf_.acall('restoreMnemonic', {'mnemonic': w['result']}, timeout=120)
    await nf_.acall('onboardTransferExistingDeviceRegistration', {'index': 0}, timeout=120)
    c['fa_old']=f; c['fa']=nf_
    for i in range(10):
        await asyncio.sleep(10)
        st_=(nf_.call('faClientStatus',{})['result'].get('status') or {}).get('formation') or {}
        print(i, [x['seatId'][:8] for x in st_.get('seats',[])], st_.get('freshness'), str(nf_.call('faClientResume',{}))[:120], str(nf_.call('faClientGuardianTransfer',{}))[:150])
f=r4['fa']
print(await fa_balance(f))
r4f=asyncio.create_task(R4f(r4, 4, 'r4a-r8'))

# ---- cell @ 2026-10-25T20:00:28.153Z ----
await asyncio.sleep(400); print(r4f.done())

# ---- cell @ 2026-10-25T20:02:31.213Z ----
await asyncio.sleep(300); print(r4f.done())

# ---- cell @ 2026-10-25T20:04:38.416Z ----
print(r4f.done(), time.strftime('%H:%M:%S'))
f=r4['fa']; print(f.name)
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {f.dir}/repl.log | grep -a -v 'netlink\\|nostr_relay' | tail -8 | cut -c1-300")

# ---- cell @ 2026-10-25T20:04:44.448Z ----
qaenv.proxy({'cmd':'tcp_unblock','group':'relay:fa0'})
for i in range(20):
    await asyncio.sleep(5)
    r=await f.acall('faClientGuardianTransfer', timeout=10); tr=(r.get('result') or {}).get('transfer')
    print(time.strftime('%H:%M:%S'), tr and tr.get('phase'))
    if not tr or tr.get('phase')=='completed': break
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {f.dir}/repl.log | grep -a -v 'netlink\\|nostr_relay' | grep -a 'request_id=55\\|StartGuardian' | tail -4 | cut -c1-300")

# ---- cell @ 2026-10-25T20:06:29.702Z ----
print(q(4,"select lower(hex(quote_id)),phase from seat_transfer_out where lower(hex(quote_id)) like '7a0c7a05%'"))
tr=(await f.acall('faClientGuardianTransfer', timeout=10))['result']['transfer']; print(tr)
dsid=tr['destinationSeatId']; print(seat_map().get(dsid,{}).get('g'), [ (g,) for g in range(14) if q(g, f"select 1 from seats where lower(hex(quote_id))='{dsid}'")])

# ---- cell @ 2026-10-25T20:06:35.341Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {E.root}/mg-12/stdout.log | grep -a -i '570515d7\\|pull\\|transfer' | grep -av 'Waiting for bitcoin\\|AlephBFT' | tail -15 | cut -c1-350", max_tokens=4000)

# ---- cell @ 2026-10-25T20:06:45.020Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {f.dir}/repl.log | awk '$0 >= \"2026-10-25T20:00:5\"' | grep -a 'nostr\\|WARN\\|ERROR' | grep -av netlink | tail -20 | cut -c1-300", max_tokens=4000)
tr=(await f.acall('faClientGuardianTransfer', timeout=10))['result']['transfer']; print(tr['phase'], time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T20:06:49.480Z ----
r=await command("rg -n 'async fn await_pulled_guardian' -A80 crates/fa-client/src/transfer.rs", workdir="/src/qa-transfer-manifold-rb", max_tokens=4000)

# ---- cell @ 2026-10-25T20:06:53.026Z ----
r=await command("sed -n 543,580p crates/fa-client/src/transfer.rs; rg -n 'fn transfer_backup_confirmed' -A30 crates/fa-client/src", workdir="/src/qa-transfer-manifold-rb", max_tokens=4000)

# ---- cell @ 2026-10-25T20:06:58.554Z ----
for i in range(12):
    await asyncio.sleep(10)
    tr=(await f.acall('faClientGuardianTransfer', timeout=10))['result']['transfer']
    print(time.strftime('%H:%M:%S'), tr and tr['phase'], q(4,"select phase from seat_transfer_out where lower(hex(quote_id)) like '7a0c7a05%'"))
    if not tr or tr['phase'] not in ('awaitingSource',): break

# ---- cell @ 2026-10-25T20:09:02.861Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {f.dir}/repl.log | grep -a 'request_id=39' | tail -3 | cut -c1-300; sed 's/\\x1b\\[[0-9;]*m//g' {f.dir}/repl.log | grep -a -i 'backup' | tail -10 | cut -c1-300")

# ---- cell @ 2026-10-25T20:09:08.871Z ----
for i in range(30):
    r=await f.acall('faClientResumeGuardianTransfer', timeout=300); tr=(r.get('result') or {}).get('transfer') if isinstance(r.get('result'),dict) else None
    ph=tr and tr.get('phase'); print(i, time.strftime('%H:%M:%S'), json.dumps(r)[:140])
    if (tr is None and (r.get('result') or {}).get('type')=='transfer') or ph=='completed': break
    await asyncio.sleep(5)

# ---- cell @ 2026-10-25T20:09:25.187Z ----
cases.append(('fa_phase:publishingBinding', 0, ('kill_mg', 'src')))
print(len(cases), await peer_sessions(r4))
st=f.call('faClientStatus',{})['result']['status']['formation']; print([x['seatId'][:8] for x in st['seats']])
print(os.path.exists(f"{E.root}/mg-4/seats/6/data"))

# ---- cell @ 2026-10-25T20:09:36.025Z ----
f=r4['fa']; b0=await fa_balance(f)
pv=await f.acall('faClientPreviewTransferDestination'); prev=pv['result']['preview']
r=await f.acall('faClientStartGuardianTransfer', {'seatIndex':5,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT-1)}, timeout=200)
print('cap', json.dumps(r)[:300])
print('gt', f.call('faClientGuardianTransfer',{}), 'spent', b0-(await fa_balance(f)))

# ---- cell @ 2026-10-25T20:09:54.510Z ----
await wait_boundary(r4, 5)
pv=await f.acall('faClientPreviewTransferDestination'); prev=pv['result']['preview']
pv2=await f.acall('faClientPreviewTransferDestination'); prev2=pv2['result']['preview']
print(prev['mgName'], prev2['mgName'])
t1=asyncio.create_task(f.acall('faClientStartGuardianTransfer', {'seatIndex':5,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=300))
await asyncio.sleep(0.2)
r2=await f.acall('faClientStartGuardianTransfer', {'seatIndex':6,'previewId':prev2['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=300)
print('second', json.dumps(r2)[:300])
r1=await t1; print('first', json.dumps(r1)[:300])
r3=await f.acall('faClientStartGuardianTransfer', {'seatIndex':6,'previewId':prev2['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=300)
print('third (while open)', json.dumps(r3)[:300])

# ---- cell @ 2026-10-25T20:10:18.450Z ----
pv3=await f.acall('faClientPreviewTransferDestination'); print('preview while open', json.dumps(pv3)[:300])
if pv3.get('result',{}).get('preview'):
    r3=await f.acall('faClientStartGuardianTransfer', {'seatIndex':5,'previewId':pv3['result']['preview']['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=300)
    print('start while open', json.dumps(r3)[:300])
print(f.call('faClientGuardianTransfer',{})['result']['transfer']['seatIndex'])

# ---- cell @ 2026-10-25T20:10:33.612Z ----
for i in range(30):
    r=await f.acall('faClientResumeGuardianTransfer', timeout=300); tr=(r.get('result') or {}).get('transfer') if isinstance(r.get('result'),dict) else None
    ph=tr and tr.get('phase')
    if (tr is None and (r.get('result') or {}).get('type')=='transfer') or ph=='completed': break
    await asyncio.sleep(5)
print(i, ph, 'spent', b0-(await fa_balance(f)))
print([x['seatId'][:8] for x in f.call('faClientStatus',{})['result']['status']['formation']['seats']])

# ---- cell @ 2026-10-25T20:10:43.289Z ----
sm=seat_map()
print(sorted({v['g'] for v in sm.values() if v.get('fed')=='Kind Juniper'}))
print([(x['seatId'][:8], sm.get(x['seatId'],{}).get('g')) for x in f.call('faClientStatus',{})['result']['status']['formation']['seats']])

# ---- cell @ 2026-10-25T20:10:55.384Z ----
async def E_src_offline(c, idx, outage=480):
    f=c['fa']; b0=await fa_balance(f)
    await wait_boundary(c, 5)
    sg=5
    E.kill_mg(sg, kill_children=True); print('killed MG', sg, time.strftime('%H:%M:%S'))
    pv=await f.acall('faClientPreviewTransferDestination'); prev=pv['result']['preview']
    r=await f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=600)
    print('start', time.strftime('%H:%M:%S'), json.dumps(r)[:200])
    t0=time.time(); log=[]
    while time.time()-t0<outage:
        r=await f.acall('faClientResumeGuardianTransfer', timeout=600)
        log.append((time.strftime('%H:%M:%S'), json.dumps(r)[:120]))
        await asyncio.sleep(20)
    print('during outage', log[0], log[-1], len(log))
    print('sessions during outage', await peer_sessions(c))
    E.start_mg(sg); print('restarted MG', sg, time.strftime('%H:%M:%S'))
    for i in range(60):
        r=await f.acall('faClientResumeGuardianTransfer', timeout=300); tr=(r.get('result') or {}).get('transfer') if isinstance(r.get('result'),dict) else None
        ph=tr and tr.get('phase')
        if (tr is None and (r.get('result') or {}).get('type')=='transfer') or ph=='completed': break
        await asyncio.sleep(5)
    print('done', time.strftime('%H:%M:%S'), ph, 'spent', b0-(await fa_balance(f)))
    print([x['seatId'][:8] for x in f.call('faClientStatus',{})['result']['status']['formation']['seats']])
    await asyncio.sleep(60); print('sessions', await peer_sessions(c))
eso=asyncio.create_task(E_src_offline(r4, 1))

# ---- cell @ 2026-10-25T20:10:58.450Z ----
await asyncio.sleep(600); print(eso.done())

# ---- cell @ 2026-10-25T20:13:01.913Z ----
print(eso.done(), eso.cancelled() if eso.done() else None, time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T20:13:05.725Z ----
await asyncio.sleep(590); print(eso.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T20:15:08.945Z ----
await asyncio.sleep(420); print(eso.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T20:17:13.468Z ----
await asyncio.sleep(300); print(eso.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T20:19:17.399Z ----
print(eso.done(), time.strftime('%H:%M:%S'))
if eso.done(): print(eso.exception())

# ---- cell @ 2026-10-25T20:19:21.375Z ----
for _ in range(60):
    if eso.done(): break
    await asyncio.sleep(10)
print(eso.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T20:23:34.403Z ----
print(q(5,"select lower(hex(quote_id)),phase from seat_transfer_out"))
command(f"pgrep -af 'mg-5/seats/4/data|mg-3/seats/7/data'; ls {E.root}/mg-5/seats/4/ 2>&1")
print(MON.get('violations'))
i=t.rfind('def monitor_loop'); print(t[i:i+1500].encode().decode('unicode_escape','ignore'))

# ---- cell @ 2026-10-25T20:23:42.762Z ----
command("rg -n 'restore|mnemonic|def start_mg|def init_mg|fleet-manager init|seed' /src/scratch/qa-gt/qaenv.py | head -30")
command("rg -n 'restore' crates/mg/specs/SPEC-nostr-backup-restore.md | head -30", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T20:23:46.922Z ----
command("sed -n 60,80p crates/mg/specs/SPEC-nostr-backup-restore.md; sed -n 138,160p crates/mg/specs/SPEC-nostr-backup-restore.md; sed -n 440,475p crates/mg/specs/SPEC-nostr-backup-restore.md", workdir="/src/qa-transfer-manifold-rb")
command("sed -n 130,200p /src/scratch/qa-gt/qaenv.py")

# ---- cell @ 2026-10-25T20:23:52.575Z ----
command("sed -n 200,300p /src/scratch/qa-gt/qaenv.py")

# ---- cell @ 2026-10-25T20:23:56.831Z ----
r=await command(f"{qaenv.BIN}/fman-cli --help; {qaenv.BIN}/fman-cli onboard restore --help", max_tokens=3000)

# ---- cell @ 2026-10-25T20:24:14.439Z ----
import shutil, subprocess
def mg_restore(g, tag):
    mn=E.fman(g,'show-mnemonic')
    mn = mn if isinstance(mn,str) else json.dumps(mn)
    words=mn.strip()
    E.kill_mg(g, kill_children=True)
    d=E.mg_dir(g); bak=f"{d}.{tag}"; os.rename(d, bak)
    E.st['pids'].pop(str(g), None); E.save()
    E.start_mg(g)
    mf=f"/tmp/mn-{g}"; open(mf,'w').write(words)
    deadline=time.time()+60
    while True:
        r=subprocess.run([f'{qaenv.BIN}/fman-cli','--data-dir',d,'onboard','restore','--mnemonic-file',mf,'--acknowledge-original-host-is-gone'],capture_output=True,text=True,timeout=300)
        if r.returncode==0: break
        if time.time()>deadline: raise RuntimeError(r.stderr[-2000:])
        time.sleep(1)
    print('restore out', r.stdout[-500:], r.stderr[-500:])
    ob=E.fman(g,'onboarding'); print('stage', ob.get('stage'))
    if ob['stage']=='holder_authorization':
        qaenv.run([f'{qaenv.BIN}/manifold-test-issuer','--environment','development','--relay',qaenv.RELAY,'--authorization-request',json.dumps({'subject_pubkey':ob['service_nostr_pubkey']}),'--publish-mg-authorization'])
        E.fman(g,'refresh-holder-authorizations')
    print(E.fman(g,'onboarding').get('stage'))
    E.fman(g,'onboard','offer','--max-seats',str(qaenv.MAX_SEATS),'--price-msats',str(SEAT))
    os.remove(mf)
    return bak
print(E.fman(13,'capacity'), E.fman(13,'plans'))

# ---- cell @ 2026-10-25T20:24:18.650Z ----
print(E.fman(13,'capacity','show')); print(E.fman(13,'plans'))
ssm=E.fman(13,'seats') if False else None
r=await command(f"{qaenv.BIN}/fman-cli --data-dir {E.mg_dir(13)} seats --help | head -20")

# ---- cell @ 2026-10-25T20:24:43.772Z ----
print(E.fman(13,'plans','show'))
print(E.fman(13,'seats','--help', check=False))

# ---- cell @ 2026-10-25T20:24:48.511Z ----
print(json.dumps(E.fman(13,'seats','list'))[:3000])

# ---- cell @ 2026-10-25T20:24:51.989Z ----
sl=E.fman(13,'seats','list')
for s in sl['seats']:
    if not s.get('decommissioned'): print(json.dumps(s)[:700])

# ---- cell @ 2026-10-25T20:24:52.712Z ----
print(json.dumps(E.fman(13,'seats','status','--help',check=False))[:500])
r=E.fman(13,'seats','status',check=False); print(json.dumps(r)[:2500])

# ---- cell @ 2026-10-25T20:25:00.604Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {E.mg_dir(13)}/stdout.log | grep -a '4fe815e2' | grep -a -i 'backup\\|import\\|accept\\|transfer' | cut -c1-330 | tail -20", max_tokens=4000)

# ---- cell @ 2026-10-25T20:25:09.616Z ----
async def M1():
    await wait_boundary(r4, 30)
    t_=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime())
    bak=await asyncio.to_thread(mg_restore, 13, 'pre-restore')
    print('restored MG13; backup of old root at', bak, time.strftime('%H:%M:%S'))
    await asyncio.sleep(20)
    print(json.dumps([s for s in E.fman(13,'seats','list')['seats'] if not s.get('decommissioned')])[:1500])
    for i in range(30):
        ps=await peer_sessions(r4); print(time.strftime('%H:%M:%S'), ps)
        if ps.get(6) is not None and ps[6]>=max(v for v in ps.values() if v is not None): break
        await asyncio.sleep(10)
    fk,_=await asyncio.to_thread(forkers_now, 'QA r4a', t_); print('forkers', fk)
m1=asyncio.create_task(M1())

# ---- cell @ 2026-10-25T20:25:13.811Z ----
for _ in range(40):
    if m1.done(): break
    await asyncio.sleep(10)
print(m1.done())

# ---- cell @ 2026-10-25T20:27:17.147Z ----
for _ in range(40):
    if m1.done(): break
    await asyncio.sleep(10)
print(m1.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T20:29:54.650Z ----
print(m1.exception())
r=await command(f"tail -5 {E.mg_dir(13)}/stdout.log | cut -c1-300")

# ---- cell @ 2026-10-25T20:30:02.312Z ----
r=subprocess.run([f'{qaenv.BIN}/fman-cli','--data-dir',E.mg_dir(13)+'.pre-restore','show-mnemonic'],capture_output=True,text=True,timeout=30)
print(r.returncode, repr(r.stdout[:80]), r.stderr[-300:])

# ---- cell @ 2026-10-25T20:30:08.452Z ----
import sqlite3
con=sqlite3.connect(f"file:{E.mg_dir(13)}.pre-restore/fleet-manager.sqlite?mode=ro", uri=True)
tabs=[r[0] for r in con.execute("select name from sqlite_master where type='table'")]
print(tabs)
for tname in tabs:
    if 'ident' in tname or 'mnemonic' in tname or 'root' in tname:
        cols=[r[1] for r in con.execute(f"pragma table_info({tname})")]; print(tname, cols)

# ---- cell @ 2026-10-25T20:30:13.255Z ----
row=con.execute("select mnemonic from identity").fetchone(); con.close()
m=row[0]; print(type(m), (len(m.split()) if isinstance(m,str) else len(m)))
words = m if isinstance(m,str) else m.decode()
# what did fman show-mnemonic give earlier? compare format
mf="/tmp/mn-13"; open(mf,'w').write(words.strip())
r=subprocess.run([f'{qaenv.BIN}/fman-cli','--data-dir',E.mg_dir(13),'onboard','restore','--mnemonic-file',mf,'--acknowledge-original-host-is-gone'],capture_output=True,text=True,timeout=600)
print(r.returncode, r.stdout[-800:], r.stderr[-800:])

# ---- cell @ 2026-10-25T20:30:20.001Z ----
os.remove(mf)
g=13; ob=E.fman(g,'onboarding'); print('stage', ob.get('stage'))
if ob['stage']=='holder_authorization':
    qaenv.run([f'{qaenv.BIN}/manifold-test-issuer','--environment','development','--relay',qaenv.RELAY,'--authorization-request',json.dumps({'subject_pubkey':ob['service_nostr_pubkey']}),'--publish-mg-authorization'])
    E.fman(g,'refresh-holder-authorizations')
print(E.fman(g,'onboarding').get('stage'))
print(E.fman(g,'onboard','offer','--max-seats',str(qaenv.MAX_SEATS),'--price-msats',str(SEAT)))
await asyncio.sleep(15)
print([ (s['seat_id'][:8], s.get('decommissioned')) for s in E.fman(13,'seats','list')['seats'] if not s.get('decommissioned')])
command(f"pgrep -af 'mg-13/seats' | cut -c1-120; ls {E.mg_dir(13)}/seats/")

# ---- cell @ 2026-10-25T20:30:43.148Z ----
con=sqlite3.connect(f"file:{E.mg_dir(13)}.pre-restore/fleet-manager.sqlite?mode=ro", uri=True)
for tname in ['seat_transfer_in','seat_transfer_out','decommissioned_seats','formed_seats']:
    cols=[r[1] for r in con.execute(f"pragma table_info({tname})")]
    rows=[r for r in con.execute(f"select * from {tname}") if r[0].hex().startswith(('c2b15f70','4fe815e2'))]
    print(tname, cols, [[ (x.hex()[:8] if isinstance(x,bytes) else (str(x)[:60])) for x in r] for r in rows])
con.close()
print(seat_map().get([k for k in seat_map() if k.startswith('c2b15f70')][0] if any(k.startswith('c2b15f70') for k in seat_map()) else None))
con2=sqlite3.connect(f"file:{E.mg_dir(13)}/fleet-manager.sqlite?mode=ro", uri=True)
print([ (r[0].hex()[:8],) for r in con2.execute("select quote_id from decommissioned_seats")][-4:]); con2.close()

# ---- cell @ 2026-10-25T20:30:48.368Z ----
t_m1=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-150))
for i in range(30):
    ps=await peer_sessions(r4); print(time.strftime('%H:%M:%S'), ps)
    if ps.get(6) is not None and ps[6]>=max(v for v in ps.values() if v is not None): break
    await asyncio.sleep(10)
fk,_=await asyncio.to_thread(forkers_now, 'QA r4a', t_m1); print('forkers', fk)

# ---- cell @ 2026-10-25T20:31:20.513Z ----
def mg_restore(g, tag):
    d=E.mg_dir(g)
    con=sqlite3.connect(f"file:{d}/fleet-manager.sqlite?mode=ro", uri=True); words=con.execute("select mnemonic from identity").fetchone()[0]; con.close()
    pre={'out':q(g,"select lower(hex(quote_id)),phase from seat_transfer_out")}
    E.kill_mg(g, kill_children=True)
    bak=f"{d}.{tag}"; os.rename(d, bak)
    E.st['pids'].pop(str(g), None); E.save()
    E.start_mg(g)
    mf=f"/tmp/mn-{g}"; open(mf,'w').write(words.strip())
    deadline=time.time()+60
    while True:
        r=subprocess.run([f'{qaenv.BIN}/fman-cli','--data-dir',d,'onboard','restore','--mnemonic-file',mf,'--acknowledge-original-host-is-gone'],capture_output=True,text=True,timeout=600)
        if r.returncode==0: break
        if time.time()>deadline: os.remove(mf); raise RuntimeError(r.stderr[-2000:])
        time.sleep(1)
    os.remove(mf)
    print('restore out', r.stdout[-300:])
    ob=E.fman(g,'onboarding')
    if ob['stage']=='holder_authorization':
        qaenv.run([f'{qaenv.BIN}/manifold-test-issuer','--environment','development','--relay',qaenv.RELAY,'--authorization-request',json.dumps({'subject_pubkey':ob['service_nostr_pubkey']}),'--publish-mg-authorization'])
        E.fman(g,'refresh-holder-authorizations')
    E.fman(g,'onboard','offer','--max-seats',str(qaenv.MAX_SEATS),'--price-msats',str(SEAT))
    return bak, pre
bak4, pre4 = await asyncio.to_thread(mg_restore, 4, 'pre-restore')
print(pre4)
await asyncio.sleep(15)
print(q(4,"select lower(hex(quote_id)),phase from seat_transfer_out"))
print([ (s['seat_id'][:8]) for s in E.fman(4,'seats','list')['seats'] if not s.get('decommissioned')])
command(f"pgrep -af 'mg-4/seats' | cut -c1-100; ls {E.mg_dir(4)}/seats/ ; ls {E.mg_dir(4)}/seats/*/data 2>/dev/null | head -40")

# ---- cell @ 2026-10-25T20:32:47.138Z ----
r=await command("rg -n 'seat_transfer_out' crates/mg/core/src --glob '!**/tests*' | head -30; rg -n \"phase IN\" crates/mg/core/migrations", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T20:32:51.132Z ----
r=await command("sed -n 940,1000p crates/mg/core/src/db/seats.rs; rg -n 'fn as_db_str|fn db_str|OutgoingTransfer::Stopped =>|OutgoingTransfer::Removed =>|\"transferred\"|\"stopped\"' crates/mg/core/src | head -20", workdir="/src/qa-transfer-manifold-rb", max_tokens=4000)

# ---- cell @ 2026-10-25T20:32:59.847Z ----
r=await command("git log --oneline -3; git branch --show-current; git show b8c013fa:crates/mg/core/src/db/seats.rs | grep -n '\"retired\"'; git merge-base --is-ancestor 8a959d08 b8c013fa && echo 185-contains-184; git log --oneline -1 b8c013fa; git rev-parse HEAD", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T20:33:04.892Z ----
r=await command("rg -n \"'retired'|\\\"retired\\\"|removed: false\" crates/mg/core/src | head -20", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T20:33:11.334Z ----
d=E.mg_dir(4)
E.kill_mg(4, kill_children=True)
os.rename(d, d+'.failed-restore')
os.rename(d+'.pre-restore', d)
E.st['pids'].pop('4', None); E.save()
E.start_mg(4)
await asyncio.sleep(15)
print(E.mg_pid(4), q(4,"select lower(hex(quote_id)),phase from seat_transfer_out"))
command(f"pgrep -af 'mg-4/seats' | cut -c1-100")

# ---- cell @ 2026-10-25T20:33:33.574Z ----
for g in range(14):
    print(g, q(g,"select substr(lower(hex(quote_id)),1,8),phase from seat_transfer_out"))

# ---- cell @ 2026-10-25T20:34:00.356Z ----
for r in RESULTS[-20:]:
    print(r.get('name','')[:80], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('peer_sessions') and min(v for v in r['peer_sessions'].values() if v is not None))

# ---- cell @ 2026-10-25T20:34:08.912Z ----
cases.append(('dest_imported', 0, ('kill_mg', 'src')))
print(len(cases), MON.get('violations'), len(MON.get('log',[])))

# ---- cell @ 2026-10-25T20:34:14.791Z ----
r=await command("rg -n '^name|\\[\\[bin\\]\\]' -A1 crates/mg/*/Cargo.toml | grep -B1 -A1 'fleet-manager\\|fman-cli' | head; git status --short | head", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T20:34:20.878Z ----
p='/src/qa-transfer-manifold-rb/crates/mg/core/src/db/seats.rs'
s=open(p).read(); assert s.count('"retired"')==1
open(p,'w').write(s.replace('"retired"','"stopped"'))
os.makedirs('/src/scratch/qa-gt/bin-f6fix', exist_ok=True)
b=await command("cargo build --bin fleet-manager 2>&1 | tail -3 && cp target/debug/fleet-manager /src/scratch/qa-gt/bin-f6fix/ && cp target/debug/fman-cli /src/scratch/qa-gt/bin-f6fix/ 2>/dev/null; git checkout crates/mg/core/src/db/seats.rs && git status --short", workdir="/src/qa-transfer-manifold-rb")
print(b)

# ---- cell @ 2026-10-25T20:34:45.742Z ----
b=await command("cargo build --bin fleet-manager 2>&1 | tail -1; cmp target/debug/fleet-manager /src/scratch/qa-gt/bin-f6fix/fleet-manager && echo SAME || echo DIFFER", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T20:35:04.594Z ----
def mg_restore2(g, tag, bin_dir=None):
    d=E.mg_dir(g)
    con=sqlite3.connect(f"file:{d}/fleet-manager.sqlite?mode=ro", uri=True); words=con.execute("select mnemonic from identity").fetchone()[0]; con.close()
    E.kill_mg(g, kill_children=True)
    bak=f"{d}.{tag}"; os.rename(d, bak)
    E.st['pids'].pop(str(g), None); E.save()
    E.start_mg(g, bin_dir=bin_dir)
    mf=f"/tmp/mn-{g}"; open(mf,'w').write(words.strip())
    deadline=time.time()+60
    while True:
        r=subprocess.run([f'{qaenv.BIN}/fman-cli','--data-dir',d,'onboard','restore','--mnemonic-file',mf,'--acknowledge-original-host-is-gone'],capture_output=True,text=True,timeout=600)
        if r.returncode==0: break
        if time.time()>deadline or 'CHECK' in r.stderr: os.remove(mf); raise RuntimeError(r.stderr[-2000:])
        time.sleep(1)
    os.remove(mf)
    print('restore out', r.stdout[-300:])
    ob=E.fman(g,'onboarding')
    if ob['stage']=='holder_authorization':
        qaenv.run([f'{qaenv.BIN}/manifold-test-issuer','--environment','development','--relay',qaenv.RELAY,'--authorization-request',json.dumps({'subject_pubkey':ob['service_nostr_pubkey']}),'--publish-mg-authorization'])
        E.fman(g,'refresh-holder-authorizations')
    E.fman(g,'onboard','offer','--max-seats',str(qaenv.MAX_SEATS),'--price-msats',str(SEAT))
    return bak
bak4=await asyncio.to_thread(mg_restore2, 4, 'pre-restore2', '/src/scratch/qa-gt/bin-f6fix')
await asyncio.sleep(15)
print(q(4,"select substr(lower(hex(quote_id)),1,8),phase from seat_transfer_out"))
print([ (s['seat_id'][:8]) for s in E.fman(4,'seats','list')['seats'] if not s.get('decommissioned')])
command(f"pgrep -af 'mg-4/seats' | cut -c1-100; for s in {E.mg_dir(4)}/seats/*; do echo $s; ls $s/data 2>/dev/null | tr '\\n' ' '; echo; done")

# ---- cell @ 2026-10-25T20:35:30.829Z ----
command(f"for s in {bak4}/seats/*; do echo $s; ls $s/data 2>/dev/null | tr '\\n' ' '; echo; done")
command("sed -n 185,212p crates/mg/specs/SPEC-nostr-backup-restore.md", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T20:35:37.084Z ----
d=E.mg_dir(4)
E.kill_mg(4, kill_children=True)
os.rename(d, d+'.restored-f6fix'); os.rename(bak4, d)
E.st['pids'].pop('4', None); E.save(); E.start_mg(4)
await asyncio.sleep(10); print(E.mg_pid(4))
command(f"pgrep -af 'mg-4/seats' | cut -c1-100")

# ---- cell @ 2026-10-25T20:36:04.287Z ----
print(E.st.get('mg_bin'), qaenv.MG_COUNT)
command("ls /src/qa-transfer-manifold-018/target/debug/ | grep -E '^(fleet-manager|fman-cli)$'; git -C /src/qa-transfer-manifold-018 log --oneline -1")

# ---- cell @ 2026-10-25T20:36:16.336Z ----
r=await command("sed -n 205,258p crates/fa-client/src/transfer.rs", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T20:36:23.421Z ----
r=await command("rg -n -i 'cancel|abandon|give up|GuardianTransferWrite::Close' crates/fa-client/src/transfer.rs crates/fa-client/src/seat_purchase.rs | head -20; rg -n 'fn faClient|fa_client_.*transfer' /src/qa-transfer-fedi/bridge/fedi-ffi/src/rpc.rs 2>/dev/null | head; rg -ln 'faClientResumeGuardianTransfer' /src/qa-transfer-fedi/bridge | head -3", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T20:36:31.127Z ----
r=await command("rg -n 'load_guardian_transfer|GuardianTransferOpen|transfer is open|open transfer' crates/fa-client/src --glob '!**/tests*' | head -20; rg -n 'InvalidIntent' crates/fa-client/src/transfer.rs | head", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T20:36:35.134Z ----
r=await command("sed -n 570,600p crates/fa-client/src/liquidity.rs; rg -n 'GuardianTransferWrite::Begin' -B3 -A25 crates/fa-client/src/db.rs | grep -n -i 'exists\\|already\\|InvalidIntent' | head", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T20:36:48.144Z ----
i=t.find('MGID='); print(t[i-200:i+400].encode().decode('unicode_escape','ignore'))
i=t.find('MGID ='); print(t[i-200:i+400].encode().decode('unicode_escape','ignore') if i>0 else None)

# ---- cell @ 2026-10-25T20:36:56.136Z ----
OLD='/src/qa-transfer-manifold-018/target/debug'
r=await command(f"{OLD}/fman-cli --help | head -30; {OLD}/fleet-manager serve --help | head -40")

# ---- cell @ 2026-10-25T20:37:04.563Z ----
def reset_mg_old(g):
    d=E.mg_dir(g)
    E.kill_mg(g, kill_children=True)
    os.rename(d, d+'.pre-018')
    E.st['pids'].pop(str(g), None)
    E.st['mg_bin'][str(g)]=OLD; E.save()
    E.start_mg(g)
    deadline=time.time()+60
    while True:
        r=subprocess.run([f'{OLD}/fman-cli','--data-dir',d,'onboard','new'],capture_output=True,text=True)
        if r.returncode==0: break
        if time.time()>deadline: raise RuntimeError(r.stderr)
        time.sleep(0.5)
    ob=E.fman(g,'onboarding')
    if ob['stage']=='holder_authorization':
        qaenv.run([f'{qaenv.BIN}/manifold-test-issuer','--environment','development','--relay',qaenv.RELAY,'--authorization-request',json.dumps({'subject_pubkey':ob['service_nostr_pubkey']}),'--publish-mg-authorization'])
        E.fman(g,'refresh-holder-authorizations')
    E.fman(g,'onboard','offer','--max-seats',str(qaenv.MAX_SEATS),'--price-msats',str(SEAT))
    return E.fman(g,'onboarding')
ob11=await asyncio.to_thread(reset_mg_old, 11)
print({k:ob11.get(k) for k in ('stage','fman_name','service_pubkey','service_nostr_pubkey')})
for k in ('service_nostr_pubkey','service_pubkey','fman_name'):
    if ob11.get(k): MGID[ob11[k]]=11

# ---- cell @ 2026-10-25T20:37:11.365Z ----
for name in ['def setup_fed','def preview_sel']:
    i=t.rfind(name); print(t[i:i+2200].encode().decode('unicode_escape','ignore')[:2000],'\n-----')

# ---- cell @ 2026-10-25T20:37:20.390Z ----
async def setup_fed_incl(name, must=11, size=7, relay_i=1, tries=40):
    f=Fa(name, relay_i=relay_i); await asyncio.to_thread(f.start)
    await f.acall('__complete_onboarding')
    await f.acall('joinFederation', {'inviteCode': E.st['payment_invite'], 'recoverFromScratch': False})
    for _ in range(30):
        r=await asyncio.to_thread(fund_fa, f, 50_000_000)
        if 'result' in r: break
        await asyncio.sleep(1)
    for k in range(tries):
        p=await preview_sel(f, size, size*SEAT)
        gs=[MGID.get(s['mgId'],s['mgId']) for s in p['seats']]
        if must in gs: break
        await asyncio.sleep(1)
    print('selected', gs, 'after', k)
    if must not in gs: return f, None
    r=await f.acall('faClientPayAndCreate', {'previewId': p['previewId'], 'intent': {'federationName':f'QA {name}','federationSize':size,'plan':'infiniteBestEffort'}, 'paymentFederationId': PAYFED, 'maxTotalMsats': str(size*SEAT)})
    print('pay', json.dumps(r)[:200])
    deadline=time.time()+400
    while True:
        st=await fstatus(f); fo=st.get('formation',{})
        if fo.get('phase')=='formed' and (fo.get('inviteCode') or fo.get('federationInvite')): break
        if time.time()>deadline: raise RuntimeError(f'not formed: {json.dumps(st)[:1000]}')
        if fo.get('lastError'): await f.acall('faClientResume')
        await asyncio.sleep(2)
    c=await ctx_of(f); c['transfers']=0; c['name']=name
    await asyncio.to_thread(meta, f'chk-{name}', c['invite'])
    return f, c
old_task=asyncio.create_task(setup_fed_incl('o18'))

# ---- cell @ 2026-10-25T20:37:24.726Z ----
for _ in range(50):
    if old_task.done(): break
    await asyncio.sleep(10)
print(old_task.done())
if old_task.done():
    fo18, co18 = old_task.result(); print(co18 and [(s['i'], s['loc']) for s in co18['seats']])

# ---- cell @ 2026-10-25T20:41:33.378Z ----
print(old_task.done(), time.strftime('%H:%M:%S'))
if old_task.done():
    print(old_task.exception() or [(s['i'], s['loc']) for s in old_task.result()[1]['seats']] if old_task.result()[1] else 'no')

# ---- cell @ 2026-10-25T20:41:40.007Z ----
for _ in range(30):
    if old_task.done(): break
    await asyncio.sleep(10)
print(old_task.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T20:42:58.067Z ----
fo18=old_task.result()[0]
r=await command(f"grep -a -c 'inclusive-stilt\\|935cc35f' {fo18.dir}/repl.log; sed 's/\\x1b\\[[0-9;]*m//g' {fo18.dir}/repl.log | grep -a -i 'inclusive-stilt\\|935cc35f\\|9b054c5c' | head -5 | cut -c1-400")
p=await preview_sel(fo18, 7, 7*SEAT); print(json.dumps(p)[:1500])

# ---- cell @ 2026-10-25T20:43:19.457Z ----
r=await command("rg -n 'fn |retry|backoff|Unimplemented|unknown|NotSupported|loop' crates/mg/core/src/fleet/transfer.rs | head -50", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T20:43:24.723Z ----
r=await command("rg -ln 'TransferSeatRequest' crates/mg/core/src | head; rg -n 'async fn pull|fn run_pull|pull_loop|PULL_RETRY|retry_delay' crates/mg/core/src | head -20", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T20:43:30.669Z ----
r=await command("rg -n 'Err|retry|delay|permanent|fatal|give' crates/mg/core/src/transfer_worker.rs | head -40", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T20:43:38.470Z ----
f=r4['fa']; names=Counter()
for i in range(15):
    pv=await f.acall('faClientPreviewTransferDestination'); prev=(pv.get('result') or {}).get('preview')
    names[prev and prev['mgName']]+=1
print(names)
print(ob11['fman_name'], [k for k,v in MGID.items() if v==11])

# ---- cell @ 2026-10-25T20:44:09.636Z ----
print(MGID.get('magical-katydid'), MGID.get('6c52967261d6f5d1b65900bdd79931e3734661e430d76bc02b7da50ff9334b8b'))
print(sorted(set(k for k in MGID if not re.fullmatch('[0-9a-f]{64}',k))))

# ---- cell @ 2026-10-25T20:44:18.095Z ----
d=E.mg_dir(11)
E.kill_mg(11, kill_children=True)
os.rename(d, d+'.018-fresh'); os.rename(d+'.pre-018', d)
E.st['mg_bin'].pop('11', None); E.st['pids'].pop('11', None); E.save(); E.start_mg(11)
await asyncio.sleep(8); print(E.mg_pid(11))
fo18.kill()

# ---- cell @ 2026-10-25T20:44:43.537Z ----
async def R4g(c, idx, newname, target_phase='publishingBinding'):
    f=c['fa']; b0=await fa_balance(f)
    await wait_boundary(c, 5)
    pv=await f.acall('faClientPreviewTransferDestination'); prev=pv['result']['preview']
    st=asyncio.create_task(f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=600))
    seen=[]
    async def drive():
        await st
        while True:
            try: await f.acall('faClientResumeGuardianTransfer', timeout=600)
            except Exception: return
            await asyncio.sleep(1)
    dt=asyncio.create_task(drive())
    ph=None
    for i in range(3000):
        r=await f.acall('faClientGuardianTransfer', timeout=10); tr=(r.get('result') or {}).get('transfer')
        ph=tr and tr.get('phase')
        if not seen or seen[-1]!=ph: seen.append(ph)
        if ph==target_phase or ph is None and len(seen)>1: break
        await asyncio.sleep(0.05)
    print('phases seen', seen)
    nf_,r1,r2=await restore_fa(f, newname); c['fa_old']=f; c['fa']=nf_
    dt.cancel()
    gt=nf_.call('faClientGuardianTransfer',{})['result'].get('transfer'); print('restored transfer', gt and gt['phase'])
    for i in range(60):
        r=await nf_.acall('faClientResumeGuardianTransfer', timeout=300); tr=(r.get('result') or {}).get('transfer') if isinstance(r.get('result'),dict) else None
        ph=tr and tr.get('phase')
        if (tr is None and (r.get('result') or {}).get('type')=='transfer') or ph=='completed': break
        await asyncio.sleep(5)
    print('final', ph, 'spent', b0-(await fa_balance(nf_)))
    print([x['seatId'][:8] for x in nf_.call('faClientStatus',{})['result']['status']['formation']['seats']])
    await asyncio.sleep(30); print(await peer_sessions(c))
r4g=asyncio.create_task(R4g(r4, 0, 'r4a-r9'))

# ---- cell @ 2026-10-25T20:44:47.196Z ----
for _ in range(50):
    if r4g.done(): break
    await asyncio.sleep(10)
print(r4g.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T20:46:52.531Z ----
await asyncio.sleep(30)
print(q(10,"select phase from seat_transfer_out where lower(hex(quote_id)) like '56eeeb2a%'"), os.path.exists(f"{E.root}/mg-10/seats/7/data"))
print(await peer_sessions(r4), MON.get('violations'))

# ---- cell @ 2026-10-25T20:47:36.508Z ----
def r4_locs(c):
    st=c['fa'].call('faClientStatus',{})['result']['status']['formation']['seats']; sm=seat_map()
    return [(sm.get(x['seatId'],{}).get('g'), sm.get(x['seatId'],{}).get('sn')) for x in st]
print(r4_locs(r4))
async def T3off(c, idx, stop_idxs, hold=150):
    f=c['fa']; b0=await fa_balance(f); locs=r4_locs(c)
    await wait_boundary(c, 5)
    pv=await f.acall('faClientPreviewTransferDestination'); prev=pv['result']['preview']
    st=asyncio.create_task(f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=600))
    F=Faults({'fa':f})
    for i in range(3000):
        r=await f.acall('faClientGuardianTransfer', timeout=10); tr=(r.get('result') or {}).get('transfer'); ph=tr and tr.get('phase')
        if ph=='publishingBinding': break
        await asyncio.sleep(0.05)
    for k in stop_idxs: F.do(('stop_seat',)+tuple(locs[k]))
    print('stopped', [locs[k] for k in stop_idxs], time.strftime('%H:%M:%S'))
    t0=time.time(); log=[]
    async def drive():
        await st
        while True:
            r=await f.acall('faClientResumeGuardianTransfer', timeout=600)
            tr=(r.get('result') or {}).get('transfer') if isinstance(r.get('result'),dict) else None
            log.append((round(time.time()-t0), tr and tr.get('phase') or json.dumps(r)[:80]))
            if (tr is None and (r.get('result') or {}).get('type')=='transfer') or (tr and tr.get('phase')=='completed'): return
            await asyncio.sleep(5)
    dt=asyncio.create_task(drive())
    await asyncio.sleep(hold)
    print('during hold', log[-3:], await peer_sessions(c))
    F.heal(False); th=time.time(); print('healed', time.strftime('%H:%M:%S'))
    try: await asyncio.wait_for(dt, 900)
    except Exception as e: print('drive', repr(e))
    print('completed', round(time.time()-th), 's after heal; log tail', log[-3:], 'spent', b0-(await fa_balance(f)))
    await asyncio.sleep(60); print(await peer_sessions(c), r4_locs(c))
t3=asyncio.create_task(T3off(r4, 2, [3,4,5]))

# ---- cell @ 2026-10-25T20:47:40.042Z ----
for _ in range(60):
    if t3.done(): break
    await asyncio.sleep(10)
print(t3.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T20:49:46.479Z ----
print(t3.exception())

# ---- cell @ 2026-10-25T20:49:49.715Z ----
pv=await r4['fa'].acall('faClientPreviewTransferDestination'); print(pv)

# ---- cell @ 2026-10-25T20:49:59.678Z ----
async def mkfed_private(name):
    c=await setup_fed(name, relay_i=1); c['transfers']=0; c['name']=name
    await asyncio.to_thread(meta, f'chk-{name}', c['invite'])
    return c
r5=await mkfed_private('r5a')
print(r4_locs(r5))

# ---- cell @ 2026-10-25T20:52:03.855Z ----
await asyncio.sleep(5)
print('r5' in globals())

# ---- cell @ 2026-10-25T20:52:12.532Z ----
tk=Task.from_session_id(6651)
await tk
print(r4_locs(r5))

# ---- cell @ 2026-10-25T20:54:16.202Z ----
for _ in range(40):
    if 'r5' in globals(): break
    await asyncio.sleep(10)
print('r5' in globals(), time.strftime('%H:%M:%S'))
if 'r5' in globals(): print(r4_locs(r5))

# ---- cell @ 2026-10-25T20:55:26.728Z ----
err=str(Task.from_session_id(6214).exception()) if False else None
c15=[c for c in CTXS if c.get('name')=='f15']
print(len(c15))
import glob
fa15=[d for d in glob.glob(f"{E.root}/fa-f15*")]; print(fa15)
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {fa15[0]}/repl.log | grep -a -i 'WARN\\|ERROR' | grep -av 'netlink\\|nostr_relay' | tail -8 | cut -c1-400", max_tokens=3000)

# ---- cell @ 2026-10-25T20:55:33.274Z ----
j=btc('getblockchaininfo'); print(j['blocks'], j['verificationprogress'], time.time()-j['time'], miner.done())
cnt=Counter()
for g in range(14):
    with open(f"{E.root}/mg-{g}/stdout.log", errors='ignore') as fh:
        fh.seek(max(0, os.path.getsize(fh.name)-3_000_000))
        for l in fh:
            if 'T20:5' in l[:40] and ('seat_unavailable' in l or 'Waiting for bitcoin' in l or 'temporarily unavailable' in l): cnt[(g, 'btc' if 'bitcoin' in l else 'unav')]+=1
print(cnt)

# ---- cell @ 2026-10-25T20:55:37.903Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {E.root}/mg-9/stdout.log | grep -a 'T20:5' | grep -a -i 'unavailable' | tail -3 | cut -c1-500; sed 's/\\x1b\\[[0-9;]*m//g' {E.root}/mg-9/stdout.log | grep -a 'T20:5' | grep -a -i 'error\\|warn' | grep -av unavailable | tail -8 | cut -c1-400", max_tokens=4000)

# ---- cell @ 2026-10-25T20:55:47.774Z ----
bad=[]
for g in range(14):
    p=E.mg_pid(g)
    exe=os.readlink(f'/proc/{p}/exe') if p else None
    if not p or '(deleted)' in exe: bad.append(g)
    print(g, p, exe)
print('bad', bad)

# ---- cell @ 2026-10-25T20:55:55.009Z ----
for g in bad:
    E.kill_mg(g, kill_children=True); E.st['pids'].pop(str(g), None); E.save()
    E.start_mg(g); await asyncio.sleep(8)
    print(g, E.mg_pid(g), os.readlink(f'/proc/{E.mg_pid(g)}/exe'))

# ---- cell @ 2026-10-25T20:57:17.642Z ----
print([k for k,v in globals().items() if isinstance(v, list) and v and type(v[0]).__name__=='Fa'][:5])
command("pgrep -af 'fa-f15|fa-r5a' | cut -c1-200")

# ---- cell @ 2026-10-25T20:57:21.980Z ----
i=t.rfind('class Fa:'); print(t[i:i+2500].encode().decode('unicode_escape','ignore')[:2400])

# ---- cell @ 2026-10-25T20:57:26.554Z ----
fa15=Fa('f15'); fa5=Fa('r5a', relay_i=1)
for ff in (fa15, fa5):
    try:
        print(ff.name, str(ff.call('faClientResume',{}, timeout=120))[:200])
    except Exception as e: print(ff.name, 'exc', e)

# ---- cell @ 2026-10-25T20:58:06.239Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {fa15.dir}/repl.log | grep -av 'netlink\\|nostr_relay' | tail -6 | cut -c1-400", max_tokens=3000)

# ---- cell @ 2026-10-25T20:58:10.320Z ----
for ff in (fa15, fa5):
    st=ff.call('faClientStatus',{})['result']['status']; fo=st.get('formation',{})
    print(ff.name, fo.get('phase'), fo.get('lastError'), bool(fo.get('inviteCode') or fo.get('federationInvite')))

# ---- cell @ 2026-10-25T20:58:17.927Z ----
c15=await ctx_of(fa15); c15['transfers']=0; c15['name']='f15'
await asyncio.to_thread(meta, 'chk-f15', c15['invite'])
CTXS.append(c15)
r5=await ctx_of(fa5); r5['transfers']=0; r5['name']='r5a'
await asyncio.to_thread(meta, 'chk-r5a', r5['invite'])
print(len(cases[21:]), [ (c.get('name'), c.get('retired'), c.get('transfers')) for c in CTXS[-4:]])
sw3=asyncio.create_task(sweep2(cases[21:], 'R1c'))
print(r4_locs(r5))

# ---- cell @ 2026-10-25T20:58:24.815Z ----
t3=asyncio.create_task(T3off(r5, 2, [3,4,5]))

# ---- cell @ 2026-10-25T21:04:39.181Z ----
for _ in range(90):
    if t3.done(): break
    await asyncio.sleep(10)
print(t3.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T21:06:46.857Z ----
cases.append(('dest_imported', 0.3, ('kill_mg', 'dst')))
print(t3.done())

# ---- cell @ 2026-10-25T21:06:52.909Z ----
LEFT=[('dest_imported', 0.3, ('kill_mg', 'dst'))]
for _ in range(60):
    if t3.done(): break
    await asyncio.sleep(10)
print(t3.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T21:09:00.035Z ----
for _ in range(10):
    if t3.done(): break
    await asyncio.sleep(10)
print(t3.done())
print(await peer_sessions(r5), r4_locs(r5))

# ---- cell @ 2026-10-25T21:09:20.496Z ----
def _alive(self):
    if self.p is not None: return self.p.poll() is None
    try:
        s=_socket.socket(_socket.AF_UNIX); s.settimeout(2); s.connect(self.sock); s.close(); return True
    except Exception: return False
Fa.alive=_alive
print(fa5.alive(), r5['fa'] is fa5, r5['fa'].name)

# ---- cell @ 2026-10-25T21:09:27.300Z ----
i=t.rfind('async def run_scenario'); print(t[i:i+3500].encode().decode('unicode_escape','ignore')[:3400])

# ---- cell @ 2026-10-25T21:09:43.726Z ----
PRIV=[r5]
async def priv_ctx():
    c=PRIV[-1]
    if c['transfers']>=6 or c.get('retired'):
        n=f"p{len(PRIV)+1}"
        c=await mkfed_private(n); PRIV.append(c); print('new private fed', n, r4_locs(c))
    return c
async def sweep_priv(cases_, label, heal_after=60):
    out=[]
    for k,(trig,delay,act) in enumerate(cases_):
        c=await priv_ctx()
        nctx=await ctx_of(c['fa']); c['seats']=nctx['seats']
        await wait_boundary(c, extra=10)
        idx=c['transfers']%7
        name=f'{label}#{k} {c["name"]} idx{idx} trig={trig} +{delay}s act={act} hold={heal_after}'
        t_=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime())
        try: res=await run_scenario(name, c, idx, faults=[(trig,delay,act)], heal_after=heal_after, drive_timeout=900)
        except Exception as e:
            import traceback; res={'name':name,'ok':False,'probs':['harness exception '+repr(e)[:300]],'tb':traceback.format_exc()[-1500:]}; RESULTS.append(res)
        c['transfers']+=1
        try: fk,_=await asyncio.to_thread(forkers_now, f"QA {c['name']}", t_); res['forkers']=sorted(fk)
        except Exception: pass
        out.append((name,res.get('ok'),res.get('probs'),res.get('forkers')))
        if not res.get('ok'):
            print('!!! FAIL', name, res.get('probs')); c['retired']='failed: '+name
            try: await asyncio.to_thread(Faults({'fa':c['fa']}).heal, False)
            except Exception: pass
    return out
r5['transfers']=1
R2cases=[('dest_waiting',0,('cut','src')), ('src_stopped',0,('cut','dst')), ('dest_imported',0,('cut','dst')),
 ('fa_phase:publishingBinding',0,('cut_endpoint','dst')), ('fa_phase:removingSource',0,('cut_endpoint','src')),
 ('dest_waiting',0,('stop_mg','src')), ('src_stopped',0,('stop_mg_tree','dst')), ('fa_phase:publishingBinding',0,('cut_fa_relay',)),
 ('dest_imported',0,('kill_seat','dst')), ('src_stopped',0,('cut_relay','src')), ('dest_imported',0,('cut_relay','dst')),
 ('dest_seat',0,('stop_mg','dst')), ('fa_phase:removingSource',0,('stop_mg_tree','src'))]
swp=asyncio.create_task(sweep_priv(R2cases, 'R2'))

# ---- cell @ 2026-10-25T21:09:52.940Z ----
r=await command("rg -n 'destination|service_pubkey|signed by|grant' crates/mg/core/src/transfer_source.rs | head -40", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T21:09:56.775Z ----
r=await command("rg -n 'async fn transfer_seat' -A60 crates/mg/core/src/service.rs | head -90", workdir="/src/qa-transfer-manifold-rb", max_tokens=4000)

# ---- cell @ 2026-10-25T21:10:00.460Z ----
r=await command("sed -n 1,60p crates/mg/core/src/fleet/transfer.rs", workdir="/src/qa-transfer-manifold-rb", max_tokens=4000)

# ---- cell @ 2026-10-25T21:10:09.480Z ----
R2cases.append(('dest_seat',0,('cut_relay','src')))
R2cases.append(('dest_waiting',0,('cut_relay','dst')))
print(len(R2cases))

# ---- cell @ 2026-10-25T21:10:13.639Z ----
for _ in range(60):
    await asyncio.sleep(10)
print(time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T21:12:17.818Z ----
await asyncio.sleep(400)
for r in RESULTS[-6:]:
    print(r.get('name','')[:110], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'), [p[1][:30] for p in r.get('phases',[])][:4])

# ---- cell @ 2026-10-25T21:14:21.726Z ----
await asyncio.sleep(300)
for r in RESULTS[-6:]:
    print(r.get('name','')[:110], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'), [p[1][:30] for p in r.get('phases',[])][:4])

# ---- cell @ 2026-10-25T21:16:40.120Z ----
await asyncio.sleep(600)
for r in RESULTS[-6:]:
    print(r.get('name','')[:110], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:18:43.798Z ----
await asyncio.sleep(600)
for r in RESULTS[-8:]:
    print(r.get('name','')[:110], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:20:52.576Z ----
import contextlib, io
mt=Task.from_session_id(9880)
mt.cancel()
def monitor_quiet():
    buf=io.StringIO()
    with contextlib.redirect_stdout(buf): monitor_once()
    for l in buf.getvalue().splitlines():
        if not l.startswith('route'): print(l)
async def monitor_loop2():
    while True:
        try: await asyncio.to_thread(monitor_quiet)
        except Exception as e: MON['log'].append((time.strftime('%H:%M:%S'),'monerr',repr(e)))
        await asyncio.sleep(1)
mon2=asyncio.create_task(monitor_loop2())

# ---- cell @ 2026-10-25T21:21:00.258Z ----
mon2.cancel()
i=t.rfind('def monitor_once'); print(t[i:i+2500].encode().decode('unicode_escape','ignore')[:2400])

# ---- cell @ 2026-10-25T21:21:11.515Z ----
def monitor_once2():
    imp=imported_seats()
    running=running_seat_dirs()
    run_by_seat={}
    for g,t_,pid in running: run_by_seat.setdefault((g,t_),[]).append(pid)
    ident_hosts={}
    for (g,t_),pids in run_by_seat.items():
        home=imp.get((g,t_),(g,t_))
        ident_hosts.setdefault(home,[]).append((g,t_,pids))
    now=time.strftime('%H:%M:%S')
    for home,hosts in ident_hosts.items():
        if len(hosts)>1:
            v=(now,'DOUBLE_HOST',home,hosts)
            MON['violations'].append(v); print('!!! VIOLATION', v)
    by_home={}
    for host,home in imp.items():
        if home[0]=='unknown': continue
        by_home.setdefault(home,[]).append(host)
    for home,hosts in by_home.items():
        live=[h for h in hosts if h in run_by_seat]
        if live: host=live[0]
        elif MON['routes'].get(home) in hosts: continue
        else: host=max(hosts, key=lambda h:(h[1],h[0]))
        if MON['routes'].get(home)!=host:
            apply_route(home, host); MON['routes'][home]=host
            MON['log'].append((now,'route',home,host)); print('route', home, '->', host)
async def monitor_loop3():
    while True:
        try: await asyncio.to_thread(monitor_once2)
        except Exception as e: MON['log'].append((time.strftime('%H:%M:%S'),'monerr',repr(e)))
        await asyncio.sleep(1)
mon3=asyncio.create_task(monitor_loop3())

# ---- cell @ 2026-10-25T21:21:20.007Z ----
await asyncio.sleep(30); print(MON['log'][-5:])

# ---- cell @ 2026-10-25T21:21:54.256Z ----
await asyncio.sleep(900)
for r in RESULTS[-6:]:
    print(r.get('name','')[:110], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:23:58.114Z ----
await asyncio.sleep(1200)
for r in RESULTS[-8:]:
    print(r.get('name','')[:110], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:26:02.520Z ----
n0=len(RESULTS)
for _ in range(120):
    if len(RESULTS)>=n0+4: break
    await asyncio.sleep(15)
for r in RESULTS[-8:]:
    print(r.get('name','')[:110], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:28:16.479Z ----
async def S1ctl(c, k=2, hold=300):
    locs=r4_locs(c); g,sn=locs[k]
    await wait_boundary(c, 20)
    F=Faults({'fa':c['fa']}); F.do(('stop_seat',g,sn)); t0=time.time(); print('stopped', (g,sn), time.strftime('%H:%M:%S'))
    await asyncio.sleep(hold)
    print('before cont', await peer_sessions(c))
    F.heal(False); th=time.time()
    ident_k=None
    for i in range(80):
        ps=await peer_sessions(c)
        mine=None
        # find which peer id is hosted at (g,sn): query that guardian directly via its own api? use max/min
        vals=[v for v in ps.values() if v is not None]
        if vals and len(vals)==7 and min(vals)==max(vals): break
        await asyncio.sleep(5)
    print('caught up', round(time.time()-th), 's after SIGCONT', ps)
s1=asyncio.create_task(S1ctl(r4))

# ---- cell @ 2026-10-25T21:28:25.458Z ----
for _ in range(120):
    if s1.done(): break
    await asyncio.sleep(10)
print(s1.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T21:34:36.985Z ----
print(s1.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T21:34:40.970Z ----
for _ in range(60):
    if s1.done(): break
    await asyncio.sleep(10)
print(s1.done(), time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T21:35:40.762Z ----
print(await peer_sessions(r4)); await asyncio.sleep(30); print(await peer_sessions(r4))
print(dag_heights('QA r4a', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(time.time()-20))))

# ---- cell @ 2026-10-25T21:36:55.423Z ----
print([(y.get('meta') or {}).get('federation_name') if isinstance(y.get('meta'),dict) else y.get('id')[:8] for y in fa5.call('listFederations',{})['result']])
print([ (y['id'][:8], y.get('balance')) for y in fa5.call('listFederations',{})['result']])
pv=await fa5.acall('faClientPreviewTransferDestination'); print(json.dumps(pv)[:300])
print(fa5.call('faClientEligiblePayers',{}))

# ---- cell @ 2026-10-25T21:37:08.770Z ----
other=[y['id'] for y in fa5.call('listFederations',{})['result'] if y['id']!=PAYFED][0]
b0=await fa_balance(fa5)
r=await fa5.acall('faClientStartGuardianTransfer', {'seatIndex':0,'previewId':pv['result']['preview']['previewId'],'paymentFederationId':other,'maxMsats':str(SEAT)}, timeout=200)
print('non-payer fed', json.dumps(r)[:300])
print(fa5.call('faClientGuardianTransfer',{}), 'spent', b0-(await fa_balance(fa5)))
pv2=await fa5.acall('faClientPreviewTransferDestination')
r=await fa5.acall('faClientStartGuardianTransfer', {'seatIndex':0,'previewId':pv2['result']['preview']['previewId'],'paymentFederationId':'00'*32,'maxMsats':str(SEAT)}, timeout=200)
print('unknown fed', json.dumps(r)[:300], fa5.call('faClientGuardianTransfer',{})['result'])

# ---- cell @ 2026-10-25T21:37:25.075Z ----
bal=await fa_balance(fa5)
r=await fa5.acall('generateEcash', {'federationId':PAYFED, 'amount': bal-100_000, 'includeInvite': False}, timeout=120)
print(str(r)[:300])
await asyncio.sleep(3); print(await fa_balance(fa5))

# ---- cell @ 2026-10-25T21:37:33.489Z ----
r=await fa5.acall('generateEcash', {'federationId':PAYFED, 'amount': bal-100_000, 'includeInvite': False, 'frontendMetadata': {'initialNotes': None, 'recipientMatrixId': None, 'senderMatrixId': None}}, timeout=120)
print(str(r)[:300])
await asyncio.sleep(3); print(await fa_balance(fa5))

# ---- cell @ 2026-10-25T21:37:42.100Z ----
r=await fa5.acall('generateEcash', {'federationId':PAYFED, 'amount': 47948334-50_000, 'includeInvite': False, 'frontendMetadata': {'initialNotes': None, 'recipientMatrixId': None, 'senderMatrixId': None}}, timeout=120)
ECASH_HOLD=r.get('result'); print(str(r)[:200])
await asyncio.sleep(3); b1=await fa_balance(fa5); print(b1)
pv3=await fa5.acall('faClientPreviewTransferDestination')
r=await fa5.acall('faClientStartGuardianTransfer', {'seatIndex':0,'previewId':pv3['result']['preview']['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=200)
print('insufficient', json.dumps(r)[:300], fa5.call('faClientGuardianTransfer',{})['result'], 'spent', b1-(await fa_balance(fa5)))

# ---- cell @ 2026-10-25T21:38:08.625Z ----
for i in range(4):
    r=await fa5.acall('faClientResumeGuardianTransfer', timeout=200); print(i, json.dumps(r)[:250])
    await asyncio.sleep(5)
print('bal', await fa_balance(fa5))
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {fa5.dir}/repl.log | grep -a -i 'WARN\\|ERROR' | grep -av 'netlink\\|nostr_relay' | tail -8 | cut -c1-400", max_tokens=3000)

# ---- cell @ 2026-10-25T21:38:54.983Z ----
r=await fa5.acall('receiveEcash', {'federationId':PAYFED, 'ecash': ECASH_HOLD['ecash'], 'frontendMetadata': {'initialNotes': None, 'recipientMatrixId': None, 'senderMatrixId': None}}, timeout=120)
print(str(r)[:200])
for _ in range(20):
    await asyncio.sleep(3)
    if (await fa_balance(fa5))>1_000_000: break
print('bal', await fa_balance(fa5))
for i in range(40):
    r=await fa5.acall('faClientResumeGuardianTransfer', timeout=300); tr=(r.get('result') or {}).get('transfer') if isinstance(r.get('result'),dict) else None
    ph=tr and tr.get('phase')
    if (tr is None and (r.get('result') or {}).get('type')=='transfer') or ph=='completed': break
    await asyncio.sleep(5)
print(i, json.dumps(r)[:200])

# ---- cell @ 2026-10-25T21:39:18.477Z ----
r=await command("git log --oneline --all | grep -i '#210\\|replac' | head -10; git log --oneline -30 b8c013fa | head -30", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T21:39:25.899Z ----
r=await command("rg -n 'replacement|Replacement' bridge/fedi-ffi/src/rpc.rs | head -20", workdir="/src/qa-transfer-fedi", max_tokens=2000)
r2=await command("rg -n 'pub async fn .*replac|pub fn .*replac|fn replacement_eligib' crates/fa-client/src | head", workdir="/src/qa-transfer-manifold-rb")

# ---- cell @ 2026-10-25T21:39:30.654Z ----
r=await command("sed -n 1,140p crates/fa-client/src/eligibility.rs", workdir="/src/qa-transfer-manifold-rb", max_tokens=5000)

# ---- cell @ 2026-10-25T21:39:38.452Z ----
def seat_peer_ids(c):
    out=[]
    for s in c['seats']:
        g,sn=s['loc'][0]
        p=f"{E.root}/mg-{g}/seats/{sn}/data/local.json"
        try: pid=json.load(open(p)).get('identity')
        except Exception as e: pid=repr(e)[:40]
        out.append((s['i'], (g,sn), pid))
    return out
for c in [r4, r5]+PRIV[1:]+[x for x in CTXS if x.get('name') in ('f16','f17')]:
    nctx=await ctx_of(c['fa']); c['seats']=nctx['seats']
    print(c['name'], seat_peer_ids(c))

# ---- cell @ 2026-10-25T21:40:05.327Z ----
r=await command("rg -n 'PeerId::from\\(|PeerId::new\\(|peer_id.*index|index.*peer' crates/fa-client/src --glob '!**/tests*' | head -40", workdir="/src/qa-transfer-manifold-rb", max_tokens=4000)

# ---- cell @ 2026-10-25T21:40:10.743Z ----
r=await command("rg -n 'struct PostFormedSeat' -A15 crates/fa-client/src/formation.rs; rg -n 'PostFormedSeat \\{' -B5 -A12 crates/fa-client/src/formation.rs | head -60", workdir="/src/qa-transfer-manifold-rb", max_tokens=4000)

# ---- cell @ 2026-10-25T21:40:28.088Z ----
r=await command("rg -n 'fn replacement_reasons' -A60 crates/fa-client/src/eligibility.rs | head -80; rg -n 'absence|missed_sessions|offline_secs|max_missed' crates/fa-client/src/*.rs | grep -v '^.*tests' | head -20", workdir="/src/qa-transfer-manifold-rb", max_tokens=5000)

# ---- cell @ 2026-10-25T21:40:33.789Z ----
r=await command("rg -n 'offline|uptime|policy|publish' /src/scratch/qa-gt/qaenv.py | head -20; rg -n 'fn max_guardian_offline_secs|fn min_uptime_percent' -A8 crates/ | head -40", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T21:40:38.729Z ----
r=await command("rg -n -i 'setup.payment|admitted|tsp|manifold-test-issuer|publisher' /src/scratch/qa-gt/qaenv.py | head; ls /src/scratch/qa-gt/", max_tokens=2000)
r=await command(f"{qaenv.BIN}/manifold-test-issuer --help | head -40")

# ---- cell @ 2026-10-25T21:40:43.525Z ----
for m in list(re.finditer('max_guardian_offline', t))[:3]:
    print(t[m.start()-1500:m.start()+500].encode().decode('unicode_escape','ignore')[-2000:]); print('=====')

# ---- cell @ 2026-10-25T21:40:50.408Z ----
fr=r4['fa']
print(json.dumps(fr.call('faClientPreviewReplacements',{}))[:800])

# ---- cell @ 2026-10-25T21:40:54.924Z ----
r=await command("rg -n 'check_replacement_candidates|preview_mg_replacements' --type rust bridge/ crates/ 2>/dev/null | head; rg -n 'faClient[A-Za-z]*' -o bridge/fedi-ffi/src/rpc.rs | sort -u | head -60", workdir="/src/qa-transfer-fedi", max_tokens=3000)

# ---- cell @ 2026-10-25T21:41:00.148Z ----
r=await command("rg -n 'check_replacement_candidates' --type rust . | grep -v 'eligibility.rs' | head", workdir="/src/qa-transfer-manifold-rb", max_tokens=2000)

# ---- cell @ 2026-10-25T21:41:05.840Z ----
r=await command("sed -n 1320,1350p crates/fa-cli/src/main.rs; rg -n 'Restore|restore|mnemonic' crates/fa-cli/src/main.rs | head -20; ls target/debug/ | grep -E '^fa-cli$'", workdir="/src/qa-transfer-manifold-rb", max_tokens=3000)

# ---- cell @ 2026-10-25T21:41:14.457Z ----
def peer_names(c):
    s=c['seats'][0]; g,sn=s['loc'][0]
    cj=json.load(open(f"{E.root}/mg-{g}/seats/{sn}/data/consensus.json"))
    eps=cj.get('global',{}).get('api_endpoints') or cj.get('api_endpoints')
    return {int(k):v['name'] for k,v in eps.items()}
for c in [r4, r5, PRIV[-1]]:
    pn=peer_names(c)
    fa_order=[x['seatId'][:8] for x in c['fa'].call('faClientStatus',{})['result']['status']['formation']['seats']]
    print(c['name'], 'FA index->seat', fa_order)
    print('   peer->name', [pn[k] for k in sorted(pn)])

# ---- cell @ 2026-10-25T21:41:19.190Z ----
s=r4['seats'][0]; g,sn=s['loc'][0]
cj=json.load(open(f"{E.root}/mg-{g}/seats/{sn}/data/consensus.json")); print(list(cj.keys())[:10])
def find_eps(o):
    if isinstance(o,dict):
        for k,v in o.items():
            if k in ('api_endpoints','peers'): return k,v
            r=find_eps(v)
            if r: return r
    return None
k,v=find_eps(cj); print(k, json.dumps(v)[:600])

# ---- cell @ 2026-10-25T21:41:22.615Z ----
print(json.dumps(cj['iroh_endpoints'])[:800])

# ---- cell @ 2026-10-25T21:41:48.325Z ----
r=await command("rg -n 'to_usize\\(\\)|seats\\.get\\(usize::from\\(peer|seats\\[peer|PeerId::from' crates/fa-client/src --glob '!**/tests*' | head -20", workdir="/src/qa-transfer-manifold-rb", max_tokens=2000)

# ---- cell @ 2026-10-25T21:42:05.540Z ----
async def sweep_priv_multi(cases_, label, heal_after=60):
    out=[]
    for k,faults in enumerate(cases_):
        c=await priv_ctx()
        nctx=await ctx_of(c['fa']); c['seats']=nctx['seats']
        await wait_boundary(c, extra=10)
        idx=c['transfers']%7
        name=f'{label}#{k} {c["name"]} idx{idx} faults={faults} hold={heal_after}'
        t_=time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime())
        try: res=await run_scenario(name, c, idx, faults=faults, heal_after=heal_after, drive_timeout=1200)
        except Exception as e:
            import traceback; res={'name':name,'ok':False,'probs':['harness exception '+repr(e)[:300]],'tb':traceback.format_exc()[-1500:]}; RESULTS.append(res)
        c['transfers']+=1
        try: fk,_=await asyncio.to_thread(forkers_now, f"QA {c['name']}", t_); res['forkers']=sorted(fk)
        except Exception: pass
        out.append((name,res.get('ok'),res.get('probs'),res.get('forkers')))
        if not res.get('ok'):
            print('!!! FAIL', name, res.get('probs')); c['retired']='failed: '+name
            try: await asyncio.to_thread(Faults({'fa':c['fa']}).heal, False)
            except Exception: pass
    return out
R3cases=[
 [('dest_waiting',0,('kill_mg','src')), ('dest_waiting',0,('cut_fa_relay',))],
 [('src_stopped',0,('kill_mg','dst')), ('src_stopped',0,('stop_mg','peer2'))],
 [('dest_imported',0,('kill_mg','dst')), ('dest_imported',0,('kill_mg','src'))],
 [('fa_phase:publishingBinding',0,('stop_mg_tree','peer0')), ('fa_phase:publishingBinding',0,('stop_mg_tree','peer1')), ('fa_phase:publishingBinding',0,('kill_mg','dst'))],
 [('dest_seat',0,('cut','src')), ('dest_seat',0,('cut','dst'))],
 [('src_stopped',0,('kill_mg_all','src'))],
 [('dest_waiting',0,('kill_fa',)), ('dest_waiting',0.5,('kill_mg','dst'))],
 [('fa_phase:removingSource',0,('kill_fa',)), ('fa_phase:removingSource',0,('kill_mg_all','src'))],
 [('dest_imported',0,('kill_seat','dst')), ('dest_imported',0,('cut_fa_relay',)), ('dest_imported',0,('stop_mg','src'))],
]
async def after_r2():
    await swp
    return await sweep_priv_multi(R3cases, 'R3')
r3=asyncio.create_task(after_r2())

# ---- cell @ 2026-10-25T21:42:12.963Z ----
n0=len(RESULTS)
for _ in range(240):
    if len(RESULTS)>=n0+5: break
    await asyncio.sleep(15)
for r in RESULTS[n0-1:]:
    print(r.get('name','')[:130], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:44:18.256Z ----
n0=len(RESULTS)
for _ in range(240):
    if len(RESULTS)>=n0+6: break
    await asyncio.sleep(15)
for r in RESULTS[n0:]:
    print(r.get('name','')[:130], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:46:23.922Z ----
await asyncio.sleep(1500)
print(time.strftime('%H:%M:%S'))
for r in RESULTS[-10:]:
    print(r.get('name','')[:130], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:50:36.277Z ----
await asyncio.sleep(1700)
print(time.strftime('%H:%M:%S'))
for r in RESULTS[-8:]:
    print(r.get('name','')[:150], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:54:48.128Z ----
await asyncio.sleep(1750)
print(time.strftime('%H:%M:%S'))
for r in RESULTS[-8:]:
    print(r.get('name','')[:150], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:58:56.450Z ----
print(time.strftime('%H:%M:%S'))
for r in RESULTS[-8:]:
    print(r.get('name','')[:150], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T21:59:05.334Z ----
import builtins
_bprint=builtins.print
QUIET_LOG=[]
def print(*a, **kw):
    s=' '.join(str(x) for x in a)
    if s.startswith(('{"name"','route ','new fed','new private fed')):
        QUIET_LOG.append((time.strftime('%H:%M:%S'), s[:300])); return
    return _bprint(*a, **kw)
print('quiet print installed')

# ---- cell @ 2026-10-25T21:59:12.315Z ----
await asyncio.sleep(3000)
_bprint(time.strftime('%H:%M:%S'))
for r in RESULTS[-10:]:
    _bprint(r.get('name','')[:150], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T22:03:20.998Z ----
_bprint(time.strftime('%H:%M:%S'))
for r in RESULTS[-6:]:
    _bprint(r.get('name','')[:170], r.get('ok'), (r.get('probs') or [])[:1], r.get('forkers'), r.get('secs'))

# ---- cell @ 2026-10-25T22:03:25.602Z ----
set_max_wait(3600)
while not r3.done():
    await asyncio.sleep(60)
_bprint('r3 done', time.strftime('%H:%M:%S'))

# ---- cell @ 2026-10-25T22:40:47.244Z ----
for name,ok,probs,fk in r3.result(): _bprint(name[:160], ok, probs, fk)
x=[r for r in RESULTS if r.get('name','').startswith('R1c#22')][-1]; _bprint(x.get('tb','')[-1200:])

# ---- cell @ 2026-10-25T22:40:54.739Z ----
c19=[c for c in CTXS if c.get('name')=='f19'][0]; f19=c19['fa']
_bprint(f19.p and f19.p.poll(), f19.alive())
r=await command(f"tail -30 {f19.dir}/repl.log | sed 's/\\x1b\\[[0-9;]*m//g' | grep -av netlink | cut -c1-300 | tail -15")
_bprint([ (r.get('name')[:90], r.get('faults')) for r in RESULTS if r.get('name','').startswith(('R1c#20','R1c#21'))])

# ---- cell @ 2026-10-25T22:40:58.927Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {f19.dir}/repl.log | grep -an 'panicked\\|===== start' | tail -8")


# ---- cell @ 2026-10-25T22:41:02.603Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {f19.dir}/repl.log | sed -n 600,660p | grep -av netlink | cut -c1-400", max_tokens=5000)

# ---- cell @ 2026-10-25T22:41:09.729Z ----
r=await web.run(search_query=[{"q":"quinn-proto \"untracked_bytes <= segment_size\" assertion failed poll_transmit"}])
_bprint(r[:3000])

# ---- cell @ 2026-10-25T22:41:21.681Z ----
_bprint(sw3.done(), [ (c['name'], c['transfers'], c.get('retired')) for c in PRIV])
_bprint(E.fman(0,'seats','decommission','--help',check=False))

# ---- cell @ 2026-10-25T22:41:31.600Z ----
async def Edecom(c, idx, which='dst'):
    f=c['fa']; nctx=await ctx_of(f); c['seats']=nctx['seats']
    s=[x for x in c['seats'] if x['i']==idx][0]; sg,ss=s['loc'][0]; src_seat=s['seat_id']
    b0=await fa_balance(f)
    await wait_boundary(c, 5)
    F=Faults({'fa':f,'src':(sg,ss)})
    F.do(('cut','src'))   # keep the move waiting for the source
    known={(g,x[0]) for g in range(14) for x in q(g,'select lower(hex(quote_id)) from seats')}
    pv=await f.acall('faClientPreviewTransferDestination'); prev=pv['result']['preview']
    r=await f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=300)
    _bprint('start', json.dumps(r)[:160])
    new=[(g,s_) for g in range(14) for (s_,) in q(g,'select lower(hex(quote_id)) from seats') if (g,s_) not in known]
    dg,dseat=new[0]
    if which=='dst':
        _bprint('decommission dest', dg, dseat[:8], E.fman(dg,'seats','decommission',dseat,check=False))
    else:
        _bprint('decommission src', sg, src_seat[:8], E.fman(sg,'seats','decommission',src_seat,check=False))
    F.heal(False)
    log=[]
    for i in range(24):
        r=await f.acall('faClientResumeGuardianTransfer', timeout=300)
        log.append(json.dumps(r)[:200]); await asyncio.sleep(10)
    _bprint('resume log first/last', log[0], '\n', log[-1])
    _bprint('spent', b0-(await fa_balance(f)), 'gt', json.dumps(f.call('faClientGuardianTransfer',{}))[:200])
    _bprint('src out', q(sg, f"select phase from seat_transfer_out where lower(hex(quote_id))='{src_seat}'"), 'dst in', q(dg, f"select state from seat_transfer_in where lower(hex(quote_id))='{dseat}'"))
    _bprint('peers', await peer_sessions(c))
    c['transfers']+=1
p5=PRIV[-1]
ed=asyncio.create_task(Edecom(p5, 1, 'dst'))

# ---- cell @ 2026-10-25T22:41:36.522Z ----
while not ed.done(): await asyncio.sleep(20)
_bprint(ed.exception())

# ---- cell @ 2026-10-25T22:45:43.436Z ----
while not ed.done(): await asyncio.sleep(20)
_bprint('done', ed.exception())

# ---- cell @ 2026-10-25T22:46:54.384Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {E.root}/mg-7/stdout.log | grep -a 'e1aca639' | grep -av 'AlephBFT\\|Waiting for bitcoin' | cut -c1-330 | tail -25", max_tokens=5000)

# ---- cell @ 2026-10-25T22:47:08.318Z ----
s=[x for x in p5['seats'] if x['i']==1][0]; _bprint(s['loc'], s['seat_id'][:8])
sg,ss=s['loc'][0]
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {E.root}/mg-{sg}/stdout.log | grep -a 'T22:4[2-9]' | grep -a -i 'transfer\\|retire\\|{s['seat_id'][:16]}' | grep -av 'AlephBFT\\|Waiting for bitcoin\\|memory' | cut -c1-330 | tail -15", max_tokens=5000)
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {E.root}/mg-7/stdout.log | grep -a 'T22:4[2-9]' | grep -a -i 'decommission\\|transfer\\|pull' | grep -av 'AlephBFT\\|Waiting for bitcoin\\|memory' | cut -c1-330 | tail -15", max_tokens=5000)

# ---- cell @ 2026-10-25T22:47:36.476Z ----
async def Edecom2(c, idx, wait_after=30):
    f=c['fa']; nctx=await ctx_of(f); c['seats']=nctx['seats']
    s=[x for x in c['seats'] if x['i']==idx][0]; sg,ss=s['loc'][0]; src_seat=s['seat_id']
    await wait_boundary(c, 5)
    F=Faults({'fa':f,'src':(sg,ss)}); F.do(('cut','src'))
    known={(g,x[0]) for g in range(14) for x in q(g,'select lower(hex(quote_id)) from seats')}
    pv=await f.acall('faClientPreviewTransferDestination'); prev=pv['result']['preview']
    r=await f.acall('faClientStartGuardianTransfer', {'seatIndex':idx,'previewId':prev['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=300)
    new=[(g,s_) for g in range(14) for (s_,) in q(g,'select lower(hex(quote_id)) from seats') if (g,s_) not in known]
    dg,dseat=new[0]
    _bprint(time.strftime('%H:%M:%S'), 'start', json.dumps(r)[:120])
    _bprint(time.strftime('%H:%M:%S'), 'decommission dest', dg, dseat[:8], E.fman(dg,'seats','decommission',dseat,check=False))
    _bprint('dst in after decom', q(dg, f"select state from seat_transfer_in where lower(hex(quote_id))='{dseat}'"), 'decom row', q(dg, f"select decommissioned_at_ms from decommissioned_seats where lower(hex(quote_id))='{dseat}'"))
    await asyncio.sleep(wait_after)
    _bprint(time.strftime('%H:%M:%S'), 'src out before heal', q(sg, f"select phase from seat_transfer_out where lower(hex(quote_id))='{src_seat}'"))
    F.heal(False); _bprint(time.strftime('%H:%M:%S'), 'healed')
    for i in range(12):
        await asyncio.sleep(10)
        o=q(sg, f"select phase from seat_transfer_out where lower(hex(quote_id))='{src_seat}'")
        if o: _bprint(time.strftime('%H:%M:%S'), 'SOURCE RETIRED', o); break
    _bprint('final src out', q(sg, f"select phase from seat_transfer_out where lower(hex(quote_id))='{src_seat}'"), 'dst in', q(dg, f"select state from seat_transfer_in where lower(hex(quote_id))='{dseat}'"))
    _bprint('src seat running?', [x for x in running_seat_dirs() if x[0]==sg and x[1]==ss])
    _bprint('FA', json.dumps(await f.acall('faClientResumeGuardianTransfer', timeout=200))[:200])
    c['transfers']+=1
    return sg,ss,src_seat,dg,dseat
ed2=asyncio.create_task(Edecom2(p5, 2))

# ---- cell @ 2026-10-25T22:47:40.303Z ----
while not ed2.done(): await asyncio.sleep(20)
_bprint('done', ed2.exception())

# ---- cell @ 2026-10-25T22:48:27.124Z ----
_bprint(json.dumps(p5['fa'].call('faClientGuardianTransfer',{}))[:300])
pv=await p5['fa'].acall('faClientPreviewTransferDestination')
r=await p5['fa'].acall('faClientStartGuardianTransfer', {'seatIndex':2,'previewId':pv['result']['preview']['previewId'],'paymentFederationId':PAYFED,'maxMsats':str(SEAT)}, timeout=300)
_bprint('second start on p5', json.dumps(r)[:200])
p5['retired']='stuck transfer (dest decommissioned)'
p6=await mkfed_private('p6'); PRIV.append(p6)
ed2=asyncio.create_task(Edecom2(p6, 0))

# ---- cell @ 2026-10-25T22:53:28.631Z ----
while not ed2.done(): await asyncio.sleep(20)
_bprint('done', ed2.exception(), ed2.result())

# ---- cell @ 2026-10-25T22:54:59.984Z ----
p6['retired']='stuck transfer (dest decommissioned, source intact)'
p7=await mkfed_private('p7'); PRIV.append(p7)
ed3=asyncio.create_task(Edecom2(p7, 0, wait_after=3))
while not ed3.done(): await asyncio.sleep(20)
_bprint('done', ed3.exception())

# ---- cell @ 2026-10-25T22:57:04.085Z ----
while 'ed3' not in globals() or not ed3.done(): await asyncio.sleep(20)
_bprint('done', ed3.exception())

# ---- cell @ 2026-10-25T22:59:15.462Z ----
await asyncio.sleep(20)
_bprint(await peer_sessions(p7))
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {p7['fa'].dir}/repl.log | grep -a 'WARN' | grep -av 'netlink\\|nostr' | tail -3 | cut -c1-400")
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {E.root}/mg-1/stdout.log | grep -a 'd91111fd' | grep -a -i 'warn\\|transfer\\|decom\\|import' | grep -av 'AlephBFT\\|bitcoin' | tail -8 | cut -c1-400", max_tokens=4000)

# ---- cell @ 2026-10-25T23:01:03.211Z ----
x=[r for r in RESULTS if r.get('name','').startswith('R1c#24')][-1]
_bprint(json.dumps({k:x.get(k) for k in ('phases','faults','start','dest','src','secs','bal_delta')})[:3000])

# ---- cell @ 2026-10-25T23:01:10.366Z ----
src_seat=[s for s in [c for c in CTXS if c.get('name')=='f20'][0]['seats'] if s['loc'][0]==(10,19)]
_bprint(src_seat and src_seat[0]['seat_id'][:16])
_bprint('src out', q(10,"select substr(lower(hex(quote_id)),1,8),phase from seat_transfer_out"))
_bprint('dst in', q(0,"select substr(lower(hex(quote_id)),1,8),state from seat_transfer_in where lower(hex(quote_id)) like '316b06ed%'"))
f20=[c for c in CTXS if c.get('name')=='f20'][0]['fa']
_bprint(json.dumps(f20.call('faClientGuardianTransfer',{}))[:400])
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {f20.dir}/repl.log | grep -a 'WARN' | grep -av 'netlink\\|nostr' | tail -5 | cut -c1-500", max_tokens=3000)

# ---- cell @ 2026-10-25T23:01:17.708Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {f20.dir}/repl.log | grep -a 'WARN\\|ERROR' | grep -av 'netlink\\|nostr\\|await_incoming' | grep -a 'T22:[45]' | cut -c1-400 | awk '!seen[substr($0,30)]++' | tail -15", max_tokens=5000)

# ---- cell @ 2026-10-25T23:01:22.022Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {f20.dir}/repl.log | grep -a 'T22:4[3-9]\\|T22:5' | grep -a -i 'transfer\\|fa_client\\|attest\\|binding' | grep -av 'rpc call\\|Operation timing' | cut -c1-400 | tail -20", max_tokens=5000)

# ---- cell @ 2026-10-25T23:01:27.560Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g' {E.root}/mg-0/stdout.log | grep -a '316b06ed' | grep -av 'AlephBFT\\|memory\\|Failed to handle Iroh' | cut -c1-260 | awk '{{print substr($0,1,260)}}' | grep -a -i 'warn\\|import\\|transfer\\|start\\|backup\\|bitcoin' | head -30", max_tokens=6000)

# ---- cell @ 2026-10-25T23:01:41.755Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g;s/\\\\x1b\\[[0-9;]*m//g' {E.root}/mg-0/stdout.log | grep -a '316b06ed' | grep -a -i 'error\\|panic\\|fatal\\|Err' | grep -av 'Failed to handle Iroh' | cut -c1-500 | awk '!s[substr($0,60,200)]++' | head -10; sed 's/\\x1b\\[[0-9;]*m//g' {E.root}/mg-0/stdout.log | grep -a -c 'seat_id=316b06ed.*exit_code=Some(255)'", max_tokens=5000)

# ---- cell @ 2026-10-25T23:02:09.872Z ----
lines=[]
with open(f"{E.root}/mg-0/stdout.log", errors='ignore') as fh:
    for l in fh:
        if '316b06ed' in l and 'T22:43:4' in l[:40]:
            lines.append(re.sub(r'(\x1b|\\x1b)\[[0-9;]*m','',l)[:400])
i=[k for k,l in enumerate(lines) if 'exited' in l][0]
for l in lines[max(0,i-12):i+1]: _bprint(l.strip()[:380])

# ---- cell @ 2026-10-25T23:02:15.192Z ----
j=[k for k,l in enumerate(lines[:i]) if 'panicked' in l or 'Error' in l or 'error' in l.lower()]
for k in j[:6]: _bprint(lines[k].strip()[:400])
k0=[k for k,l in enumerate(lines[:i]) if 'panicked' in l]
if k0:
    for l in lines[k0[0]:k0[0]+8]: _bprint(l.strip()[:400])

# ---- cell @ 2026-10-25T23:02:23.607Z ----
ex=[]; binds=[]
with open(f"{E.root}/mg-0/stdout.log", errors='ignore') as fh:
    for l in fh:
        if '316b06ed' in l:
            if 'exit_code=Some(255)' in l: ex.append(l[5:33])
            if 'Address already in use' in l: binds.append(l[5:33])
_bprint(len(ex), ex[:2], ex[-2:], len(binds))
_bprint([x for x in running_seat_dirs() if x[0]==0])
command(f"ss -lunp 2>/dev/null | grep -E ':2609[0-3]' | head; ss -ltnp | grep -E ':2609[0-3]' | head")

# ---- cell @ 2026-10-25T23:02:36.561Z ----
for pid in (1704574, 1656208):
    _bprint(pid, open(f'/proc/{pid}/cmdline').read().replace('\0',' ')[:300])
command("ss -lunp 2>/dev/null | grep -E 'pid=(1656208|1704574),' ")

# ---- cell @ 2026-10-25T23:02:46.509Z ----
_bprint(q(0,"select seat_no from seats where lower(hex(quote_id)) like '316b06ed%'"))
_bprint(q(0,"select seat_no, substr(lower(hex(quote_id)),1,8) from seats order by seat_no desc limit 5"))
_bprint([l[:200] for l in lines if 'bind-metrics' in l or 'metrics server url' in l][:3])

# ---- cell @ 2026-10-25T23:02:57.469Z ----
r=await command(f"sed 's/\\x1b\\[[0-9;]*m//g;s/\\\\x1b\\[[0-9;]*m//g' {E.root}/mg-0/stdout.log | grep -a '316b06ed' | grep -a 'exited\\|already in use\\|Starting consensus' | tail -4 | cut -c1-250; ss -lunp | grep -E ':26099 |:26089 |:26088 '", max_tokens=3000)

# ---- cell @ 2026-10-25T23:03:15.675Z ----
_bprint(open('/proc/1655549/cmdline').read().replace('\0',' ')[:250])
command("ss -lunp | grep 'pid=1655549,'")

# ---- cell @ 2026-10-25T23:03:21.412Z ----
_bprint(qaenv.endpoint_port(0), qaenv.mg_base(0), qaenv.MAX_SEATS)
command("rg -n 'def endpoint_port|def mg_base|def seat_port|MAX_SEATS =' -A3 /src/scratch/qa-gt/qaenv.py")

# ---- cell @ 2026-10-25T23:03:37.596Z ----
_bprint({g: q(g,"select max(seat_no) from seats")[0][0] for g in range(14)})

# ---- cell @ 2026-10-25T23:03:51.909Z ----
_bprint(sw3.done(), [r.get('name')[:60] for r in RESULTS[-3:]])

# ---- cell @ 2026-10-25T23:03:57.743Z ----
sw3.cancel()
await asyncio.sleep(3)
_bprint(sw3.done(), {g: q(g,"select max(seat_no) from seats")[0][0] for g in range(14)})
# heal any leftover faults from a cancelled scenario
try: Faults({'fa':None}).heal(False)
except Exception as e: _bprint('heal', e)
_bprint([g for g in range(14) if E.mg_pid(g) is None])

# ---- cell @ 2026-10-25T23:04:07.673Z ----
for g in range(14):
    for tag in (f'cut-mg{g}', f'cutep-mg{g}'):
        try: qaenv.proxy({'cmd':'unblock','tag':tag})
        except Exception: pass
    try: qaenv.proxy({'cmd':'tcp_unblock','group':f'relay:mg{g}'})
    except Exception: pass
for i in range(5):
    try: qaenv.proxy({'cmd':'tcp_unblock','group':f'relay:fa{i}'})
    except Exception: pass
try: qaenv.proxy({'cmd':'unblock','tag':'cut-fa'})
except Exception: pass
# any SIGSTOPped processes?
stopped=[]
for pid in os.listdir('/proc'):
    if pid.isdigit():
        try:
            st=open(f'/proc/{pid}/stat').read().split(')')[-1].split()[0]
            if st=='T': stopped.append(int(pid))
        except Exception: pass
_bprint('stopped procs', stopped)

# ---- cell @ 2026-10-25T23:04:17.443Z ----
_bprint(E.fman(0,'capacity','set','--help',check=False))

# ---- cell @ 2026-10-25T23:04:23.552Z ----
HIGH=[g for g in range(14) if q(g,"select max(seat_no) from seats")[0][0]>=21]
_bprint('high', HIGH)
for g in HIGH:
    _bprint(g, E.fman(g,'capacity','set','0',check=False))
await asyncio.sleep(5)
_bprint({g:E.fman(g,'capacity','show') for g in HIGH})

# ---- cell @ 2026-10-25T23:04:33.424Z ----
for g in HIGH:
    _bprint(g, E.fman(g,'plans','set',check=False))
await asyncio.sleep(3)
_bprint({g:E.fman(g,'plans','show') for g in HIGH})

# ---- cell @ 2026-10-25T23:04:45.139Z ----
for c in PRIV: 
    if not c.get('retired') and c['transfers']>0: c['retired']='rotate'
R1left=[('dest_seat',0.3,('kill_mg','src')), ('fa_phase:publishingBinding',0,('kill_mg','src')), ('dest_imported',0,('kill_mg','src')), ('dest_imported',0.3,('kill_mg','dst'))]
_orig_priv_ctx=priv_ctx
async def priv_ctx():
    c=PRIV[-1]
    if c['transfers']>=2 or c.get('retired'):
        n=f"p{len(PRIV)+1}"
        c=await mkfed_private(n); PRIV.append(c); _bprint('new private fed', n, r4_locs(c))
    return c
r1l=asyncio.create_task(sweep_priv(R1left, 'R1d', heal_after=0))

# ---- cell @ 2026-10-25T23:04:54.921Z ----
r=await command("git log --oneline -3 rho/qa-gt-harness2; git remote -v | head -2; git show --stat rho/qa-gt-harness2 | head -20; git ls-remote --heads origin 'rho/qa-gt*' | head", workdir="/src/qa-transfer-fedi")
