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

async fn try_connect(
    server: &str,
    id: DeviceId,
    password: &str,
    member: Option<&SigningKey>,
) -> anyhow::Result<(ctxremote_core::viewer::ViewerSession, std_mpsc::Receiver<ctxremote_core::viewer::ViewerEvent>)> {
    let (tx, rx) = std_mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let session = ctxremote_core::viewer::ViewerSession::connect(server, None, id, password, member, None, None, move |event| {
        let _ = tx.lock().unwrap().send(event);
    })
    .await?;
    Ok((session, rx))
}

#[tokio::test(flavor = "multi_thread")]
async fn team_members_connect_without_a_password_until_they_leave() {
    let (_server, addr) = start_server().await;
    // The host computer's app (a technician's account), a colleague, a stranger.
    let (desk, colleague, stranger) =
        (SigningKey::from_bytes(&[41; 32]), SigningKey::from_bytes(&[42; 32]), SigningKey::from_bytes(&[43; 32]));
    let desk_link = account::create(&addr, &desk).await.unwrap();
    let colleague_link = account::create(&addr, &colleague).await.unwrap();
    let stranger_link = account::create(&addr, &stranger).await.unwrap();
    team::create(&addr, &colleague, &colleague_link, "Werkstatt").await.unwrap();
    let code = team::invite(&addr, &colleague, &colleague_link, TeamRole::Technician).await.unwrap();
    let desk_team = team::join(&addr, &desk, &desk_link, &code).await.unwrap();

    // The desk computer opens itself to the team.
    let (_host, id, config) = start_host_shared(&addr, |_| {}).await;
    let mut app = Config { team: Some(desk_team.link.clone()), device_key: hex::encode(desk.to_bytes()), ..Config::default() };
    app.server = addr.clone();
    let grant = team::TeamAccessGrant::for_host(&app, id).unwrap();
    let password = grant.password.clone();
    assert!(try_connect(&addr, id, &password, Some(&colleague)).await.is_err(), "ohne Freigabe nur ein falsches Passwort");
    config.write().unwrap().team_access = Some(grant.clone());

    // The colleague finds the device in the team's access lists and gets in.
    let mut list = team::AccessList::default();
    list.hosts.insert(id.get(), ("Empfang".into(), team::stamp()));
    team::set_access(&addr, &desk, &desk_link, &list).await.unwrap();
    let seen = team::sync(&addr, &colleague, &colleague_link, None).await.unwrap().unwrap();
    assert_eq!(seen.access.get(&id.get()).map(|(_, name)| name.as_str()), Some("Empfang"));
    let mut viewer = Config { team: Some(seen.link.clone()), ..Config::default() };
    viewer.team_open = seen.access.iter().map(|(host, (_, name))| (*host, name.clone())).collect();
    let chosen = team::access_for(&viewer, id).expect("Team-Passwort für das Gerät");
    assert_eq!(chosen, password);
    let (session, events) = try_connect(&addr, id, &chosen, Some(&colleague)).await.expect("Kollege kommt rein");
    echo(&session, &events, "über das Team");
    session.close();

    // Someone outside the team does not, even with the password.
    let refused = try_connect(&addr, id, &password, Some(&stranger)).await.err().expect("Fremder abgewiesen");
    assert!(refused.to_string().contains("nicht mehr zum Team"), "{refused:#}");
    let _ = stranger_link;

    // Removed: the old password would still fit until renewed, but the
    // membership check stops it right away …
    team::rotate(&addr, &colleague, &colleague_link, &[desk_team.link.me]).await.unwrap();
    let refused = try_connect(&addr, id, &password, Some(&colleague)).await.err().expect("Gerät nicht mehr im Team");
    assert!(refused.to_string().contains("nicht mehr zum Team"), "{refused:#}");
    let code = team::invite(&addr, &colleague, &colleague_link, TeamRole::Technician).await.unwrap();
    let rejoined = team::join(&addr, &desk, &desk_link, &code).await.unwrap();
    // … and once back in, renewing for the new epoch needs no elevation.
    app.team = Some(rejoined.link.clone());
    let renewed = team::TeamAccessGrant::for_host(&app, id).unwrap();
    assert!(grant.renewed_by(&renewed));
    assert!(!renewed.renewed_by(&grant));
    assert!(try_connect(&addr, id, &renewed.password, Some(&colleague)).await.is_err(), "neues Passwort erst nach Erneuerung");
    config.write().unwrap().team_access = Some(renewed.clone());
    let (session, events) = try_connect(&addr, id, &renewed.password, Some(&colleague)).await.expect("nach Erneuerung");
    echo(&session, &events, "neue Epoche");
    session.close();
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_are_reported_to_owners_and_admins_only() {
    use ctxremote_core::report::{self, Outbox, Report, ReportKey, Side};
    let (_server, addr) = start_server().await;
    let (boss, tech, desk) = (SigningKey::from_bytes(&[51; 32]), SigningKey::from_bytes(&[52; 32]), SigningKey::from_bytes(&[53; 32]));
    let boss_link = account::create(&addr, &boss).await.unwrap();
    let tech_link = account::create(&addr, &tech).await.unwrap();
    let desk_link = account::create(&addr, &desk).await.unwrap();
    let created = team::create(&addr, &boss, &boss_link, "Werkstatt").await.unwrap();
    let code = team::invite(&addr, &boss, &boss_link, TeamRole::Technician).await.unwrap();
    let tech_team = team::join(&addr, &tech, &tech_link, &code).await.unwrap();
    let code = team::invite(&addr, &boss, &boss_link, TeamRole::Technician).await.unwrap();
    let desk_team = team::join(&addr, &desk, &desk_link, &code).await.unwrap();

    // The technician's app reports a session it opened.
    let viewer_report = Report {
        side: Side::Viewer,
        device: Some(7),
        device_name: "Laptop".into(),
        peer: Some(8),
        peer_name: "Kasse".into(),
        profile: None,
        started: 100,
        ended: 160,
        outcome: None,
    };
    let outbox = Outbox::open(None);
    outbox.push(viewer_report.clone());
    outbox.flush(&addr, &tech, &ReportKey::for_team(&tech_team.link).unwrap()).await.unwrap();
    assert!(outbox.is_empty());

    // A host opened to the team (its service key is in no account) reports
    // who came in.
    let (_host, id, config) = start_host_shared(&addr, |_| {}).await;
    let mut app = Config { team: Some(desk_team.link.clone()), device_key: hex::encode(desk.to_bytes()), ..Config::default() };
    app.server = addr.clone();
    let grant = team::TeamAccessGrant::for_host(&app, id).unwrap();
    assert!(grant.report.is_some());
    config.write().unwrap().team_access = Some(grant.clone());
    let mut list = team::AccessList::default();
    list.hosts.insert(id.get(), ("Empfang".into(), team::stamp()));
    team::set_access(&addr, &desk, &desk_link, &list).await.unwrap();
    let (session, events) = try_connect(&addr, id, &grant.password, Some(&tech)).await.expect("Techniker kommt rein");
    echo(&session, &events, "Bericht");
    session.close();

    let mut rows = Vec::new();
    for _ in 0..50 {
        let view = team::sync(&addr, &boss, &boss_link, None).await.unwrap().unwrap();
        rows = report::fetch(&addr, &boss, &view).await.unwrap();
        if rows.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].report.as_ref(), Some(&viewer_report));
    assert_eq!(rows[0].member, Some(tech_team.link.me), "the server names the sender");
    let host_side = rows[1].report.as_ref().expect("lesbar");
    assert_eq!((host_side.side, host_side.device), (Side::Host, Some(id.get())));
    assert_eq!(host_side.outcome, Some(ctxremote_core::history::Outcome::Team));
    assert!(host_side.ended >= host_side.started);
    assert_eq!(rows[1].member, None, "the service key is in no account");

    // Technicians do not read them, and a made-up token writes nothing.
    let tech_view = team::sync(&addr, &tech, &tech_link, None).await.unwrap().unwrap();
    assert!(report::fetch(&addr, &tech, &tech_view).await.is_err());
    let mut forged = ReportKey::for_team(&tech_team.link).unwrap();
    forged.token = hex::encode([9u8; 32]);
    let other = Outbox::open(None);
    other.push(viewer_report.clone());
    assert!(other.flush(&addr, &tech, &forged).await.is_err());

    // After a new epoch the old reports still open; the removed technician's
    // token writes no more.
    let after = team::rotate(&addr, &boss, &boss_link, &[tech_team.link.me]).await.unwrap();
    assert_eq!(after.book.past.get(&created.link.epoch), Some(&created.link.key));
    let rows = report::fetch(&addr, &boss, &after).await.unwrap();
    assert!(rows.iter().all(|r| r.report.is_some()), "{rows:?}");
    assert!(other.flush(&addr, &tech, &ReportKey::for_team(&tech_team.link).unwrap()).await.is_err());
    assert_eq!(other.len(), 1, "kept for later");
}

