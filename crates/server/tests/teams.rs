//! Teams through a real server: create, invite with a code, roles, the
//! shared address book, and a new epoch when someone is removed.

mod common;

use common::*;
use ctxremote_core::account;
use ctxremote_core::proto::account::TeamRole;
use ctxremote_core::team;
use ed25519_dalek::SigningKey;

#[tokio::test(flavor = "multi_thread")]
async fn a_team_shares_its_book_and_shuts_out_who_leaves() {
    let (_server, addr) = start_server().await;
    let keys: Vec<SigningKey> = (1..=4).map(|n| SigningKey::from_bytes(&[n; 32])).collect();
    let (owner, admin, tech, outsider) = (&keys[0], &keys[1], &keys[2], &keys[3]);
    let mut links = Vec::new();
    for key in &keys {
        links.push(account::create(&addr, key).await.unwrap());
    }

    let created = team::create(&addr, owner, &links[0], "Werkstatt").await.unwrap();
    assert_eq!(created.book.name, "Werkstatt");
    assert_eq!(created.link.role, TeamRole::Owner);
    assert!(created.unverified.is_empty());

    let code = team::invite(&addr, owner, &links[0], TeamRole::Admin).await.unwrap();
    // The wrong code opens nothing.
    let wrong = format!("{}0000-0000", &code[..5]);
    assert!(team::join(&addr, outsider, &links[3], &wrong).await.is_err());
    let joined = team::join(&addr, admin, &links[1], &code).await.unwrap();
    assert_eq!(joined.link.key, created.link.key);
    assert_eq!(joined.book.name, "Werkstatt");
    assert_eq!(joined.link.role, TeamRole::Admin);

    let code = team::invite(&addr, admin, &links[1], TeamRole::Technician).await.unwrap();
    team::join(&addr, tech, &links[2], &code).await.unwrap();

    // The admin adds a device; everyone sees it, all keys vouched for.
    let pc: DeviceId = "482913077".parse().unwrap();
    team::update_book(&addr, admin, &links[1], |book| {
        let mut config = Config::default();
        config.set_alias(pc, Some("Kasse 1")).unwrap();
        book.book = account::Book::merge(&book.book, &account::Book::from_config(&config));
    })
    .await
    .unwrap();
    let seen = team::sync(&addr, tech, &links[2], None).await.unwrap().unwrap();
    assert_eq!(seen.book.book.entries[&pc.get()].alias.as_deref(), Some("Kasse 1"));
    assert_eq!(seen.members.len(), 3);
    assert!(seen.unverified.is_empty(), "{:?}", seen.unverified);
    // Technicians read, but do not change or invite.
    assert!(team::update_book(&addr, tech, &links[2], |book| book.name = "Meins".into()).await.is_err());
    assert!(team::invite(&addr, tech, &links[2], TeamRole::Technician).await.is_err());

    // Removed: a new key for the others, nothing more for the technician.
    let old_key = seen.link.clone();
    let tech_id = seen.link.me;
    let after = team::rotate(&addr, owner, &links[0], &[tech_id]).await.unwrap();
    assert_eq!(after.link.epoch, 2);
    assert_ne!(after.link.key, old_key.key);
    assert_eq!(after.book.book.entries[&pc.get()].alias.as_deref(), Some("Kasse 1"));
    assert!(team::sync(&addr, tech, &links[2], Some(&old_key)).await.unwrap().is_none());
    let admin_view = team::sync(&addr, admin, &links[1], None).await.unwrap().unwrap();
    assert_eq!(admin_view.link.key, after.link.key);

    // Leaving makes the key stale; the next admin sync starts epoch 3.
    team::leave(&addr, admin).await.unwrap();
    let owner_view = team::sync(&addr, owner, &links[0], None).await.unwrap().unwrap();
    assert_eq!(owner_view.link.epoch, 3);
    assert!(!owner_view.stale);
    assert_eq!(owner_view.members.len(), 1);
}
