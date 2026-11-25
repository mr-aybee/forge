//! Runs the adopt recipe against a real RustFS container on this host's
//! Docker. Ignored by default: `FORGE_SMOKE_CONTAINER=<name>
//! FORGE_SMOKE_PORT=<s3 port> cargo test -p forge-catalog --test objstore_smoke -- --ignored`.
#![allow(clippy::unwrap_used)] // reason: test

use forge_catalog::storage::{Runtime, StoreOp, StoreSpec, adopted_env, take_credential};
use forge_core::storage_settings::{DriveVolume, RustfsSettings};

#[test]
#[ignore = "needs Docker and a running RustFS container"]
fn adopt_reads_the_credential_of_a_real_container() {
    let (Ok(name), Ok(port)) = (std::env::var("FORGE_SMOKE_CONTAINER"), std::env::var("FORGE_SMOKE_PORT")) else {
        return;
    };

    let spec = StoreSpec {
        provider: "rustfs".into(),
        slug: "smoke".into(),
        version: "latest".into(),
        runtime: Runtime::Docker,
        s3_port: port.parse().unwrap(),
        console_port: None,
        data_paths: vec!["/data".into()],
        peers: vec![],
        require_mount: false,
        adopted: true,
        owner: String::new(),
        region: String::new(),
        docker: Some(RustfsSettings {
            container_name: name,
            adopted: true,
            volumes: vec![DriveVolume::Named {
                name: "smoke-rustfs-data".into(),
                driver: String::new(),
                options: Default::default(),
            }],
            ..RustfsSettings::default()
        }),
    };
    let op = StoreOp::Adopt { spec };
    op.validate().unwrap();
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(op.linux_script())
        .env_clear()
        .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .envs(op.env())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let (cred, rest) = take_credential(&text);
    let cred = cred.unwrap_or_else(|| panic!("no credential: {text}"));
    println!("adopted: access key {} (secret withheld)\n{rest}", cred.access_key);
    println!("adopted env: {:?}", adopted_env(&rest));
    assert!(out.status.success(), "{rest}");
    assert!(!rest.contains(&cred.secret_key));
}
