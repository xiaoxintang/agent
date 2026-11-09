#![cfg(unix)]

use std::{io::Write, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

#[tokio::test]
async fn sigint_and_sigterm_finish_through_the_shutdown_handler() {
    for signal in ["-INT", "-TERM"] {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "shutdown_child", "--ignored", "--nocapture"])
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(line) = output.next_line().await.unwrap() {
                if line == "READY" {
                    return;
                }
            }
            panic!("child exited before registering signals");
        })
        .await
        .unwrap();
        assert!(
            Command::new("kill")
                .args([signal, &child.id().unwrap().to_string()])
                .status()
                .await
                .unwrap()
                .success()
        );
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            status.success(),
            "shutdown was bypassed for {signal}: {status}"
        );
    }
}

#[tokio::test]
#[ignore = "subprocess helper for signal delivery"]
async fn shutdown_child() {
    let signal = panel_common::shutdown::signal().unwrap();
    println!("READY");
    std::io::stdout().flush().unwrap();
    signal.await.unwrap();
}
