use anyhow::{Context as _, Result, bail, ensure};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize, Serialize)]
pub struct Envelope {
    pub manifest: String,
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub schema: u32,
    pub channel: String,
    pub version: Version,
    pub minimum_version: Option<Version>,
    pub notes: String,
    pub assets: BTreeMap<String, Asset>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Asset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub kind: PackageKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    Exe,
    Deb,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    pub notes: String,
    pub asset: Asset,
}

#[derive(Clone, Debug)]
pub enum Offer {
    Current,
    Update(Release),
    Manual(Version),
}

pub fn open(bytes: &[u8], keys: &[String]) -> Result<Manifest> {
    ensure!(!keys.is_empty(), "此构建未配置更新公钥，无法验证在线更新");
    let envelope: Envelope = serde_json::from_slice(bytes).context("更新清单格式错误")?;
    let signature = Signature::decode(&envelope.signature).context("更新签名格式错误")?;
    ensure!(
        keys.iter()
            .any(|key| PublicKey::from_base64(key).is_ok_and(|key| key
                .verify(envelope.manifest.as_bytes(), &signature, false)
                .is_ok())),
        "更新清单签名验证失败"
    );
    let manifest: Manifest = serde_json::from_str(&envelope.manifest)?;
    ensure!(manifest.schema == 1, "更新清单版本不兼容，请手动下载安装");
    ensure!(
        manifest.channel == "stable" && manifest.version.pre.is_empty(),
        "不接受非稳定版更新"
    );
    ensure!(
        signature.trusted_comment() == format!("alacrterm-manifest stable {}", manifest.version),
        "更新通道或版本与签名不一致"
    );
    Ok(manifest)
}

impl Manifest {
    pub fn offer(&self, current: &Version, platform: &str) -> Result<Offer> {
        if self.version <= *current {
            return Ok(Offer::Current);
        }
        if self
            .minimum_version
            .as_ref()
            .is_some_and(|minimum| current < minimum)
        {
            return Ok(Offer::Manual(self.version.clone()));
        }
        let Some(asset) = self.assets.get(platform) else {
            bail!("此版本没有 {platform} 安装包");
        };
        let expected = match platform {
            "windows-x64" => PackageKind::Exe,
            "linux-x64" => PackageKind::Deb,
            _ => bail!("不支持的平台 {platform}"),
        };
        ensure!(asset.kind == expected, "安装包类型与平台不一致");
        ensure!(
            asset.size > 0
                && asset.sha256.len() == 64
                && asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "安装包摘要或大小无效"
        );
        let url = reqwest::Url::parse(&asset.url)?;
        ensure!(
            url.scheme() == "https" && url.username().is_empty() && url.password().is_none(),
            "安装包必须使用 HTTPS"
        );
        Ok(Offer::Update(Release {
            version: self.version.clone(),
            notes: self.notes.clone(),
            asset: asset.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture() -> Manifest {
        Manifest {
            schema: 1,
            channel: "stable".into(),
            version: Version::new(0, 2, 0),
            minimum_version: None,
            notes: "更新说明".into(),
            assets: BTreeMap::from([(
                "linux-x64".into(),
                Asset {
                    url: "https://example.com/app.deb".into(),
                    size: 3,
                    sha256: "ab".repeat(32),
                    kind: PackageKind::Deb,
                },
            )]),
        }
    }

    #[test]
    fn signed_manifest_rejects_tampering_unknown_keys_and_channel_mismatch() {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let manifest = serde_json::to_string(&fixture()).unwrap();
        let signature = minisign::sign(
            Some(&pair.pk),
            &pair.sk,
            Cursor::new(manifest.as_bytes()),
            Some("alacrterm-manifest stable 0.2.0"),
            None,
        )
        .unwrap()
        .into_string();
        let bytes = serde_json::to_vec(&Envelope {
            manifest,
            signature,
        })
        .unwrap();
        assert!(open(&bytes, &[pair.pk.to_base64()]).is_ok());
        assert!(open(&bytes, &[]).is_err());
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        assert!(open(&bytes, &[other.pk.to_base64()]).is_err());
        let tampered = String::from_utf8(bytes.clone())
            .unwrap()
            .replace("更新说明", "篡改说明");
        assert!(open(tampered.as_bytes(), &[pair.pk.to_base64()]).is_err());
        let envelope: Envelope = serde_json::from_slice(&bytes).unwrap();
        let signature = minisign::sign(
            Some(&pair.pk),
            &pair.sk,
            Cursor::new(envelope.manifest.as_bytes()),
            Some("alacrterm-manifest beta 0.2.0"),
            None,
        )
        .unwrap()
        .into_string();
        assert!(
            open(
                &serde_json::to_vec(&Envelope {
                    manifest: envelope.manifest,
                    signature
                })
                .unwrap(),
                &[pair.pk.to_base64()]
            )
            .is_err()
        );
    }

    #[test]
    fn versions_platforms_and_minimum_versions_are_enforced() {
        let mut manifest = fixture();
        assert!(matches!(
            manifest.offer(&Version::new(0, 2, 0), "linux-x64").unwrap(),
            Offer::Current
        ));
        assert!(matches!(
            manifest.offer(&Version::new(0, 1, 0), "linux-x64").unwrap(),
            Offer::Update(_)
        ));
        assert!(
            manifest
                .offer(&Version::new(0, 1, 0), "windows-x64")
                .is_err()
        );
        manifest.minimum_version = Some(Version::new(0, 1, 5));
        assert!(matches!(
            manifest.offer(&Version::new(0, 1, 0), "linux-x64").unwrap(),
            Offer::Manual(_)
        ));
    }
}
