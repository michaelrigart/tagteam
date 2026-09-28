use tagteam_core::ProviderId;
use tagteam_core::validate::{AccountRefInput, normalize_alias, parse_account_ref};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::AccountRow;

impl Engine {
    /// §10.4: a position (within `provider`, else the default provider), then an alias
    /// (unique across providers), then an exact email (within `provider` if given).
    pub fn candidates(
        &self,
        input: &str,
        provider: Option<&ProviderId>,
    ) -> Result<Vec<AccountRow>, EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(vec![]);
        };
        match parse_account_ref(input) {
            None => Ok(vec![]),
            Some(AccountRefInput::Position(p)) => {
                let provider = provider.unwrap_or(&self.default_provider);
                Ok(store.find_by_position(provider, p)?.into_iter().collect())
            }
            Some(AccountRefInput::Text(t)) => {
                if let Ok(alias) = normalize_alias(&t) {
                    if let Some(row) = store.find_by_alias(&alias)? {
                        let wrong_provider = provider.is_some_and(|p| p != &row.provider);
                        return Ok(if wrong_provider { vec![] } else { vec![row] });
                    }
                }
                Ok(store.find_by_email(&t, provider)?)
            }
        }
    }

    pub fn resolve(
        &self,
        input: &str,
        provider: Option<&ProviderId>,
    ) -> Result<AccountRow, EngineError> {
        let mut found = self.candidates(input, provider)?;
        match found.len() {
            0 => Err(EngineError::NoSuchAccount(input.to_owned())),
            1 => Ok(found.remove(0)),
            _ => Err(EngineError::Ambiguous {
                input: input.to_owned(),
                candidates: found
                    .iter()
                    .map(|r| {
                        let org = r.org_name.clone().unwrap_or_else(|| {
                            if r.org_uuid.is_empty() {
                                "personal".into()
                            } else {
                                r.org_uuid.clone()
                            }
                        });
                        format!("{} #{} {} ({org})", r.provider, r.position, r.label)
                    })
                    .collect(),
            }),
        }
    }
}
