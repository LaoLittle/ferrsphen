use chat_adapter::{LoginSnapshot, User};
use gpui_kit::App;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub enum Protocol {
    Qq,
    Telegram,
    Matrix,
    Mock,
}
impl Protocol {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "qq" => Some(Self::Qq),
            "telegram" => Some(Self::Telegram),
            "matrix" => Some(Self::Matrix),
            "mock" => Some(Self::Mock),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Qq => "QQ",
            Self::Telegram => "Telegram",
            Self::Matrix => "Matrix",
            Self::Mock => "Mock",
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub enum Location {
    Managed,
    LegacyQq,
    LegacyTelegram,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub protocol: Protocol,
    pub account_id: String,
    pub user: Option<User>,
    pub location: Location,
}
impl Profile {
    pub fn fresh(protocol: Protocol) -> Self {
        let id = hex::encode(rand::random::<[u8; 16]>());
        Self {
            account_id: id.clone(),
            id,
            protocol,
            user: None,
            location: Location::Managed,
        }
    }
    pub fn directory(&self, root: &Path) -> PathBuf {
        match self.location {
            Location::Managed => root.join("accounts").join(&self.id),
            Location::LegacyQq => root.to_owned(),
            Location::LegacyTelegram => root.join("adapters"),
        }
    }
    pub fn label(&self) -> &str {
        self.user
            .as_ref()
            .map(|user| user.name.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(self.protocol.name())
    }
}
#[derive(Default, Serialize, Deserialize)]
pub struct Catalog {
    pub profiles: Vec<Profile>,
    pub selected: Option<String>,
}
impl Catalog {
    pub fn load(root: &Path) -> anyhow::Result<Self> {
        let path = root.join("accounts.bin");
        if path.exists() {
            return crate::proto::decode(&std::fs::read(path)?);
        }
        let mut catalog = Self::default();
        if let Ok(bytes) = std::fs::read(root.join("keystore.bin")) {
            let session = crate::proto::decode_keystore_file(&bytes)?;
            if session.keystore.uin != 0
                && (!session.keystore.a1.is_empty() || !session.keystore.a2.is_empty())
            {
                catalog.profiles.push(Profile {
                    id: "legacy-qq".into(),
                    protocol: Protocol::Qq,
                    account_id: session.keystore.uin.to_string(),
                    user: Some(User {
                        id: session.keystore.uin.to_string(),
                        name: "QQ".into(),
                        avatar: None,
                    }),
                    location: Location::LegacyQq,
                });
            }
        }
        if root.join("adapters/ferrsphen.skv").is_dir() {
            let db = crate::storage::Database::open(&root.join("adapters"))?;
            let key = format!(
                "adapter/session/{}",
                hex::encode("telegram/64656661756c74/user")
            );
            if let Some(serde_json::Value::String(id)) = db.get::<serde_json::Value>(&key)? {
                catalog.profiles.push(Profile {
                    id: "legacy-telegram".into(),
                    protocol: Protocol::Telegram,
                    account_id: "default".into(),
                    user: Some(User {
                        id,
                        name: "Telegram".into(),
                        avatar: None,
                    }),
                    location: Location::LegacyTelegram,
                });
            }
        }
        catalog.selected = catalog.profiles.first().map(|profile| profile.id.clone());
        Ok(catalog)
    }
    pub fn save(&self, root: &Path) -> anyhow::Result<()> {
        use std::io::Write as _;
        std::fs::create_dir_all(root)?;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&crate::proto::encode(self)?)?;
        file.as_file().sync_all()?;
        file.persist(root.join("accounts.bin"))?;
        Ok(())
    }
    pub fn remember(&mut self, profile: Profile) {
        if let Some(existing) = self.profiles.iter_mut().find(|old| old.id == profile.id) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }
}
pub type LoginObserver = Rc<dyn Fn(LoginSnapshot, &mut App)>;
#[derive(Clone)]
pub struct SessionHooks {
    pub on_login: LoginObserver,
    pub accounts: crate::launcher::ProtocolSwitch,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accounts_share_a_catalog_but_never_a_managed_directory() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = Catalog::load(root.path()).unwrap();
        assert!(catalog.profiles.is_empty());
        let mut first = Profile::fresh(Protocol::Telegram);
        let mut second = Profile::fresh(Protocol::Telegram);
        for profile in [&mut first, &mut second] {
            profile.user = Some(User {
                id: "same".into(),
                name: "A".into(),
                avatar: None,
            });
        }
        assert_ne!(first.directory(root.path()), second.directory(root.path()));
        catalog.remember(first.clone());
        catalog.remember(second.clone());
        catalog.selected = Some(second.id.clone());
        catalog.save(root.path()).unwrap();
        let restored = Catalog::load(root.path()).unwrap();
        assert_eq!(restored.profiles.len(), 2);
        assert_eq!(restored.selected, Some(second.id));
    }
    #[test]
    fn creating_an_unfinished_login_does_not_auto_select_a_platform_next_launch() {
        let root = tempfile::tempdir().unwrap();
        let _pending = Profile::fresh(Protocol::Matrix);
        std::fs::create_dir_all(root.path().join("accounts")).unwrap();
        assert!(Catalog::load(root.path()).unwrap().profiles.is_empty());
    }
}
