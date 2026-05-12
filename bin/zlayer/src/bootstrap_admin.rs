use anyhow::{bail, Context, Result};
use std::sync::Arc;
use tracing::{info, warn};
use zlayer_api::{IdentityManager, UserRole};

const ENV_EMAIL: &str = "ZLAYER_BOOTSTRAP_EMAIL";
const ENV_PASSWORD: &str = "ZLAYER_BOOTSTRAP_PASSWORD";
const ENV_PASSWORD_FILE: &str = "ZLAYER_BOOTSTRAP_PASSWORD_FILE";
const ENV_DISPLAY_NAME: &str = "ZLAYER_BOOTSTRAP_DISPLAY_NAME";

fn load_password() -> Result<String> {
    let inline = std::env::var(ENV_PASSWORD).ok();
    let file = std::env::var(ENV_PASSWORD_FILE).ok();

    match (inline, file) {
        (Some(_), Some(_)) => {
            bail!("{ENV_PASSWORD} and {ENV_PASSWORD_FILE} are mutually exclusive")
        }
        (Some(p), None) => {
            if p.is_empty() {
                bail!("{ENV_PASSWORD} is set but empty");
            }
            Ok(p)
        }
        (None, Some(path)) => {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {ENV_PASSWORD_FILE} at {path}"))?;
            let trimmed = raw.trim_end_matches(['\n', '\r']).to_string();
            if trimmed.is_empty() {
                bail!("{ENV_PASSWORD_FILE} at {path} is empty");
            }
            Ok(trimmed)
        }
        (None, None) => {
            bail!("{ENV_EMAIL} is set but neither {ENV_PASSWORD} nor {ENV_PASSWORD_FILE} is")
        }
    }
}

