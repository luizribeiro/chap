#![cfg(feature = "microsandbox")]

use std::time::{SystemTime, UNIX_EPOCH};

use chap_vm::host::{
    Backend as MicrosandboxBackend, MountSpec, ResolvedImage, SecretSource, SecretSpec, VmBackend,
    VmCallSettings, VmCommand, VmConfig, VmIdentity, VmPrincipal, VmSettings,
};
use chap_vm::vm::Egress;

const HOST_CONTENTS: &[u8] = b"hello from the host bind mount\n";
const GUEST_CONTENTS: &[u8] = b"written through the guest filesystem";
const HOST_SECRET_VAR: &str = "CHAP_MICROSANDBOX_TEST_SECRET";
const DUMMY_SECRET: &str = "chap-microsandbox-dummy-secret";

#[tokio::test]
#[ignore = "boots a microsandbox vm and pulls docker.io/library/alpine:3.20; run with -- --ignored"]
async fn boots_alpine_and_exercises_the_backend_contract() {
    let host_dir = tempfile::tempdir().unwrap();
    std::fs::write(host_dir.path().join("known.txt"), HOST_CONTENTS).unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let identity = VmIdentity {
        installation_id: format!("chap-microsandbox-e2e-{nonce}"),
        session_epoch: 1,
        principal: VmPrincipal::Plugin("microsandbox-e2e".parse().unwrap()),
        logical_name: "alpine".into(),
    };
    let config = VmConfig {
        image: ResolvedImage {
            registry: "docker.io".into(),
            repository: "library/alpine".into(),
            tag: Some("3.20".into()),
            digest: None,
        },
        mounts: vec![MountSpec {
            host: host_dir
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            guest: "/mnt/project".into(),
            readonly: true,
        }],
        egress: vec![],
        env: vec![],
        secrets: vec![],
        cpus: 1,
        memory_mb: 256,
        max_lifetime_ms: 60_000,
        idle_timeout_ms: 30_000,
        config_hash: "microsandbox-e2e-v1".into(),
    };
    let settings = VmSettings {
        calls: VmCallSettings {
            exec_timeout_ceiling_ms: 5_000,
            ..VmCallSettings::default()
        },
        ..VmSettings::default()
    };
    let backend = MicrosandboxBackend::for_session(&settings).unwrap();

    println!(
        "microsandbox boot: creating {} as {:?}",
        identity.physical_label(),
        identity.principal
    );
    let vm = backend.create(&identity, &config).await.unwrap();

    let uname = backend
        .exec(&vm, command(&["uname", "-a"], None))
        .await
        .unwrap();
    println!(
        "microsandbox boot: uname exit={:?} stdout={:?}",
        uname.exit_code,
        String::from_utf8_lossy(&uname.stdout)
    );
    assert_eq!(uname.exit_code, Some(0));
    assert!(String::from_utf8_lossy(&uname.stdout).contains("Linux"));

    assert_eq!(
        backend
            .read_file(&vm, "/mnt/project/known.txt", 1_024)
            .await
            .unwrap(),
        HOST_CONTENTS
    );
    let readonly_write = backend
        .exec(
            &vm,
            command(
                &["sh", "-c", "echo denied > /mnt/project/should-not-exist"],
                None,
            ),
        )
        .await
        .unwrap();
    println!(
        "microsandbox boot: readonly write exit={:?} stderr={:?}",
        readonly_write.exit_code,
        String::from_utf8_lossy(&readonly_write.stderr)
    );
    assert!(matches!(readonly_write.exit_code, Some(code) if code != 0));
    assert!(!host_dir.path().join("should-not-exist").exists());

    backend
        .write_file(&vm, "/tmp/round-trip.txt", GUEST_CONTENTS)
        .await
        .unwrap();
    assert_eq!(
        backend
            .read_file(&vm, "/tmp/round-trip.txt", 1_024)
            .await
            .unwrap(),
        GUEST_CONTENTS
    );
    let timed_out = backend
        .exec(
            &vm,
            command(
                &[
                    "sh",
                    "-c",
                    "printf partial-stdout; printf partial-stderr >&2; sleep 30",
                ],
                Some(250),
            ),
        )
        .await
        .unwrap();
    assert_eq!(timed_out.exit_code, None);
    assert_eq!(timed_out.stdout, b"partial-stdout");
    assert_eq!(timed_out.stderr, b"partial-stderr");
    println!("microsandbox boot: timeout returned partial output");

    assert_eq!(backend.get(&identity).await.unwrap(), Some(vm.clone()));
    assert_eq!(
        backend.owner_of(&vm).await.unwrap(),
        Some(identity.principal.clone())
    );
    backend
        .shutdown(&identity.installation_id, identity.session_epoch)
        .await
        .unwrap();
    assert_eq!(backend.get(&identity).await.unwrap(), None);
    println!("microsandbox boot: reap removed the sandbox");

    let network_backend = MicrosandboxBackend::for_session(&VmSettings {
        calls: VmCallSettings {
            exec_timeout_ceiling_ms: 60_000,
            ..VmCallSettings::default()
        },
        ..VmSettings::default()
    })
    .unwrap();
    let dns_identity = VmIdentity {
        logical_name: "network-with-dns".into(),
        ..identity.clone()
    };
    let mut dns_config = config.clone();
    dns_config.mounts.clear();
    dns_config.egress = egress(&["0.0.0.0/0:443", "0.0.0.0/0:80", "0.0.0.0/0:53"]);
    dns_config.secrets = vec![SecretSpec {
        env: "GUEST".into(),
        source: SecretSource::HostEnv(HOST_SECRET_VAR.into()),
        hosts: vec!["example.com".into()],
    }];
    dns_config.config_hash = "microsandbox-e2e-dns-v1".into();
    dns_config.max_lifetime_ms = 120_000;
    unsafe { std::env::set_var(HOST_SECRET_VAR, DUMMY_SECRET) };
    let dns_vm = network_backend
        .create(&dns_identity, &dns_config)
        .await
        .unwrap();
    let guest_secret = network_backend
        .exec(&dns_vm, command(&["printenv", "GUEST"], None))
        .await
        .unwrap();
    assert_eq!(guest_secret.stdout, b"$MSB_GUEST\n");
    assert!(
        !guest_secret
            .stdout
            .windows(DUMMY_SECRET.len())
            .any(|value| value == DUMMY_SECRET.as_bytes())
    );
    let guest_env = network_backend
        .exec(&dns_vm, command(&["env"], None))
        .await
        .unwrap();
    assert!(
        !guest_env
            .stdout
            .windows(DUMMY_SECRET.len())
            .any(|value| value == DUMMY_SECRET.as_bytes())
    );
    let apk_with_dns = network_backend
        .exec(
            &dns_vm,
            command(&["apk", "add", "--no-cache", "curl"], Some(60_000)),
        )
        .await;
    let curl = network_backend
        .exec(
            &dns_vm,
            command(&["curl", "-sSI", "https://example.com"], Some(60_000)),
        )
        .await;

    let no_dns_identity = VmIdentity {
        logical_name: "network-without-dns".into(),
        ..identity.clone()
    };
    let mut no_dns_config = dns_config.clone();
    no_dns_config.egress = egress(&["0.0.0.0/0:443", "0.0.0.0/0:80"]);
    no_dns_config.config_hash = "microsandbox-e2e-no-dns-v1".into();
    let no_dns_vm = network_backend
        .create(&no_dns_identity, &no_dns_config)
        .await
        .unwrap();
    let apk_without_dns = network_backend
        .exec(
            &no_dns_vm,
            command(&["apk", "add", "--no-cache", "curl"], Some(60_000)),
        )
        .await;

    network_backend
        .shutdown(&identity.installation_id, identity.session_epoch)
        .await
        .unwrap();
    unsafe { std::env::remove_var(HOST_SECRET_VAR) };
    assert_eq!(network_backend.get(&dns_identity).await.unwrap(), None);
    assert_eq!(network_backend.get(&no_dns_identity).await.unwrap(), None);

    let apk_with_dns = apk_with_dns.unwrap();
    println!(
        "microsandbox network: apk with dns exit={:?} stdout={:?} stderr={:?}",
        apk_with_dns.exit_code,
        String::from_utf8_lossy(&apk_with_dns.stdout),
        String::from_utf8_lossy(&apk_with_dns.stderr)
    );
    assert_eq!(apk_with_dns.exit_code, Some(0));

    let curl = curl.unwrap();
    println!(
        "microsandbox network: curl exit={:?} stdout={:?} stderr={:?}",
        curl.exit_code,
        String::from_utf8_lossy(&curl.stdout),
        String::from_utf8_lossy(&curl.stderr)
    );
    assert_eq!(curl.exit_code, Some(0));

    let apk_without_dns = apk_without_dns.unwrap();
    println!(
        "microsandbox network: apk without dns exit={:?} stdout={:?} stderr={:?}",
        apk_without_dns.exit_code,
        String::from_utf8_lossy(&apk_without_dns.stdout),
        String::from_utf8_lossy(&apk_without_dns.stderr)
    );
    assert!(matches!(apk_without_dns.exit_code, Some(code) if code != 0));
    assert!(
        String::from_utf8_lossy(&apk_without_dns.stderr)
            .to_ascii_lowercase()
            .contains("dns")
    );
}

fn command(args: &[&str], timeout_ms: Option<u64>) -> VmCommand {
    VmCommand {
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd: None,
        stdin: None,
        timeout_ms,
    }
}

fn egress(scopes: &[&str]) -> Vec<Egress> {
    scopes.iter().map(|scope| scope.parse().unwrap()).collect()
}
