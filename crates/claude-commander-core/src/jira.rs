//! Jira ticket lookup and browser URL construction.

use std::sync::LazyLock;

use regex::Regex;

static TICKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z]{3,}-[0-9]+").expect("valid Jira ticket pattern"));

/// First match in the review title, session name, then branch, normalized to uppercase.
pub fn reference(review_title: Option<&str>, session_name: &str, branch: &str) -> Option<String> {
    review_title
        .into_iter()
        .chain([session_name, branch])
        .find_map(|text| TICKET.find(text).map(|m| m.as_str().to_ascii_uppercase()))
}

/// Hostname and optional port only; schemes, paths and credentials are rejected.
pub fn validate_hostname(hostname: &str) -> Result<(), &'static str> {
    claude_commander_protocol::hosting::validate_gitlab_hostname(hostname)
        .map(|_| ())
        .map_err(|_| "Use a hostname with an optional port, without a scheme, path or credentials")
}

/// Construct the HTTPS issue URL for a configured Jira host.
pub fn browse_url(hostname: &str, ticket: &str) -> Result<String, &'static str> {
    validate_hostname(hostname)?;
    Ok(format!("https://{hostname}/browse/{ticket}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jira_reference_uses_review_then_session_then_branch() {
        assert_eq!(
            reference(Some("Review abc-12 XYZ-99"), "DEF-34", "ghi-56"),
            Some("ABC-12".into())
        );
        assert_eq!(
            reference(Some("No ticket"), "Def-34 abc-2", "ghi-56"),
            Some("DEF-34".into())
        );
        assert_eq!(
            reference(None, "No ticket", "feature/ghi-56"),
            Some("GHI-56".into())
        );
        assert_eq!(reference(None, "AB-12", "no-ticket"), None);
        assert_eq!(reference(None, "ABC-no", "no-ticket"), None);
    }

    #[test]
    fn jira_hostname_builds_https_browse_url() {
        assert_eq!(
            browse_url("jira.example.com:8443", "ABC-12").unwrap(),
            "https://jira.example.com:8443/browse/ABC-12"
        );
        for host in [
            "",
            "https://jira.example.com",
            "user:secret@jira.example.com",
            "jira.example.com/path",
            "jira.example.com?x=1",
        ] {
            assert!(browse_url(host, "ABC-12").is_err(), "{host}");
        }
    }
}
