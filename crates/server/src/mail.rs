//! Mails about accounts: confirming the address and security notices.
//!
//! Configured through the environment (see `docs/DEPLOY.md`, "Mails"):
//! `CTXREMOTE_SMTP_URL` (e.g. `smtps://user:password@smtp.example.org:465`
//! or `smtp://user:password@smtp.example.org:587?tls=required`),
//! `CTXREMOTE_MAIL_FROM` (e.g. `CTXRemote <noreply@ctx.ink>`) and
//! `CTXREMOTE_SITE_URL` for links (default `https://ctxremote.ctx.ink`).
//! Without them mails are only logged. Mails never contain anything secret
//! besides the one-time confirmation link.

use lettre::message::{Mailbox, SinglePart};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use tokio::sync::mpsc;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mail {
    /// Confirm `to`; the link carries `token`.
    Verify { to: String, token: String },
    /// Sent to the old address when the address changed.
    AddressChanged { to: String, new: String },
    PasswordChanged { to: String },
    /// A device or browser signed in to the account; `how` says which way.
    NewSignIn { to: String, how: &'static str },
    RecoveryUsed { to: String },
    AccountDeleted { to: String },
}

impl Mail {
    fn to(&self) -> &str {
        match self {
            Mail::Verify { to, .. }
            | Mail::AddressChanged { to, .. }
            | Mail::PasswordChanged { to }
            | Mail::NewSignIn { to, .. }
            | Mail::RecoveryUsed { to }
            | Mail::AccountDeleted { to } => to,
        }
    }

    /// Subject and plain-text body.
    fn render(&self, site: &str) -> (String, String) {
        let footer = format!(
            "\n\nViele Grüße\nIhr CTXRemote\n\n{site}\nDiese Nachricht wurde automatisch verschickt."
        );
        let unexpected = "Waren Sie das nicht? Dann ändern Sie bitte sofort Ihr Passwort im Konto und entfernen \
                          unbekannte Geräte.";
        match self {
            Mail::Verify { token, .. } => (
                "Bitte bestätigen Sie Ihre E-Mail-Adresse".into(),
                format!(
                    "Hallo,\n\nbitte bestätigen Sie Ihre E-Mail-Adresse für Ihr CTXRemote-Konto:\n\n\
                     {site}/konto/#bestaetigen={token}\n\nDer Link gilt 48 Stunden. Wenn Sie kein Konto angelegt \
                     haben, können Sie diese Nachricht ignorieren.{footer}"
                ),
            ),
            Mail::AddressChanged { new, .. } => (
                "Ihre E-Mail-Adresse wurde geändert".into(),
                format!(
                    "Hallo,\n\ndie E-Mail-Adresse Ihres CTXRemote-Kontos wurde auf {new} geändert. Diese Adresse \
                     erhält ab jetzt keine Nachrichten mehr zum Konto.\n\n{unexpected}{footer}"
                ),
            ),
            Mail::PasswordChanged { .. } => (
                "Ihr Passwort wurde geändert".into(),
                format!(
                    "Hallo,\n\ndas Passwort Ihres CTXRemote-Kontos wurde gerade geändert. Dabei ist auch ein neuer \
                     Wiederherstellungscode entstanden, der alte gilt nicht mehr.\n\n{unexpected}{footer}"
                ),
            ),
            Mail::NewSignIn { how, .. } => (
                "Neue Anmeldung bei Ihrem Konto".into(),
                format!("Hallo,\n\nsoeben hat sich {how} bei Ihrem CTXRemote-Konto angemeldet.\n\n{unexpected}{footer}"),
            ),
            Mail::AccountDeleted { .. } => (
                "Ihr Konto wurde gelöscht".into(),
                format!(
                    "Hallo,\n\nIhr CTXRemote-Konto wurde soeben gelöscht, mit Anmeldung, Geräten und Geräteliste. Ihre \
                     Geräte arbeiten ohne Konto weiter.\n\nWaren Sie das nicht? Dann melden Sie sich bitte bei uns.{footer}"
                ),
            ),
            Mail::RecoveryUsed { .. } => (
                "Ihr Wiederherstellungscode wurde verwendet".into(),
                format!(
                    "Hallo,\n\nmit Ihrem Wiederherstellungscode wurde gerade ein neues Passwort für Ihr \
                     CTXRemote-Konto gesetzt. Der Code gilt jetzt nicht mehr.\n\n{unexpected}{footer}"
                ),
            ),
        }
    }
}

/// A plain-text UTF-8 mail. `singlepart` (not `body`) writes MIME-Version
/// and Content-Type, without which some clients show the quoted-printable
/// text undecoded.
fn build(from: Mailbox, to: Mailbox, subject: String, body: String) -> Result<Message, lettre::error::Error> {
    Message::builder().from(from).to(to).subject(subject).singlepart(SinglePart::plain(body))
}

/// Starts the mail sender and returns the queue the accounts post to.
pub fn start() -> mpsc::UnboundedSender<Mail> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Mail>();
    let site = std::env::var("CTXREMOTE_SITE_URL").unwrap_or_else(|_| "https://ctxremote.ctx.ink".into());
    let site = site.trim_end_matches('/').to_string();
    let transport = std::env::var("CTXREMOTE_SMTP_URL").ok().and_then(|url| {
        match AsyncSmtpTransport::<Tokio1Executor>::from_url(&url) {
            Ok(builder) => Some(builder.build()),
            Err(e) => {
                warn!("CTXREMOTE_SMTP_URL ungültig, Mails werden nicht verschickt: {e}");
                None
            }
        }
    });
    let from: Option<Mailbox> = std::env::var("CTXREMOTE_MAIL_FROM").ok().and_then(|f| f.parse().ok());
    match (&transport, &from) {
        (Some(_), Some(from)) => info!("Mails werden verschickt als {from}"),
        _ => info!("Kein Mailversand eingerichtet (CTXREMOTE_SMTP_URL, CTXREMOTE_MAIL_FROM); Mails werden nur protokolliert"),
    }
    tokio::spawn(async move {
        while let Some(mail) = rx.recv().await {
            let (subject, body) = mail.render(&site);
            let (Some(transport), Some(from)) = (&transport, &from) else {
                info!(to = mail.to(), subject, "Mail nicht verschickt (kein Mailversand eingerichtet)");
                continue;
            };
            let Ok(to) = mail.to().parse::<Mailbox>() else {
                warn!(to = mail.to(), "ungültige Empfängeradresse");
                continue;
            };
            let message = build(from.clone(), to, subject, body);
            match message {
                Ok(message) => {
                    if let Err(e) = transport.send(message).await {
                        warn!(to = mail.to(), "Mail nicht verschickt: {e}");
                    }
                }
                Err(e) => warn!("Mail nicht gebaut: {e}"),
            }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_carry_the_link_and_nothing_secret() {
        let (subject, body) = Mail::Verify { to: "a@b.de".into(), token: "TOKEN".into() }.render("https://site");
        assert!(subject.contains("bestätigen"));
        assert!(body.contains("https://site/konto/#bestaetigen=TOKEN"));
        let (_, body) = Mail::NewSignIn { to: "a@b.de".into(), how: "ein Gerät" }.render("https://site");
        assert!(body.contains("ein Gerät") && body.contains("Passwort"));
    }

    #[test]
    fn mails_declare_their_encoding() {
        let from = "CTXRemote <noreply@example.org>".parse().unwrap();
        let to = "a@b.de".parse().unwrap();
        let mail = build(from, to, "Grüße".into(), "bestätigen".into()).unwrap();
        let text = String::from_utf8(mail.formatted()).unwrap();
        assert!(text.contains("MIME-Version: 1.0"));
        assert!(text.contains("Content-Type: text/plain; charset=utf-8"));
    }
}
