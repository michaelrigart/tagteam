use tagteam_core::ProviderId;
use tagteam_provider::Read;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::AccountRow;

#[derive(Debug, Clone)]
pub struct AccountView {
    pub row: AccountRow,
    /// The live identity wins over the store's active account.
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct ProviderAccounts {
    pub provider: ProviderId,
    pub active_position: Option<u32>,
    pub accounts: Vec<AccountView>,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum StatusView {
    NoLogin,
    Unmanaged { email: String },
    Managed { account: AccountView, total: usize },
}

impl Engine {
    fn view(&self, provider: &ProviderId) -> Result<(ProviderAccounts, Read<String>), EngineError> {
        let p = self.provider(provider)?;
        let rows = match self.existing_store()? {
            Some(s) => s.accounts(provider)?,
            None => vec![],
        };
        let live = p.live_identity(&self.env);
        let live_key = live.as_ref().map(|i| p.identity_key(i).as_str().to_owned());
        let stored_active = match (&live_key, self.existing_store()?) {
            (Read::Unreadable(_), Some(s)) => s.active(provider)?,
            _ => None,
        };
        let accounts: Vec<AccountView> = rows
            .into_iter()
            .map(|row| {
                let active = match &live_key {
                    Read::Present(k) => &row.identity_key == k,
                    Read::Absent => false,
                    Read::Unreadable(_) => stored_active.as_ref() == Some(&row.id),
                };
                AccountView { row, active }
            })
            .collect();
        let active_position = accounts.iter().find(|v| v.active).map(|v| v.row.position);
        let live_label = live.map(|i| i.email.unwrap_or(i.label));
        Ok((
            ProviderAccounts {
                provider: provider.clone(),
                active_position,
                accounts,
            },
            live_label,
        ))
    }

    /// Every provider that has accounts, plus the default provider; or just `provider`.
    pub fn accounts(
        &self,
        provider: Option<&ProviderId>,
    ) -> Result<Vec<ProviderAccounts>, EngineError> {
        let ids: Vec<ProviderId> = match provider {
            Some(p) => vec![p.clone()],
            None => {
                let mut ids = vec![self.default_provider.clone()];
                if let Some(s) = self.existing_store()? {
                    for row in s.all_accounts()? {
                        if !ids.contains(&row.provider)
                            && self.registry.get(&row.provider).is_some()
                        {
                            ids.push(row.provider);
                        }
                    }
                }
                ids
            }
        };
        ids.iter().map(|id| self.view(id).map(|(v, _)| v)).collect()
    }

    pub fn status(&self, provider: &ProviderId) -> Result<StatusView, EngineError> {
        let (list, live) = self.view(provider)?;
        let total = list.accounts.len();
        if let Some(active) = list.accounts.into_iter().find(|v| v.active) {
            return Ok(StatusView::Managed {
                account: active,
                total,
            });
        }
        match live {
            Read::Present(email) => Ok(StatusView::Unmanaged { email }),
            Read::Absent => Ok(StatusView::NoLogin),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }
}
