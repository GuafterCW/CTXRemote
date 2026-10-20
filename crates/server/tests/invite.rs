//! Invitation links end to end: the quick helper offers its ID and password
//! to the device that invited it, sealed so the server cannot read them.

mod common;

use std::time::Duration;

use common::*;
use ctxremote_core::host::HostEvent;
use ctxremote_proto::invite;

#[tokio::test(flavor = "multi_thread")]
async fn invited_helper_offers_access() {
    let (_server, addr) = start_server().await;
    let (inviter, inviter_id) = start_host(&addr, false).await;
    let (helper, helper_id) = start_host(&addr, false).await;
    let token = invite::new_token();
    let mut events = inviter.events();

    helper.offer_help(invite::Inviter::Device(inviter_id), &token, "anna").await.expect("offer delivered");
    let (from, hint, sealed) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(HostEvent::HelpRequest { from, hint, sealed }) = events.recv().await {
                return (from, hint, sealed);
            }
        }
    })
    .await
    .expect("help request arrives");
    assert_eq!(from, helper_id.to_string());
    assert_eq!(hint, invite::hint(&token));
    let offer = invite::open(&token, helper_id, &sealed).expect("the token opens it");
    assert_eq!((offer.from, offer.password.as_str(), offer.name.as_str()), (helper_id, helper.password().as_str(), "anna"));
    assert_eq!(invite::open(&invite::new_token(), helper_id, &sealed), None, "nobody else can read it");

    // With what arrived, the inviter connects (the helper still has to agree as usual).
    let (session, events) = connect(&addr, &helper, helper_id).await;
    echo(&session, &events, "hallo");

    // An inviter that is not online cannot be reached.
    let nobody = ctxremote_proto::DeviceId::new(999_999_998).unwrap();
    let result = helper.offer_help(invite::Inviter::Device(nobody), &invite::new_token(), "anna").await;
    assert_eq!(result, Err("Ihr Helfer ist gerade nicht online".to_string()));
    drop(inviter);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_team_queue_hands_each_offer_to_one_member() {
    use ctxremote_core::proto::account::TeamRole;
    use ctxremote_core::{account, queue, team};
    use ed25519_dalek::SigningKey;
    let (_server, addr) = start_server().await;
    let (boss, tech, outsider) = (SigningKey::from_bytes(&[71; 32]), SigningKey::from_bytes(&[72; 32]), SigningKey::from_bytes(&[73; 32]));
    let boss_link = account::create(&addr, &boss).await.unwrap();
    let tech_link = account::create(&addr, &tech).await.unwrap();
    let outsider_link = account::create(&addr, &outsider).await.unwrap();
    team::create(&addr, &boss, &boss_link, "Werkstatt").await.unwrap();
    let code = team::invite(&addr, &boss, &boss_link, TeamRole::Technician).await.unwrap();
    team::join(&addr, &tech, &tech_link, &code).await.unwrap();
    team::create(&addr, &outsider, &outsider_link, "Andere").await.unwrap();

    // Only owners and admins open a link; its token is in the book for everyone.
    assert!(queue::open_link(&addr, &tech, &tech_link).await.is_err(), "Techniker dürfen nicht");
    let view = queue::open_link(&addr, &boss, &boss_link).await.unwrap();
    let link = view.book.help.clone().expect("Hilfelink im Buch");
    let tech_view = team::sync(&addr, &tech, &tech_link, None).await.unwrap().unwrap();
    assert_eq!(tech_view.book.help.as_ref(), Some(&link));

    // A customer's quick helper sends its access into the queue.
    let (helper, helper_id) = start_host(&addr, false).await;
    let mut events = helper.events();
    helper.offer_help(invite::Inviter::Team(link.queue.clone()), &link.token, "anna").await.expect("in der Warteschlange");
    let waiting = queue::waiting(&addr, &tech, &link).await.unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!((waiting[0].id.as_str(), waiting[0].name.as_str()), (helper_id.to_string().as_str(), "anna"));
    assert_eq!(queue::waiting(&addr, &boss, &link).await.unwrap(), waiting, "alle Mitglieder sehen sie");
    assert!(queue::waiting(&addr, &outsider, &link).await.unwrap().is_empty(), "andere Teams nicht");
    // Asking again replaces the earlier offer.
    helper.offer_help(invite::Inviter::Team(link.queue.clone()), &link.token, "anna").await.unwrap();
    let waiting = queue::waiting(&addr, &tech, &link).await.unwrap();
    assert_eq!(waiting.len(), 1);

    // The technician takes it, the owner is too late; the customer hears of it.
    let offer = queue::claim(&addr, &tech, &link, waiting[0].ticket).await.expect("übernommen");
    assert_eq!((offer.from, offer.password.as_str()), (helper_id, helper.password().as_str()));
    let late = queue::claim(&addr, &boss, &link, waiting[0].ticket).await.err().expect("schon weg");
    assert!(late.to_string().contains("schon jemand"), "{late:#}");
    assert!(queue::waiting(&addr, &boss, &link).await.unwrap().is_empty());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(HostEvent::HelpClaimed) = events.recv().await {
                return;
            }
        }
    })
    .await
    .expect("Kunde erfährt es");
    let (session, session_events) = connect(&addr, &helper, helper_id).await;
    echo(&session, &session_events, "übernommen");

    // A new link ends the old one; a closed one takes nothing.
    let renewed = queue::open_link(&addr, &boss, &boss_link).await.unwrap().book.help.unwrap();
    assert_ne!(renewed.queue, link.queue);
    let old = helper.offer_help(invite::Inviter::Team(link.queue.clone()), &link.token, "anna").await;
    assert!(old.unwrap_err().contains("gilt nicht mehr"));
    queue::close_link(&addr, &boss, &boss_link).await.unwrap();
    let closed = helper.offer_help(invite::Inviter::Team(renewed.queue), &renewed.token, "anna").await;
    assert!(closed.unwrap_err().contains("gilt nicht mehr"));
}
