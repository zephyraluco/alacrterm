//! SSH X11 authentication and channel bridging to the shared platform display.
use anyhow::{Result, ensure};
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
#[derive(Clone)]
pub(crate) struct Forwarder {
    fake: [u8; 16],
    enabled: Arc<AtomicBool>,
}
impl Forwarder {

    pub(crate) fn session() -> Self {
        Self {
            fake: rand::random(),
            enabled: Arc::new(AtomicBool::new(true)),
        }
    }

    pub(crate) fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
    }

    pub(crate) fn cookie_hex(&self) -> String {
        self.fake.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub(crate) async fn accept(
        &self,
        channel: russh::Channel<russh::client::Msg>,
        reply: russh::client::ChannelOpenHandle,
    ) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        let Ok(forwarder) = platform::wayx::Endpoint::current() else {
            return;
        };
        let Ok(permit) = forwarder.acquire() else {
            return;
        };
        reply.accept().await;
        let fake = self.fake;
        tokio::spawn(async move {
            let _permit = permit;
            let result: Result<()> = async {
                let mut remote = channel.into_stream();
                let prefix = tokio::time::timeout(
                    Duration::from_secs(10),
                    validated_setup(&mut remote, &fake),
                )
                .await??;
                // Authentication must complete before opening any local X connection.
                let mut local = forwarder.local().await?;
                local.write_all(&prefix).await?;
                tokio::select! {
                    result = tokio::io::copy_bidirectional(&mut remote, &mut local) => { result?; }
                    _ = async {
                        loop {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            if !forwarder.is_current() { break; }
                        }
                    } => {}
                }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                log::debug!("X11 通道关闭:{error}");
            }
        });
    }
}

// Adapted from zephyraluco/WayX src/ssh.rs (MIT, see docs/wayx-LICENSE).
async fn validated_setup<R: AsyncRead + Unpin>(stream: &mut R, fake: &[u8; 16]) -> Result<Vec<u8>> {
    let mut header = [0u8; 12];
    stream.read_exact(&mut header).await?;
    ensure!(matches!(header[0], b'l' | b'B'), "Invalid X11 byte order");
    let read = |offset: usize| {
        if header[0] == b'l' {
            u16::from_le_bytes([header[offset], header[offset + 1]])
        } else {
            u16::from_be_bytes([header[offset], header[offset + 1]])
        }
    };
    ensure!(
        read(2) == 11 && read(4) == 0,
        "Unsupported X11 protocol version"
    );
    ensure!(
        read(6) == 18 && read(8) == 16,
        "Only MIT-MAGIC-COOKIE-1 authentication is accepted"
    );
    let mut body = [0; 36];
    stream.read_exact(&mut body).await?;
    ensure!(
        &body[..18] == b"MIT-MAGIC-COOKIE-1",
        "Unsupported X11 authentication protocol"
    );
    let difference = body[20..]
        .iter()
        .zip(fake)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    ensure!(difference == 0, "Invalid X11 forwarding cookie");
    // SSH authenticates each forwarded connection; the local WayX display uses -ac.
    // Strip both authentication fields and their body, preserving byte order/version.
    header[6..10].fill(0);
    Ok(header.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(big: bool, cookie: &[u8; 16]) -> Vec<u8> {
        let mut packet = vec![if big { b'B' } else { b'l' }, 0];
        for value in [11u16, 0, 18, 16, 0] {
            packet.extend_from_slice(&if big {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            });
        }
        packet.extend_from_slice(b"MIT-MAGIC-COOKIE-1\0\0");
        packet.extend_from_slice(cookie);
        packet
    }

    #[tokio::test]
    async fn authenticates_both_byte_orders_and_strips_local_authentication() {
        for big in [false, true] {
            let packet = setup(big, &[7; 16]);
            let result = validated_setup(&mut packet.as_slice(), &[7; 16])
                .await
                .unwrap();
            assert_eq!(&result[..6], &packet[..6]);
            assert_eq!(&result[6..], &[0; 6]);
            assert_eq!(result.len(), 12);
        }
    }

    #[tokio::test]
    async fn stripping_authentication_preserves_following_x11_requests() {
        let mut packet = setup(false, &[7; 16]);
        packet.extend_from_slice(&[127, 0, 1, 0]); // X11 NoOperation request.
        let mut stream = packet.as_slice();
        let header = validated_setup(&mut stream, &[7; 16]).await.unwrap();
        assert_eq!(header, [b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(stream, &[127, 0, 1, 0]);
    }

    #[tokio::test]
    async fn rejects_invalid_cookie_protocol_and_truncated_setup() {
        let valid = setup(false, &[7; 16]);
        let mut invalid_cookie = valid.clone();
        invalid_cookie[47] ^= 1;
        let mut invalid_order = valid.clone();
        invalid_order[0] = b'?';
        let mut invalid_version = valid.clone();
        invalid_version[2] = 10;
        let mut invalid_size = valid.clone();
        invalid_size[7] = 255;
        let mut invalid_name = valid.clone();
        invalid_name[12] = b'?';
        let truncated = valid[..40].to_vec();
        for packet in [
            invalid_cookie,
            invalid_order,
            invalid_version,
            invalid_size,
            invalid_name,
            truncated,
        ] {
            assert!(
                validated_setup(&mut packet.as_slice(), &[7; 16])
                    .await
                    .is_err()
            );
        }
    }

}
