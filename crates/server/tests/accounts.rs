//! Accounts through a real server: create, pair a second device with the
//! code, and sync the address book both ways.

mod common;

use common::*;
use ctxremote_core::account::{self, Book};
use ed25519_dalek::SigningKey;

#[tokio::test(flavor = "multi_thread")]
async fn two_devices_share_one_address_book() {
    let (_server, addr) = start_server().await;
    let (a_key, b_key, c_key) = (SigningKey::from_bytes(&[1; 32]), SigningKey::from_bytes(&[2; 32]), SigningKey::from_bytes(&[3; 32]));
    let mut a = Config::default();
    let mut b = Config::default();

    assert_eq!(account::status(&addr, &a_key).await.unwrap(), None);
    let link_a = account::create(&addr, &a_key).await.unwrap();

    // A names a device and syncs.
    let buero: DeviceId = "482913077".parse().unwrap();
    a.set_alias(buero, Some("Büro")).unwrap();
    let (book, _) = account::sync(&addr, &a_key, &link_a, &Book::from_config(&a)).await.unwrap();
    book.apply_to(&mut a);

    // B joins with the code shown on A and gets the same key.
    let code = account::offer_pairing(&addr, &a_key, &link_a).await.unwrap();
    assert_eq!(code.len(), 14, "{code}");
    // A wrong secret is refused and does not reveal the key.
    let wrong = format!("{}0000-0000", &code[..5]);
    assert!(account::join(&addr, &c_key, &wrong).await.is_err());
    let link_b = account::join(&addr, &b_key, &code.to_lowercase()).await.unwrap();
    assert_eq!(link_b.key, link_a.key);
    assert_eq!(link_b.devices, 2);
    // The code is spent.
    assert!(account::join(&addr, &c_key, &code).await.is_err());

    // B has its own device and receives A's.
    let mama: DeviceId = "731004552".parse().unwrap();
    b.set_alias(mama, Some("Mama")).unwrap();
    let (book, _) = account::sync(&addr, &b_key, &link_b, &Book::from_config(&b)).await.unwrap();
    book.apply_to(&mut b);
    assert_eq!(b.resolve("büro"), Some(buero));
    assert_eq!(b.resolve("Mama"), Some(mama));

    // A gets B's, and a removal on A reaches B.
    let (book, _) = account::sync(&addr, &a_key, &link_a, &Book::from_config(&a)).await.unwrap();
    book.apply_to(&mut a);
    assert_eq!(a.resolve("Mama"), Some(mama));
    a.forget(buero);
    account::sync(&addr, &a_key, &link_a, &Book::from_config(&a)).await.unwrap();
    let (book, _) = account::sync(&addr, &b_key, &link_b, &Book::from_config(&b)).await.unwrap();
    book.apply_to(&mut b);
    assert_eq!(b.resolve("Büro"), None);
    assert_eq!(b.peers.len(), 1);

    // A device outside the account can neither read nor write it.
    assert!(account::sync(&addr, &c_key, &link_a, &Book::default()).await.is_err());

    account::leave(&addr, &b_key).await.unwrap();
    assert_eq!(account::status(&addr, &b_key).await.unwrap(), None);
    assert_eq!(account::status(&addr, &a_key).await.unwrap().map(|i| i.devices), Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn login_with_password_and_recovery_code() {
    let (_server, addr) = start_server().await;
    let keys: Vec<SigningKey> = (10..14).map(|n| SigningKey::from_bytes(&[n; 32])).collect();
    let (a, b, c, d) = (&keys[0], &keys[1], &keys[2], &keys[3]);
    let email = "Philipp@Example.org";

    assert!(account::register(&addr, a, email, "kurz").await.is_err(), "zu kurzes Passwort");
    let (link_a, code) = account::register(&addr, a, email, "ein langes Passwort").await.unwrap();
    assert!(account::register(&addr, b, "philipp@example.org", "noch ein Passwort").await.is_err(), "Adresse vergeben");
    account::set_label(&addr, a, &link_a, "Büro-PC").await.unwrap();

    // The book A writes is readable after logging in elsewhere.
    let mut config = Config::default();
    config.set_alias("731004552".parse().unwrap(), Some("Mama")).unwrap();
    account::sync(&addr, a, &link_a, &Book::from_config(&config)).await.unwrap();
    assert!(account::login(&addr, b, email, "falsches Passwort!").await.is_err());
    let link_b = account::login(&addr, b, "philipp@example.org", "ein langes Passwort").await.unwrap();
    assert_eq!(link_b.key, link_a.key);
    let (book, _) = account::sync(&addr, b, &link_b, &Book::default()).await.unwrap();
    let mut other = Config::default();
    book.apply_to(&mut other);
    assert_eq!(other.resolve("mama"), Some("731004552".parse().unwrap()));

    // Devices with their names; B sees A's name and itself.
    let devices = account::devices(&addr, b, &link_b).await.unwrap();
    assert_eq!(devices.len(), 2);
    assert!(devices.iter().any(|d| d.name == "Büro-PC" && !d.this));
    assert!(devices.iter().any(|d| d.this));

    // Recovery: the code gives the key back and sets a new password.
    let (link_c, new_code) = account::recover(&addr, c, email, &code.to_lowercase(), "das neue Passwort").await.unwrap();
    assert_eq!(link_c.key, link_a.key);
    assert_ne!(new_code, code);
    assert!(account::login(&addr, d, email, "ein langes Passwort").await.is_err(), "altes Passwort");
    assert!(account::recover(&addr, d, email, &code, "noch ein neues Passwort").await.is_err(), "alter Code");
    let link_d = account::login(&addr, d, email, "das neue Passwort").await.unwrap();
    assert_eq!(link_d.key, link_a.key);

    // Removing a device takes it out.
    let c_key = hex::encode(c.verifying_key().to_bytes());
    account::remove_device(&addr, a, &c_key).await.unwrap();
    assert_eq!(account::status(&addr, c).await.unwrap(), None);
    let status = account::login_status(&addr, a).await.unwrap().unwrap();
    assert_eq!(status.email, "philipp@example.org");
}

/// Values derived from passwords never travel unencrypted.
#[tokio::test(flavor = "multi_thread")]
async fn logins_need_an_encrypted_connection() {
    use ctxremote_core::proto::account::{sign, AccountError, AccountOp};
    use ctxremote_core::proto::framing;
    use ctxremote_core::proto::rendezvous::{ClientMsg, ServerMsg};
    let (_server, addr) = start_server().await;
    let mut t = framing::transport(tokio::net::TcpStream::connect(&addr).await.unwrap());
    let ServerMsg::Challenge { nonce, .. } = framing::recv::<ServerMsg>(&mut t).await.unwrap() else { panic!() };
    let op = AccountOp::Login { email: "a@b.de".into(), auth: [1; 32] };
    let auth = sign(&SigningKey::from_bytes(&[20; 32]), &nonce, &op);
    framing::send(&mut t, &ClientMsg::Account { auth, op }).await.unwrap();
    let reply = framing::recv::<ServerMsg>(&mut t).await.unwrap();
    assert!(matches!(reply, ServerMsg::Account(Err(AccountError::Unencrypted))), "{reply:?}");
}