pub async fn maybe_bootstrap_admin(identity: &Arc<IdentityManager>) -> Result<()> {
    let count = identity
        .users()
        .count()
        .await
        .context("user store count failed")?;

    if count > 0 {
        if std::env::var(ENV_EMAIL).is_ok() {
            info!("users table non-empty; ignoring {ENV_EMAIL} (already bootstrapped)");
        }
        return Ok(());
    }

    let email = match std::env::var(ENV_EMAIL) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            warn!(
                "no bootstrap credentials configured — first HTTP client to reach /auth/bootstrap will become admin. Set ZLAYER_BOOTSTRAP_EMAIL + ZLAYER_BOOTSTRAP_PASSWORD(_FILE) to provision non-interactively."
            );
            return Ok(());
        }
    };

    let password = load_password()?;
    if password.len() < 12 {
        warn!("bootstrap password is shorter than 12 chars — strongly consider longer");
    }

    let email_lc = email.trim().to_lowercase();
    let display_name = std::env::var(ENV_DISPLAY_NAME)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            email_lc
                .split('@')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("admin")
                .to_string()
        });

    identity
        .create_user(&email_lc, display_name, UserRole::Admin, &password)
        .await
        .context("IdentityManager::create_user failed")?;

    info!(email = %email_lc, "bootstrap admin created from env");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use zlayer_api::storage::{InMemoryUserStore, UserStorage};
    use zlayer_paths::ZLayerDirs;
    use zlayer_secrets::{CredentialStore, EncryptionKey, PersistentSecretsStore};
    use zlayer_types::storage::StoredUser;

    fn clear_envs() {
        std::env::remove_var(ENV_EMAIL);
        std::env::remove_var(ENV_PASSWORD);
        std::env::remove_var(ENV_PASSWORD_FILE);
        std::env::remove_var(ENV_DISPLAY_NAME);
    }

    async fn make_identity() -> (
        Arc<IdentityManager>,
        Arc<dyn UserStorage>,
        Arc<CredentialStore<Arc<PersistentSecretsStore>>>,
        zlayer_types::Scratch,
    ) {
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("make-identity-")
            .unwrap();
        let users: Arc<dyn UserStorage> = Arc::new(InMemoryUserStore::new());
        let secrets = Arc::new(
            PersistentSecretsStore::open(
                tmp.path().join("secrets.sqlite"),
                EncryptionKey::generate(),
            )
            .await
            .unwrap(),
        );
        let credentials = Arc::new(CredentialStore::new(secrets));
        let identity = Arc::new(IdentityManager::new(users.clone(), credentials.clone()));
        (identity, users, credentials, tmp)
    }

    #[test]
    #[serial]
    fn load_password_inline_non_empty() {
        clear_envs();
        std::env::set_var(ENV_PASSWORD, "hunter2hunter2");
        assert_eq!(load_password().unwrap(), "hunter2hunter2");
        clear_envs();
    }

    #[test]
    #[serial]
    fn load_password_inline_empty_errors() {
        clear_envs();
        std::env::set_var(ENV_PASSWORD, "");
        let err = load_password().unwrap_err().to_string();
        assert!(err.contains("set but empty"), "unexpected: {err}");
        clear_envs();
    }

    #[test]
    #[serial]
    fn load_password_both_set_errors() {
        clear_envs();
        std::env::set_var(ENV_PASSWORD, "abc");
        std::env::set_var(ENV_PASSWORD_FILE, "/tmp/whatever");
        let err = load_password().unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "unexpected: {err}");
        clear_envs();
    }

    #[test]
    #[serial]
    fn load_password_neither_set_errors() {
        clear_envs();
        let err = load_password().unwrap_err().to_string();
        assert!(err.contains("is set but neither"), "unexpected: {err}");
    }

    #[test]
    #[serial]
    fn load_password_file_reads_and_trims() {
        clear_envs();
        let tmp = ZLayerDirs::system_default()
            .scratch_file("load-password-file-reads-and-trims-")
            .unwrap();
        writeln!(tmp.as_file(), "filesecret123").unwrap();
        std::env::set_var(ENV_PASSWORD_FILE, tmp.path());
        let got = load_password().unwrap();
        assert_eq!(got, "filesecret123");
        clear_envs();
    }

    #[test]
    #[serial]
    fn load_password_file_empty_errors() {
        clear_envs();
        let tmp = ZLayerDirs::system_default()
            .scratch_file("load-password-file-empty-errors-")
            .unwrap();
        std::env::set_var(ENV_PASSWORD_FILE, tmp.path());
        let err = load_password().unwrap_err().to_string();
        assert!(err.contains("is empty"), "unexpected: {err}");
        clear_envs();
    }

    #[test]
    #[serial]
    fn load_password_file_missing_errors() {
        clear_envs();
        std::env::set_var(ENV_PASSWORD_FILE, "/nonexistent/zlayer/bootstrap/pw");
        let err = load_password().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("reading"), "unexpected: {msg}");
        clear_envs();
    }

    #[tokio::test]
    #[serial]
    async fn maybe_bootstrap_short_circuits_when_table_non_empty() {
        clear_envs();
        let (identity, users, creds, _tmp) = make_identity().await;
        let seed = StoredUser::new(
            "seed@example.com".to_string(),
            "Seed".to_string(),
            UserRole::User,
        );
        users.store(&seed).await.unwrap();

        std::env::set_var(ENV_EMAIL, "new@example.com");
        std::env::set_var(ENV_PASSWORD, "correcthorsebatterystaple");
        maybe_bootstrap_admin(&identity).await.unwrap();

        assert_eq!(users.count().await.unwrap(), 1);
        assert!(!creds.exists("new@example.com").await.unwrap());
        clear_envs();
    }

    #[tokio::test]
    #[serial]
    async fn maybe_bootstrap_no_envs_is_noop() {
        clear_envs();
        let (identity, users, _creds, _tmp) = make_identity().await;
        maybe_bootstrap_admin(&identity).await.unwrap();
        assert_eq!(users.count().await.unwrap(), 0);
    }

    #[tokio::test]
    #[serial]
    async fn maybe_bootstrap_creates_user_and_credential() {
        clear_envs();
        let (identity, users, creds, _tmp) = make_identity().await;

        std::env::set_var(ENV_EMAIL, "  Admin@Example.COM  ");
        std::env::set_var(ENV_PASSWORD, "correcthorsebatterystaple");
        std::env::set_var(ENV_DISPLAY_NAME, "The Admin");

        maybe_bootstrap_admin(&identity).await.unwrap();

        assert_eq!(users.count().await.unwrap(), 1);
        let fetched = users
            .get_by_email("admin@example.com")
            .await
            .unwrap()
            .expect("user row");
        assert_eq!(fetched.email, "admin@example.com");
        assert_eq!(fetched.display_name, "The Admin");
        assert_eq!(fetched.role, UserRole::Admin);

        let roles = creds
            .validate("admin@example.com", "correcthorsebatterystaple")
            .await
            .unwrap()
            .expect("validate");
        assert!(roles.iter().any(|r| r == "admin"));
        clear_envs();
    }

    #[tokio::test]
    #[serial]
    async fn maybe_bootstrap_email_without_password_errors() {
        clear_envs();
        let (identity, _users, _creds, _tmp) = make_identity().await;
        std::env::set_var(ENV_EMAIL, "admin@example.com");
        let err = maybe_bootstrap_admin(&identity).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("is set but neither"), "unexpected: {msg}");
        clear_envs();
    }

    #[tokio::test]
    #[serial]
    async fn maybe_bootstrap_is_idempotent_across_restart() {
        clear_envs();
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("maybe-bootstrap-is-idempotent-across-restart-")
            .unwrap();
        let users_path = tmp.path().join("users.db");
        let secrets_path = tmp.path().join("secrets.sqlite");
        let key = EncryptionKey::generate();

        std::env::set_var(ENV_EMAIL, "admin@example.com");
        std::env::set_var(ENV_PASSWORD, "correcthorsebatterystaple");

        {
            let users: Arc<dyn UserStorage> = Arc::new(
                zlayer_api::storage::SqlxUserStore::open(&users_path)
                    .await
                    .unwrap(),
            );
            let secrets = Arc::new(
                PersistentSecretsStore::open(&secrets_path, key.clone())
                    .await
                    .unwrap(),
            );
            let creds = Arc::new(CredentialStore::new(secrets));
            let identity = Arc::new(IdentityManager::new(users.clone(), creds.clone()));
            maybe_bootstrap_admin(&identity).await.unwrap();
            assert_eq!(users.count().await.unwrap(), 1);
        }

        {
            let users: Arc<dyn UserStorage> = Arc::new(
                zlayer_api::storage::SqlxUserStore::open(&users_path)
                    .await
                    .unwrap(),
            );
            let secrets = Arc::new(
                PersistentSecretsStore::open(&secrets_path, key)
                    .await
                    .unwrap(),
            );
            let creds = Arc::new(CredentialStore::new(secrets));
            let identity = Arc::new(IdentityManager::new(users.clone(), creds.clone()));
            maybe_bootstrap_admin(&identity).await.unwrap();
            assert_eq!(
                users.count().await.unwrap(),
                1,
                "restart must not duplicate"
            );
            let roles = creds
                .validate("admin@example.com", "correcthorsebatterystaple")
                .await
                .unwrap()
                .expect("credential survives restart");
            assert!(roles.iter().any(|r| r == "admin"));
        }

        clear_envs();
    }

    #[tokio::test]
    #[serial]
    async fn maybe_bootstrap_display_name_defaults_to_local_part() {
        clear_envs();
        let (identity, users, _creds, _tmp) = make_identity().await;
        std::env::set_var(ENV_EMAIL, "alice@example.com");
        std::env::set_var(ENV_PASSWORD, "correcthorsebatterystaple");
        maybe_bootstrap_admin(&identity).await.unwrap();

        let fetched = users
            .get_by_email("alice@example.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.display_name, "alice");
        clear_envs();
    }
}
