/// `^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$` (§10.2), without a regex engine.
pub fn is_valid_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    let local_ok = !local.is_empty()
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._%+-".contains(&b));
    // The TLD is all letters, so it can only follow the last dot.
    let Some((host, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    local_ok
        && !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        && tld.len() >= 2
        && tld.bytes().all(|b| b.is_ascii_alphabetic())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AliasError {
    #[error("an alias cannot be empty")]
    Empty,
    #[error("an alias may contain only a-z, 0-9, '_', '.' and '-'")]
    Charset,
    #[error("an alias cannot be all digits, which would read as a position")]
    AllDigits,
    #[error("an alias cannot start with '-'")]
    LeadingDash,
}

/// Aliases are lowercase and match `^[a-z0-9_.-]+$`; they cannot be all digits or start
/// with `-` (§10.3).
pub fn normalize_alias(s: &str) -> Result<String, AliasError> {
    let a = s.to_ascii_lowercase();
    if a.is_empty() {
        return Err(AliasError::Empty);
    }
    if !a
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
    {
        return Err(AliasError::Charset);
    }
    if a.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AliasError::AllDigits);
    }
    if a.starts_with('-') {
        return Err(AliasError::LeadingDash);
    }
    Ok(a)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountRefInput {
    Position(u32),
    Text(String),
}

/// First stage of §10.4: all digits is a position; anything else is an alias or email.
pub fn parse_account_ref(s: &str) -> Option<AccountRefInput> {
    if s.is_empty() {
        return None;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(p) = s.parse::<u32>() {
            return Some(AccountRefInput::Position(p));
        }
    }
    Some(AccountRefInput::Text(s.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_matches_the_spec_regex() {
        for ok in [
            "a@b.co",
            "first.last+tag@sub.example.org",
            "x_1%y@host-name.io",
            "a@..co",
        ] {
            assert!(is_valid_email(ok), "{ok}");
        }
        for bad in [
            "", "a", "a@b", "a@b.c", "@b.co", "a@.co", "a@b.c0", "a@@b.co", "a b@c.co", "a@b.co ",
            "é@b.co",
        ] {
            assert!(!is_valid_email(bad), "{bad}");
        }
    }

    #[test]
    fn alias_is_lowercased_then_checked() {
        assert_eq!(normalize_alias("Work.Main").unwrap(), "work.main");
        assert_eq!(normalize_alias("a-1_b").unwrap(), "a-1_b");
        assert_eq!(normalize_alias(""), Err(AliasError::Empty));
        assert_eq!(normalize_alias("has space"), Err(AliasError::Charset));
        assert_eq!(normalize_alias("café"), Err(AliasError::Charset));
        assert_eq!(normalize_alias("123"), Err(AliasError::AllDigits));
        assert_eq!(normalize_alias("-x"), Err(AliasError::LeadingDash));
    }

    #[test]
    fn account_refs_split_positions_from_text() {
        assert_eq!(parse_account_ref("3"), Some(AccountRefInput::Position(3)));
        assert_eq!(parse_account_ref("007"), Some(AccountRefInput::Position(7)));
        assert_eq!(
            parse_account_ref("work"),
            Some(AccountRefInput::Text("work".into()))
        );
        assert_eq!(
            parse_account_ref("a@b.co"),
            Some(AccountRefInput::Text("a@b.co".into()))
        );
        assert_eq!(parse_account_ref(""), None);
        // All digits but too large for a position: treated as text, which never matches an alias.
        assert_eq!(
            parse_account_ref("99999999999"),
            Some(AccountRefInput::Text("99999999999".into()))
        );
    }
}