/// The first `Rights` the viewer gets after it came in.
fn first_rights(events: &std_mpsc::Receiver<ctxremote_core::viewer::ViewerEvent>) -> Option<ctxremote_core::proto::session::Permissions> {
    wait_for(events, |e| match e {
        ctxremote_core::viewer::ViewerEvent::Rights(r) => Some(r),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn the_team_policy_caps_what_its_devices_allow() {
    use ctxremote_core::policy::{self, TeamPolicy};
    use ctxremote_core::proto::session::Permissions;
    let (_server, addr) = start_server().await;
    let (boss, tech, desk) = (SigningKey::from_bytes(&[61; 32]), SigningKey::from_bytes(&[62; 32]), SigningKey::from_bytes(&[63; 32]));
    let boss_link = account::create(&addr, &boss).await.unwrap();
    let tech_link = account::create(&addr, &tech).await.unwrap();
    let desk_link = account::create(&addr, &desk).await.unwrap();
    let created = team::create(&addr, &boss, &boss_link, "Werkstatt").await.unwrap();
    let code = team::invite(&addr, &boss, &boss_link, TeamRole::Technician).await.unwrap();
    let tech_team = team::join(&addr, &tech, &tech_link, &code).await.unwrap();
    let code = team::invite(&addr, &boss, &boss_link, TeamRole::Technician).await.unwrap();
    let desk_team = team::join(&addr, &desk, &desk_link, &code).await.unwrap();

    // Only owners and admins set it; every member can read it.
    let rule = TeamPolicy {
        deny_technician: Permissions(Permissions::TERMINAL | Permissions::FILES),
        one_time: false,
        permanent: false,
        changed: 1,
        ..TeamPolicy::default()
    };
    let boss_view = team::sync(&addr, &boss, &boss_link, None).await.unwrap().unwrap();
    assert!(policy::load(&addr, &boss, &boss_view).await.unwrap().is_none());
    assert!(policy::save(&addr, &tech, &tech_team.link, Some(&rule)).await.is_err(), "Techniker dürfen nicht");
    policy::save(&addr, &boss, &created.link, Some(&rule)).await.unwrap();
    let tech_view = team::sync(&addr, &tech, &tech_link, None).await.unwrap().unwrap();
    assert_eq!(policy::load(&addr, &tech, &tech_view).await.unwrap(), Some(rule.clone()));

    // A computer opened to the team, with every right and a permanent password.
    let (host, id, config) = start_host_shared(&addr, |c| c.permanent_password = Some("Lange-Geheim-42".into())).await;
    let mut app = Config { team: Some(desk_team.link.clone()), device_key: hex::encode(desk.to_bytes()), ..Config::default() };
    app.server = addr.clone();
    let grant = team::TeamAccessGrant::for_host(&app, id).unwrap();
    config.write().unwrap().team_access = Some(grant.clone());

    // The technician comes in without terminal and files …
    let (session, events) = try_connect(&addr, id, &grant.password, Some(&tech)).await.expect("Techniker kommt rein");
    let rights = first_rights(&events).expect("Rechte");
    assert_eq!(rights, Permissions(Permissions::ALL.0 & !(Permissions::TERMINAL | Permissions::FILES)));
    // … and the person at the computer cannot give them back.
    let number = host.sessions()[0].0;
    host.set_rights(number, Permissions::ALL).unwrap();
    assert!(!host.session_rights()[0].1.has(Permissions::TERMINAL));
    session.close();
    assert_eq!(config.read().unwrap().team_policy.as_ref().map(|h| &h.policy), Some(&rule), "kept for later");

    // The owner keeps everything.
    let (session, events) = try_connect(&addr, id, &grant.password, Some(&boss)).await.expect("Inhaber kommt rein");
    assert_eq!(first_rights(&events), Some(Permissions::ALL));
    session.close();

    // Passwords the policy closes are refused with a reason.
    let refused = try_connect(&addr, id, &host.password(), None).await.err().expect("Einmalpasswort zu");
    assert!(refused.to_string().contains("Einmalpasswort"), "{refused:#}");
    let refused = try_connect(&addr, id, "Lange-Geheim-42", None).await.err().expect("festes Passwort zu");
    assert!(refused.to_string().contains("festen Passwort"), "{refused:#}");

    // Without the server the policy fetched last still applies.
    let mut broken = grant.clone();
    if let Some(key) = broken.report.as_mut() {
        key.token = hex::encode([9u8; 32]);
    }
    config.write().unwrap().team_access = Some(broken);
    assert!(try_connect(&addr, id, &host.password(), None).await.is_err(), "zuletzt geholte Richtlinie gilt");
    config.write().unwrap().team_access = Some(grant.clone());

    // After a new epoch the owner's app encrypts it anew; removed, it is gone.
    let after = team::rotate(&addr, &boss, &boss_link, &[tech_team.link.me]).await.unwrap();
    app.team = Some(team::sync(&addr, &desk, &desk_link, None).await.unwrap().unwrap().link);
    let renewed = team::TeamAccessGrant::for_host(&app, id).unwrap();
    assert_eq!(policy::fetch(&addr, &desk, renewed.report.as_ref().unwrap()).await.unwrap(), Some(rule.clone()));
    policy::save(&addr, &boss, &after.link, None).await.unwrap();
    assert_eq!(policy::fetch(&addr, &desk, renewed.report.as_ref().unwrap()).await.unwrap(), None);
    config.write().unwrap().team_access = Some(renewed);
    let (session, _events) = try_connect(&addr, id, &host.password(), None).await.expect("Einmalpasswort wieder offen");
    session.close();
    assert!(config.read().unwrap().team_policy.is_none());
}
