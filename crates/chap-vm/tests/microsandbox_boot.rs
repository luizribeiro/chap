#![cfg(feature = "microsandbox")]

use std::time::{SystemTime, UNIX_EPOCH};

use chap_vm::host::{
    Backend as MicrosandboxBackend, MountSpec, ResolvedImage, Subject, VmBackend, VmCommand,
    VmConfig, VmError, VmIdentity, VmSettings,
};

const HOST_CONTENTS: &[u8] = b"hello from the host bind mount\n";
const GUEST_CONTENTS: &[u8] = b"written through the guest filesystem";

#[tokio::test]
async fn boots_alpine_and_exercises_the_backend_contract() {
    if std::env::var("CHAP_MICROSANDBOX_E2E").as_deref() != Ok("1") {
        println!("CHAP_MICROSANDBOX_E2E is not 1; skipping microsandbox boot test");
        return;
    }

    let host_dir = tempfile::tempdir().unwrap();
    std::fs::write(host_dir.path().join("known.txt"), HOST_CONTENTS).unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let identity = VmIdentity {
        installation_id: format!("chap-microsandbox-e2e-{nonce}"),
        session_epoch: 1,
        principal: "microsandbox-e2e".into(),
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
            host: host_dir.path().to_string_lossy().into_owned(),
            guest: "/mnt/project".into(),
            readonly: true,
        }],
        egress: vec![],
        env: vec![],
        cpus: 1,
        memory_mb: 256,
        max_duration_ms: 60_000,
        idle_timeout_ms: 30_000,
        config_hash: "microsandbox-e2e-v1".into(),
    };
    let settings = VmSettings {
        max_exec_ms: 5_000,
        ..VmSettings::default()
    };
    let backend = MicrosandboxBackend::new(&settings);

    println!(
        "microsandbox boot: creating {} as {}",
        identity.physical_label(),
        identity.principal
    );
    let vm = backend.create(&identity, &config).await.unwrap();

    let uname = backend
        .exec(&vm, command(&["uname", "-a"], None))
        .await
        .unwrap();
    println!(
        "microsandbox boot: uname exit={} stdout={:?}",
        uname.exit_code,
        String::from_utf8_lossy(&uname.stdout)
    );
    assert_eq!(uname.exit_code, 0);
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
        "microsandbox boot: readonly write exit={} stderr={:?}",
        readonly_write.exit_code,
        String::from_utf8_lossy(&readonly_write.stderr)
    );
    assert_ne!(readonly_write.exit_code, 0);
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
    assert!(matches!(
        backend
            .exec(&vm, command(&["sleep", "30"], Some(250)))
            .await,
        Err(VmError::TimedOut)
    ));
    println!("microsandbox boot: timeout mapped to VmError::TimedOut");

    assert_eq!(backend.get(&identity).await.unwrap(), Some(vm.clone()));
    assert_eq!(
        backend.owner_of(&vm).await.unwrap(),
        Some(Subject(identity.principal.clone()))
    );
    backend.reap(&identity.installation_id, &[]).await.unwrap();
    assert_eq!(backend.get(&identity).await.unwrap(), None);
    println!("microsandbox boot: reap removed the sandbox");
}

fn command(args: &[&str], timeout_ms: Option<u64>) -> VmCommand {
    VmCommand {
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd: None,
        stdin: None,
        timeout_ms,
    }
}
