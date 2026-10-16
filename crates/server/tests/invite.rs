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

    helper.offer_help(inviter_id, &token, "anna").await.expect("offer delivered");
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
    let result = helper.offer_help(nobody, &invite::new_token(), "anna").await;
    assert_eq!(result, Err("Ihr Helfer ist gerade nicht online".to_string()));
    drop(inviter);
}
