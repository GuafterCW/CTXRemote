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
