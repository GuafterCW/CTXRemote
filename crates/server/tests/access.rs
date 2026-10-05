//! Access without a password for devices of the host's account, through a
//! real server: it needs the account's access password *and* a device key
//! the server still counts as a member.

mod common;

use std::sync::mpsc as std_mpsc;

use common::*;
use ctxremote_core::account::{self, access_password, AccessGrant};
use ctxremote_core::viewer::ViewerSession;
use ed25519_dalek::SigningKey;

async fn try_connect(
    server: &str,
    id: DeviceId,
    password: &str,
    member: Option<&SigningKey>,
) -> anyhow::Result<(ViewerSession, std_mpsc::Receiver<ctxremote_core::viewer::ViewerEvent>)> {
    let (tx, rx) = std_mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let session = ViewerSession::connect(server, None, id, password, member, None, None, move |event| {
        let _ = tx.lock().unwrap().send(event);
    })
    .await?;
    Ok((session, rx))
}

#[tokio::test(flavor = "multi_thread")]
async fn account_devices_connect_without_a_password() {
    let (_server, addr) = start_server().await;
    // The host computer's own app key (the witness), a second device of the
    // account, and a stranger.
    let (home, laptop, stranger) =
        (SigningKey::from_bytes(&[21; 32]), SigningKey::from_bytes(&[22; 32]), SigningKey::from_bytes(&[23; 32]));
    let link = account::create(&addr, &home).await.unwrap();
    let code = account::offer_pairing(&addr, &home, &link).await.unwrap();
    account::join(&addr, &laptop, &code).await.unwrap();
    let account_key: [u8; 32] = hex::decode(&link.key).unwrap().try_into().unwrap();

    let (host, id, config) = start_host_shared(&addr, |_| {}).await;
    let mut app = Config { account: Some(link.clone()), device_key: hex::encode(home.to_bytes()), ..Config::default() };
    app.server = addr.clone();
    let grant = AccessGrant::for_host(&app, id).unwrap();
    assert_eq!(grant.password, access_password(&account_key, id));
    let access = grant.password.clone();

    // Not allowed yet: the account password is just a wrong password.
    assert!(try_connect(&addr, id, &access, Some(&laptop)).await.is_err());

    config.write().unwrap().account_access = Some(grant);
    let (session, events) = try_connect(&addr, id, &access, Some(&laptop)).await.expect("Konto-Gerät kommt rein");
    echo(&session, &events, "über das Konto");
    session.close();

    // Knowing the account password is not enough without membership …
    let refused = try_connect(&addr, id, &access, Some(&stranger)).await.err().expect("Fremder abgewiesen");
    assert!(refused.to_string().contains("nicht mehr zum Konto"), "{refused:#}");
    // … nor without proving it at all.
    assert!(try_connect(&addr, id, &access, None).await.is_err());
    // A password for another host does not fit.
    let other = access_password(&account_key, "111222333".parse().unwrap());
    assert!(try_connect(&addr, id, &other, Some(&laptop)).await.is_err());

    // The host's own password still works as before.
    let (session, events) = try_connect(&addr, id, &host.password(), None).await.unwrap();
    echo(&session, &events, "mit Passwort");
    session.close();

    // Once the laptop leaves the account, its key opens nothing, although
    // it still knows the account key.
    account::leave(&addr, &laptop).await.unwrap();
    let refused = try_connect(&addr, id, &access, Some(&laptop)).await.err().expect("ehemaliges Gerät abgewiesen");
    assert!(refused.to_string().contains("nicht mehr zum Konto"), "{refused:#}");

    // The host's connection log has all of it, newest first.
    use ctxremote_core::history::Outcome;
    let outcomes: Vec<Outcome> = host.history().iter().map(|v| v.outcome).collect();
    assert_eq!(
        outcomes,
        [
            Outcome::NotMember,
            Outcome::OneTimePassword,
            Outcome::WrongPassword,
            // The account password without a proof of membership.
            Outcome::NotMember,
            Outcome::NotMember,
            Outcome::Account,
            Outcome::WrongPassword,
        ]
    );
    let visit = &host.history()[5];
    assert!(visit.peer.contains('('), "{visit:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn wake_requests_reach_the_accounts_online_devices() {
    let (_server, addr) = start_server().await;
    let (office, laptop, stranger) =
        (SigningKey::from_bytes(&[31; 32]), SigningKey::from_bytes(&[32; 32]), SigningKey::from_bytes(&[33; 32]));
    let link = account::create(&addr, &office).await.unwrap();
    let code = account::offer_pairing(&addr, &office, &link).await.unwrap();
    account::join(&addr, &laptop, &code).await.unwrap();
    let macs = vec!["01:23:45:67:89:ab".to_string()];

    // Nobody of the account is online yet.
    assert_eq!(account::wake(&addr, &laptop, macs.clone()).await.unwrap(), 0);

    // The office computer runs a host under its account key; it can wake.
    let office_key = hex::encode(office.to_bytes());
    let (_host, _id) = start_host_with(&addr, |c| c.device_key = office_key).await;
    assert_eq!(account::wake(&addr, &laptop, macs.clone()).await.unwrap(), 1);
    // The asking device itself is not counted, and strangers have no account.
    assert_eq!(account::wake(&addr, &office, macs.clone()).await.unwrap(), 0);
    assert!(account::wake(&addr, &stranger, macs).await.is_err());
}
