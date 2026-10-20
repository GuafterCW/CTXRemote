//! The team's support queue (see `docs/ACCOUNTS.md`, "Warteschlange"): a
//! help link of the team instead of one helper's device. Offers made
//! through it wait on the server until a member takes one; the others no
//! longer see it then.
//!
//! The link's token sits in the team's book, so every member can open the
//! offers and nobody else; the server keeps them sealed, in memory only.

use anyhow::{bail, Result};
use ctxremote_proto::account::{AccountError, AccountOp, AccountReply, QueueEntry};
use ctxremote_proto::invite::{self, HelpOffer};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use crate::account::{request, AccountLink};
use crate::team::{self, TeamView};

/// What the book keeps about the help link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelpLink {
    /// Its name on the server ([`invite::QUEUE_LEN`] letters).
    pub queue: String,
    /// The token offers are sealed with; never on the server.
    pub token: String,
}

impl HelpLink {
    /// The link to hand out, with the team's name for the page only.
    pub fn url(&self, website: &str, team_name: &str) -> String {
        let name: String = team_name.chars().take(60).collect();
        let name = urlencoding(&name);
        // Everything after "#" stays in the browser.
        if name.is_empty() {
            format!("{website}/hilfe.html#t{}-{}", self.queue, self.token)
        } else {
            format!("{website}/hilfe.html#t{}-{}/{name}", self.queue, self.token)
        }
    }
}

/// Percent-encodes everything but unreserved characters.
fn urlencoding(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// An offer in the queue, opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waiting {
    pub ticket: u64,
    /// The asking device's ID.
    pub id: String,
    /// The name it gave (self-declared).
    pub name: String,
    /// Unix seconds since it waits.
    pub since: u64,
}

/// Owners and admins: makes a new help link for the team; one it had stops
/// working, and offers made through it go.
pub async fn open_link(server: &str, key: &SigningKey, account: &AccountLink) -> Result<TeamView> {
    for _ in 0..5 {
        let link = HelpLink { queue: invite::new_queue(), token: invite::new_token() };
        match request(server, key, AccountOp::SetHelpQueue { queue: Some(link.queue.clone()) }).await {
            Ok(_) => return team::update_book(server, key, account, |book| book.help = Some(link.clone())).await,
            Err(e) if e.downcast_ref::<AccountError>() == Some(&AccountError::Conflict) => continue,
            Err(e) => return Err(e),
        }
    }
    bail!("Kein freier Name für den Hilfelink gefunden, bitte nochmal versuchen")
}

/// Owners and admins: ends the help link.
pub async fn close_link(server: &str, key: &SigningKey, account: &AccountLink) -> Result<TeamView> {
    request(server, key, AccountOp::SetHelpQueue { queue: None }).await?;
    team::update_book(server, key, account, |book| book.help = None).await
}

fn opened(link: &HelpLink, entry: &QueueEntry) -> Option<HelpOffer> {
    (invite::hint(&link.token) == entry.hint).then_some(())?;
    invite::open(&link.token, entry.from, &entry.sealed)
}

/// The offers waiting, oldest first; ones the link's token does not open
/// (made up, or for an older link) are left out.
pub async fn waiting(server: &str, key: &SigningKey, link: &HelpLink) -> Result<Vec<Waiting>> {
    let entries = match request(server, key, AccountOp::GetQueue).await? {
        AccountReply::Queue(entries) => entries,
        other => bail!("unerwartete Serverantwort: {other:?}"),
    };
    Ok(entries
        .iter()
        .filter_map(|e| {
            let offer = opened(link, e)?;
            Some(Waiting { ticket: e.ticket, id: offer.from.to_string(), name: offer.name, since: e.since })
        })
        .collect())
}

/// Takes `ticket`: its ID and password to connect with, if nobody was faster.
pub async fn claim(server: &str, key: &SigningKey, link: &HelpLink, ticket: u64) -> Result<HelpOffer> {
    let entry = match request(server, key, AccountOp::ClaimHelp { ticket }).await? {
        AccountReply::Claimed(entry) => entry,
        other => bail!("unerwartete Serverantwort: {other:?}"),
    };
    opened(link, &entry).ok_or_else(|| anyhow::anyhow!("Die Anfrage ließ sich nicht öffnen"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_keeps_the_token_behind_the_hash() {
        let link = HelpLink { queue: "k3x9pq7m".into(), token: "abcdefghijkmnpqrstuv".into() };
        assert_eq!(link.url("https://remocto.com", ""), "https://remocto.com/hilfe.html#tk3x9pq7m-abcdefghijkmnpqrstuv");
        assert_eq!(
            link.url("https://remocto.com", "Müller & Söhne"),
            "https://remocto.com/hilfe.html#tk3x9pq7m-abcdefghijkmnpqrstuv/M%C3%BCller%20%26%20S%C3%B6hne"
        );
    }
}
