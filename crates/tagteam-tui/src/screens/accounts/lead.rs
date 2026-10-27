//! What the Now strip on Accounts keeps of the lead card it replaced (S§3.2): focus's account in
//! use, the forecast's best pick in the strip's form, the strip's spacing and its hint with no
//! account in use.

use ratatui::text::Span;
use tagteam_engine::switch::BestForecast;
use tagteam_engine::views::{AccountView, ProviderAccounts};
use tagteam_engine::words;

use crate::daemon;
use crate::state::State;
use crate::theme::Role;

/// The least room between the strip's left part and the best pick (§4.3's group gap).
pub(crate) const PICK_GAP: usize = 3;
/// The left part keeps this much room at least, or the best pick is dropped (T3-19).
pub(crate) const LEFT_LEAST: usize = 16;
/// What the strip on Accounts says to do with no account in use, after `no account in use · `.
pub(crate) const NO_ACCOUNT_HINT: &str = "s switches to the selected account";

/// Focus's provider and its account in use, when it has one.
pub(crate) fn in_use(state: &State) -> Option<(&ProviderAccounts, &AccountView)> {
    let view = state.view_of(&daemon::focus(state))?;
    let v = view.accounts.iter().find(|v| v.active)?;
    Some((view, v))
}

/// The forecast's words in the Now strip's form (§5.4: no key, and nothing for any `None`).
pub(crate) fn pick_words(state: &State, view: &ProviderAccounts) -> Option<Vec<Span<'static>>> {
    let muted = state.theme.role(Role::Muted);
    match &view.forecast {
        BestForecast::Pick { order } => {
            let position = *order.first()?;
            let name = view
                .accounts
                .iter()
                .find(|a| a.row.position == position)
                .map(|a| a.row.alias.clone().unwrap_or_else(|| words::email(&a.row)));
            let pick = match name {
                Some(name) => format!("{position} {name}"),
                None => position.to_string(),
            };
            Some(vec![
                Span::styled("best now", muted),
                Span::raw(" "),
                Span::raw(pick),
            ])
        }
        BestForecast::AlreadyBest => Some(vec![Span::styled("on the best account", muted)]),
        BestForecast::None(_) => None,
    }
}
