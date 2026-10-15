//! Attribution (token report §7): which account a response went to, decided at query time.

use crate::ids::AccountId;

/// A Claude Code home a session ran in: the default home, or an account's `run` profile.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Home {
    Default,
    Profile(AccountId),
}
