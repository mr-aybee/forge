//! [`StoreIam`] over RustFS's native admin API (D-170): access keys are
//! service accounts of the root user, each with an inline policy.

use async_trait::async_trait;
use forge_core::storage_iam::{IamError, IamKey, PolicyDocument, StoreIam};
use reqwest::Method;
use serde::Deserialize;

use crate::{Client, S3Error};

fn iam_error(e: S3Error) -> IamError {
    match e {
        S3Error::Transport(m) => IamError::Unreachable(m),
        S3Error::Service { status, .. } if status >= 500 => IamError::Unreachable(e.to_string()),
        S3Error::Service { status: 404, .. } => IamError::NotFound(e.to_string()),
        other => IamError::Rejected(other.to_string()),
    }
}

fn valid_access_key(k: &str) -> Result<(), IamError> {
    if k.is_empty() || k.len() > 128 || !k.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(IamError::Rejected("access key: letters and digits".into()));
    }

    Ok(())
}

#[derive(Deserialize)]
struct Accounts {
    #[serde(default)]
    accounts: Vec<Account>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    access_key: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    account_status: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountInfo {
    #[serde(default)]
    implied_policy: bool,
    /// The policy as JSON text.
    #[serde(default)]
    policy: Option<String>,
}

fn unexpected() -> IamError {
    IamError::Rejected("unexpected admin response".into())
}

#[async_trait]
impl StoreIam for Client {
    async fn list_keys(&self) -> Result<Vec<IamKey>, IamError> {
        let text = self.admin(Method::GET, "list-service-accounts", &[], None).await.map_err(iam_error)?;
        let a: Accounts = serde_json::from_str(&text).map_err(|_| unexpected())?;

        Ok(a.accounts
            .into_iter()
            .map(|a| IamKey {
                access_key: a.access_key,
                name: a.name.unwrap_or_default(),
                description: a.description.unwrap_or_default(),
                status: a.account_status,
            })
            .collect())
    }

    async fn key_policy(&self, access_key: &str) -> Result<Option<PolicyDocument>, IamError> {
        valid_access_key(access_key)?;
        let text = self
            .admin(Method::GET, "info-service-account", &[("accessKey", access_key)], None)
            .await
            .map_err(iam_error)?;
        let info: AccountInfo = serde_json::from_str(&text).map_err(|_| unexpected())?;
        if info.implied_policy {
            return Ok(None);
        }

        match info.policy.filter(|p| !p.trim().is_empty()) {
            Some(p) => serde_json::from_str(&p).map(Some).map_err(|_| unexpected()),
            None => Ok(None),
        }
    }

    async fn create_key(
        &self,
        name: &str,
        description: &str,
        policy: &PolicyDocument,
    ) -> Result<(String, String), IamError> {
        let policy = serde_json::to_value(policy).map_err(|e| IamError::Rejected(e.to_string()))?;
        self.add_service_account(name, description, &policy).await.map_err(iam_error)
    }

    async fn set_policy(&self, access_key: &str, policy: &PolicyDocument) -> Result<(), IamError> {
        valid_access_key(access_key)?;
        #[derive(serde::Serialize)]
        struct Update<'a> {
            #[serde(rename = "newPolicy")]
            new_policy: &'a PolicyDocument,
        }
        let body =
            serde_json::to_string(&Update { new_policy: policy }).map_err(|e| IamError::Rejected(e.to_string()))?;

        self.admin(Method::POST, "update-service-account", &[("accessKey", access_key)], Some(body))
            .await
            .map(|_| ())
            .map_err(iam_error)
    }

    async fn delete_key(&self, access_key: &str) -> Result<(), IamError> {
        self.delete_service_account(access_key).await.map_err(iam_error)
    }
}
