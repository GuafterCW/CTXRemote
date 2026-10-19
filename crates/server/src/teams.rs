//! Teams: accounts that share an encrypted address book, with roles the
//! server enforces (see `docs/ACCOUNTS.md`, "Teams").
//!
//! The server keeps the team's blobs, the members' exchange keys with their
//! tags and the team key sealed for each, but never the team key itself.
//! Invitations live in memory only, like pairings.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ctxremote_proto::account::{
    valid_code_id, AccountError, AccountOp, AccountReply, MemberKey, Rotation, TeamMember, TeamRole, TeamState, MAX_BOOK,
    MAX_TEAM, PAIRING_SECS,
};
use serde::{Deserialize, Serialize};

use super::{constant_time_eq, unix_now, Accounts, MAX_TRIES};

/// Largest sealed team key (ephemeral key, nonce, key, tag).
const MAX_SEALED: usize = 256;
/// Largest access list of one member.
const MAX_ACCESS: usize = 64 * 1024;
/// Open invitations per team.
const MAX_INVITES: usize = 5;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Team {
    /// Hex; part of what the team's blobs are bound to.
    id: String,
    epoch: u32,
    revision: u64,
    /// The shared address book, encrypted with the team key (hex).
    book: String,
    members: BTreeMap<u64, Seat>,
    /// Someone left without a new epoch; the next admin's app starts one.
    #[serde(default)]
    stale: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Seat {
    role: TeamRole,
    since: u64,
    /// `None` until the member confirmed after joining.
    #[serde(default)]
    key: Option<StoredKey>,
    /// The member's access list, encrypted with the team key (hex).
    #[serde(default)]
    access: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredKey {
    public_key: String,
    tag: String,
    sealed: String,
}

impl StoredKey {
    fn from_wire(key: &MemberKey) -> Result<Self, AccountError> {
        if key.sealed.len() > MAX_SEALED {
            return Err(AccountError::TooLarge);
        }
        Ok(Self { public_key: hex::encode(key.public_key), tag: hex::encode(key.tag), sealed: hex::encode(&key.sealed) })
    }
}

pub(super) struct Invite {
    team: u64,
    role: TeamRole,
    salt: [u8; 16],
    verifier: [u8; 32],
    sealed: Vec<u8>,
    expires: Instant,
    tries: u32,
}

fn unhex32(text: &str) -> [u8; 32] {
    hex::decode(text).ok().and_then(|b| b.try_into().ok()).unwrap_or([0; 32])
}

impl Accounts {
    /// The team `account` is in.
    fn team_of(&self, account: u64) -> Option<u64> {
        self.stored.teams.iter().find(|(_, t)| t.members.contains_key(&account)).map(|(id, _)| *id)
    }

    /// Whether both device keys belong to confirmed members of one team.
    pub fn same_team(&self, a: &[u8; 32], b: &[u8; 32]) -> bool {
        let (Some(&x), Some(&y)) = (self.by_key.get(&hex::encode(a)), self.by_key.get(&hex::encode(b))) else {
            return false;
        };
        let confirmed = |account: u64| {
            let team = self.team_of(account)?;
            self.stored.teams[&team].members[&account].key.as_ref().map(|_| team)
        };
        matches!((confirmed(x), confirmed(y)), (Some(s), Some(t)) if s == t)
    }

    fn team_state(&self, team: u64, me: u64) -> TeamState {
        let t = &self.stored.teams[&team];
        let members = t
            .members
            .iter()
            .map(|(&id, seat)| TeamMember {
                id,
                role: seat.role,
                email: self.email(id),
                key: seat.key.as_ref().map(|k| (unhex32(&k.public_key), unhex32(&k.tag))),
                since: seat.since,
            })
            .collect();
        TeamState {
            id: hex::decode(&t.id).ok().and_then(|b| b.try_into().ok()).unwrap_or([0; 16]),
            epoch: t.epoch,
            revision: t.revision,
            blob: hex::decode(&t.book).unwrap_or_default(),
            me,
            sealed: t.members[&me].key.as_ref().and_then(|k| hex::decode(&k.sealed).ok()),
            members,
            stale: t.stale,
            access: t
                .members
                .iter()
                .filter(|(_, s)| !s.access.is_empty())
                .map(|(&id, s)| (id, hex::decode(&s.access).unwrap_or_default()))
                .collect(),
        }
    }

    /// Takes `account` out of its team when the account goes away: an
    /// owner hands over to the longest-serving admin, a team without
    /// members ends.
    pub(super) fn drop_from_team(&mut self, account: u64) {
        let Some(id) = self.team_of(account) else { return };
        let team = self.stored.teams.get_mut(&id).expect("found above");
        let seat = team.members.remove(&account).expect("member of it");
        if team.members.is_empty() {
            self.stored.teams.remove(&id);
            self.invites.retain(|_, i| i.team != id);
            return;
        }
        team.stale = true;
        if seat.role == TeamRole::Owner {
            let heir = team
                .members
                .iter()
                .filter(|(_, s)| s.role == TeamRole::Admin)
                .min_by_key(|(id, s)| (s.since, **id))
                .map(|(id, _)| *id);
            match heir {
                Some(heir) => team.members.get_mut(&heir).expect("found above").role = TeamRole::Owner,
                None => {
                    self.stored.teams.remove(&id);
                    self.invites.retain(|_, i| i.team != id);
                }
            }
        }
    }

    /// The team operations of [`AccountOp`], for `account`.
    pub(super) fn run_team(&mut self, account: u64, op: AccountOp) -> Result<AccountReply, AccountError> {
        let now = Instant::now();
        self.invites.retain(|_, i| i.expires > now);
        let team = self.team_of(account);
        // The caller's role in its team, for the ops that need one.
        let role = team.map(|t| self.stored.teams[&t].members[&account].role);
        let managing = || match (team, role) {
            (Some(t), Some(r)) if r.manages() => Ok(t),
            (Some(_), _) => Err(AccountError::NotAllowed),
            (None, _) => Err(AccountError::NoTeam),
        };

        match op {
            AccountOp::CreateTeam { key, blob } => {
                if team.is_some() {
                    return Err(AccountError::AlreadyInTeam);
                }
                if blob.len() > MAX_BOOK {
                    return Err(AccountError::TooLarge);
                }
                let seat = Seat { role: TeamRole::Owner, since: unix_now(), key: Some(StoredKey::from_wire(&key)?), access: String::new() };
                let id = self.stored.next_team.max(1);
                let before = self.snapshot();
                self.stored.next_team = id + 1;
                self.stored.teams.insert(
                    id,
                    Team {
                        id: hex::encode(rand::random::<[u8; 16]>()),
                        epoch: 1,
                        revision: 1,
                        book: hex::encode(&blob),
                        members: BTreeMap::from([(account, seat)]),
                        stale: false,
                    },
                );
                self.commit(before)?;
                Ok(AccountReply::Team(Some(self.team_state(id, account))))
            }
            AccountOp::GetTeam => Ok(AccountReply::Team(team.map(|t| self.team_state(t, account)))),
            AccountOp::OfferInvite { code_id, salt, verifier, sealed, role: newcomer } => {
                let id = managing()?;
                if newcomer == TeamRole::Owner {
                    return Err(AccountError::NotAllowed);
                }
                if !valid_code_id(&code_id) || sealed.len() > MAX_SEALED {
                    return Err(AccountError::UnknownCode);
                }
                let open = self.invites.values().filter(|i| i.team == id).count();
                if self.stored.teams[&id].members.len() + open >= MAX_TEAM || open >= MAX_INVITES {
                    return Err(AccountError::TeamFull);
                }
                if self.invites.contains_key(&code_id) || self.pairings.contains_key(&code_id) {
                    return Err(AccountError::Conflict);
                }
                let expires = now + Duration::from_secs(PAIRING_SECS.into());
                self.invites.insert(code_id, Invite { team: id, role: newcomer, salt, verifier, sealed, expires, tries: 0 });
                Ok(AccountReply::Done)
            }
            AccountOp::InviteSalt { code_id } => {
                self.invites.get(&code_id).map(|i| AccountReply::Salt(i.salt)).ok_or(AccountError::UnknownCode)
            }
            AccountOp::JoinTeam { code_id, proof, public_key: _ } => {
                let invite = self.invites.get_mut(&code_id).ok_or(AccountError::UnknownCode)?;
                if !constant_time_eq(&invite.verifier, &proof) {
                    invite.tries += 1;
                    if invite.tries >= MAX_TRIES {
                        self.invites.remove(&code_id);
                    }
                    return Err(AccountError::WrongCode);
                }
                if team.is_some() {
                    return Err(AccountError::AlreadyInTeam);
                }
                let id = invite.team;
                if !self.stored.teams.contains_key(&id) {
                    self.invites.remove(&code_id);
                    return Err(AccountError::UnknownCode);
                }
                if self.stored.teams[&id].members.len() >= MAX_TEAM {
                    return Err(AccountError::TeamFull);
                }
                let invite = self.invites.remove(&code_id).expect("looked up above");
                let before = self.snapshot();
                let seat = Seat { role: invite.role, since: unix_now(), key: None, access: String::new() };
                self.stored.teams.get_mut(&id).expect("checked above").members.insert(account, seat);
                self.commit(before)?;
                Ok(AccountReply::JoinedTeam { sealed: invite.sealed, team: self.team_state(id, account) })
            }
            AccountOp::ConfirmMember { epoch, key } => {
                let id = team.ok_or(AccountError::NoTeam)?;
                if self.stored.teams[&id].epoch != epoch {
                    return Err(AccountError::StaleKey);
                }
                let stored = StoredKey::from_wire(&key)?;
                let before = self.snapshot();
                self.stored.teams.get_mut(&id).expect("its team").members.get_mut(&account).expect("member").key = Some(stored);
                self.commit(before)?;
                Ok(AccountReply::Done)
            }
            AccountOp::PutTeamBook { epoch, base, blob } => {
                let id = managing()?;
                let t = &self.stored.teams[&id];
                if t.epoch != epoch {
                    return Err(AccountError::StaleKey);
                }
                if t.revision != base {
                    return Err(AccountError::Conflict);
                }
                if blob.len() > MAX_BOOK {
                    return Err(AccountError::TooLarge);
                }
                let before = self.snapshot();
                let t = self.stored.teams.get_mut(&id).expect("its team");
                t.revision += 1;
                t.book = hex::encode(&blob);
                let revision = t.revision;
                self.commit(before)?;
                Ok(AccountReply::Stored { revision })
            }
            AccountOp::SetRole { member, role: new } => {
                let id = managing()?;
                let caller = role.expect("managing members have a role");
                let target = self.stored.teams[&id].members.get(&member).ok_or(AccountError::NotLinked)?.role;
                let before = self.snapshot();
                let seats = &mut self.stored.teams.get_mut(&id).expect("its team").members;
                if new == TeamRole::Owner {
                    // The handover: only the owner, to someone else.
                    if caller != TeamRole::Owner || member == account {
                        return Err(AccountError::NotAllowed);
                    }
                    seats.get_mut(&account).expect("member").role = TeamRole::Admin;
                } else if target == TeamRole::Owner {
                    return Err(AccountError::OwnerStays);
                }
                seats.get_mut(&member).expect("checked above").role = new;
                self.commit(before)?;
                Ok(AccountReply::Done)
            }
            AccountOp::Rotate { remove, rotation } => {
                let id = managing()?;
                self.rotate(id, account, &remove, rotation)?;
                Ok(AccountReply::Team(Some(self.team_state(id, account))))
            }
            AccountOp::LeaveTeam => {
                team.ok_or(AccountError::NoTeam)?;
                if role == Some(TeamRole::Owner) {
                    return Err(AccountError::OwnerStays);
                }
                let before = self.snapshot();
                self.drop_from_team(account);
                self.commit(before)?;
                Ok(AccountReply::Done)
            }
            AccountOp::DeleteTeam => {
                let id = team.ok_or(AccountError::NoTeam)?;
                if role != Some(TeamRole::Owner) {
                    return Err(AccountError::NotAllowed);
                }
                let before = self.snapshot();
                self.stored.teams.remove(&id);
                self.commit(before)?;
                self.invites.retain(|_, i| i.team != id);
                Ok(AccountReply::Done)
            }
            AccountOp::SetTeamAccess { epoch, blob } => {
                let id = team.ok_or(AccountError::NoTeam)?;
                if self.stored.teams[&id].epoch != epoch {
                    return Err(AccountError::StaleKey);
                }
                if blob.len() > MAX_ACCESS {
                    return Err(AccountError::TooLarge);
                }
                let before = self.snapshot();
                self.stored.teams.get_mut(&id).expect("its team").members.get_mut(&account).expect("member").access =
                    hex::encode(&blob);
                self.commit(before)?;
                Ok(AccountReply::Done)
            }
            _ => unreachable!("only team operations come here"),
        }
    }

    /// Removes `remove` and moves team `id` to the next epoch, if `rotation`
    /// covers exactly the members that stay.
    fn rotate(&mut self, id: u64, caller: u64, remove: &[u64], rotation: Rotation) -> Result<(), AccountError> {
        let t = &self.stored.teams[&id];
        if rotation.epoch != t.epoch + 1 {
            return Err(AccountError::StaleKey);
        }
        if rotation.base != t.revision {
            return Err(AccountError::Conflict);
        }
        if rotation.blob.len() > MAX_BOOK {
            return Err(AccountError::TooLarge);
        }
        for member in remove {
            match t.members.get(member) {
                None => return Err(AccountError::NotLinked),
                Some(seat) if seat.role == TeamRole::Owner => return Err(AccountError::OwnerStays),
                // Leaving is `LeaveTeam`.
                Some(_) if *member == caller => return Err(AccountError::NotAllowed),
                Some(_) => {}
            }
        }
        let staying: Vec<u64> = t.members.keys().filter(|m| !remove.contains(m)).copied().collect();
        let mut covered: Vec<u64> = rotation.members.iter().map(|(m, _)| *m).collect();
        covered.sort_unstable();
        covered.dedup();
        if covered != staying || covered.len() != rotation.members.len() {
            return Err(AccountError::StaleKey);
        }
        let mut keys = BTreeMap::new();
        for (member, key) in &rotation.members {
            keys.insert(*member, StoredKey::from_wire(key)?);
        }
        let mut access = BTreeMap::new();
        for (member, blob) in rotation.access {
            if !staying.contains(&member) || blob.len() > MAX_ACCESS {
                return Err(AccountError::TooLarge);
            }
            access.insert(member, hex::encode(blob));
        }

        let before = self.snapshot();
        let t = self.stored.teams.get_mut(&id).expect("its team");
        t.members.retain(|m, _| !remove.contains(m));
        for (member, seat) in t.members.iter_mut() {
            seat.key = keys.remove(member);
            seat.access = access.remove(member).unwrap_or_default();
        }
        t.epoch = rotation.epoch;
        t.revision += 1;
        t.book = hex::encode(&rotation.blob);
        t.stale = false;
        self.commit(before)?;
        // Their sealed keys belong to the old epoch.
        self.invites.retain(|_, i| i.team != id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Accounts {
        let path = std::env::temp_dir().join(format!("ctxremote-teams-{}.json", rand::random::<u32>()));
        Accounts::open(path, [3; 32]).unwrap()
    }

    /// A new account with one device; returns its ID.
    fn account(store: &mut Accounts, device: u8) -> u64 {
        store.handle([device; 32], AccountOp::Create).unwrap();
        store.by_key[&hex::encode([device; 32])]
    }

    fn key(n: u8) -> MemberKey {
        MemberKey { public_key: [n; 32], tag: [n; 32], sealed: vec![n; 60] }
    }

    fn team(store: &mut Accounts, device: u8) -> TeamState {
        match store.handle([device; 32], AccountOp::GetTeam).unwrap() {
            AccountReply::Team(Some(t)) => t,
            other => panic!("{other:?}"),
        }
    }

    fn invite(store: &mut Accounts, device: u8, code: &str, role: TeamRole) -> Result<AccountReply, AccountError> {
        let op = AccountOp::OfferInvite { code_id: code.into(), salt: [1; 16], verifier: [9; 32], sealed: vec![7; 60], role };
        store.handle([device; 32], op)
    }

    fn join(store: &mut Accounts, device: u8, code: &str) {
        let op = AccountOp::JoinTeam { code_id: code.into(), proof: [9; 32], public_key: [device; 32] };
        let AccountReply::JoinedTeam { sealed, .. } = store.handle([device; 32], op).unwrap() else { panic!() };
        assert_eq!(sealed, vec![7; 60]);
        let epoch = team(store, device).epoch;
        store.handle([device; 32], AccountOp::ConfirmMember { epoch, key: key(device) }).unwrap();
    }

    fn rotation(t: &TeamState, remove: &[u64]) -> Rotation {
        let members = t.members.iter().filter(|m| !remove.contains(&m.id)).map(|m| (m.id, key(m.id as u8))).collect();
        Rotation { epoch: t.epoch + 1, base: t.revision, blob: vec![t.epoch as u8 + 1], members, access: vec![] }
    }

    #[test]
    fn roles_decide_who_manages() {
        let mut s = store();
        let (owner, admin, tech) = (account(&mut s, 1), account(&mut s, 2), account(&mut s, 3));
        s.handle([1; 32], AccountOp::CreateTeam { key: key(1), blob: vec![1] }).unwrap();
        assert_eq!(s.handle([1; 32], AccountOp::CreateTeam { key: key(1), blob: vec![1] }), Err(AccountError::AlreadyInTeam));

        invite(&mut s, 1, "AAAA", TeamRole::Admin).unwrap();
        assert_eq!(invite(&mut s, 1, "BBBB", TeamRole::Owner), Err(AccountError::NotAllowed));
        join(&mut s, 2, "AAAA");
        invite(&mut s, 2, "CCCC", TeamRole::Technician).unwrap();
        // A wrong proof three times ends the invitation.
        for _ in 0..3 {
            let op = AccountOp::JoinTeam { code_id: "CCCC".into(), proof: [8; 32], public_key: [3; 32] };
            assert_eq!(s.handle([3; 32], op), Err(AccountError::WrongCode));
        }
        let op = AccountOp::JoinTeam { code_id: "CCCC".into(), proof: [9; 32], public_key: [3; 32] };
        assert_eq!(s.handle([3; 32], op), Err(AccountError::UnknownCode));
        invite(&mut s, 2, "DDDD", TeamRole::Technician).unwrap();
        join(&mut s, 3, "DDDD");

        let t = team(&mut s, 3);
        assert_eq!(t.members.len(), 3);
        assert_eq!(t.me, tech);
        assert_eq!(t.sealed, Some(vec![3; 60]));
        assert!(s.same_team(&[1; 32], &[3; 32]));
        assert!(!s.same_team(&[1; 32], &[4; 32]));

        // Technicians read but do not change or invite.
        let put = |epoch, base| AccountOp::PutTeamBook { epoch, base, blob: vec![5] };
        assert_eq!(s.handle([3; 32], put(1, t.revision)), Err(AccountError::NotAllowed));
        assert_eq!(invite(&mut s, 3, "EEEE", TeamRole::Technician), Err(AccountError::NotAllowed));
        assert_eq!(s.handle([2; 32], put(1, t.revision + 5)), Err(AccountError::Conflict));
        assert_eq!(s.handle([2; 32], put(2, t.revision)), Err(AccountError::StaleKey));
        s.handle([2; 32], put(1, t.revision)).unwrap();

        // Nobody but the owner touches the owner; the handover makes it an admin.
        assert_eq!(s.handle([2; 32], AccountOp::SetRole { member: owner, role: TeamRole::Technician }), Err(AccountError::OwnerStays));
        assert_eq!(s.handle([2; 32], AccountOp::SetRole { member: tech, role: TeamRole::Owner }), Err(AccountError::NotAllowed));
        assert_eq!(s.handle([1; 32], AccountOp::LeaveTeam), Err(AccountError::OwnerStays));
        s.handle([1; 32], AccountOp::SetRole { member: admin, role: TeamRole::Owner }).unwrap();
        let roles: Vec<_> = team(&mut s, 1).members.iter().map(|m| (m.id, m.role)).collect();
        assert_eq!(roles, vec![(owner, TeamRole::Admin), (admin, TeamRole::Owner), (tech, TeamRole::Technician)]);
        assert_eq!(s.handle([3; 32], AccountOp::DeleteTeam), Err(AccountError::NotAllowed));
    }

    #[test]
    fn removing_needs_a_new_epoch_for_everyone_who_stays() {
        let mut s = store();
        let (_owner, _admin, tech) = (account(&mut s, 1), account(&mut s, 2), account(&mut s, 3));
        s.handle([1; 32], AccountOp::CreateTeam { key: key(1), blob: vec![1] }).unwrap();
        invite(&mut s, 1, "AAAA", TeamRole::Admin).unwrap();
        join(&mut s, 2, "AAAA");
        invite(&mut s, 1, "BBBB", TeamRole::Technician).unwrap();
        join(&mut s, 3, "BBBB");
        s.handle([3; 32], AccountOp::SetTeamAccess { epoch: 1, blob: vec![3] }).unwrap();
        // An open invitation carries the old key and goes with the epoch.
        invite(&mut s, 1, "CCCC", TeamRole::Technician).unwrap();

        let t = team(&mut s, 2);
        let mut short = rotation(&t, &[tech]);
        short.members.pop();
        let rotate = |remove: Vec<u64>, rotation| AccountOp::Rotate { remove, rotation };
        assert_eq!(s.handle([2; 32], rotate(vec![tech], short)), Err(AccountError::StaleKey));
        assert_eq!(s.handle([3; 32], rotate(vec![], rotation(&t, &[]))), Err(AccountError::NotAllowed));
        let old = Rotation { epoch: 1, ..rotation(&t, &[tech]) };
        assert_eq!(s.handle([2; 32], rotate(vec![tech], old)), Err(AccountError::StaleKey));
        s.handle([2; 32], rotate(vec![tech], rotation(&t, &[tech]))).unwrap();

        let t = team(&mut s, 1);
        assert_eq!(t.epoch, 2);
        assert_eq!(t.members.len(), 2);
        assert_eq!(t.blob, vec![2]);
        assert!(!s.same_team(&[1; 32], &[3; 32]));
        assert_eq!(s.handle([3; 32], AccountOp::GetTeam), Ok(AccountReply::Team(None)));
        let op = AccountOp::JoinTeam { code_id: "CCCC".into(), proof: [9; 32], public_key: [3; 32] };
        assert_eq!(s.handle([3; 32], op), Err(AccountError::UnknownCode));

        // Leaving marks the key stale until an admin starts the next epoch.
        s.handle([2; 32], AccountOp::LeaveTeam).unwrap();
        let t = team(&mut s, 1);
        assert!(t.stale);
        s.handle([1; 32], rotate(vec![], rotation(&t, &[]))).unwrap();
        assert!(!team(&mut s, 1).stale);
    }

    #[test]
    fn a_deleted_owner_hands_the_team_to_the_oldest_admin() {
        let mut s = store();
        let (owner, admin) = (account(&mut s, 1), account(&mut s, 2));
        s.handle([1; 32], AccountOp::CreateTeam { key: key(1), blob: vec![1] }).unwrap();
        invite(&mut s, 1, "AAAA", TeamRole::Admin).unwrap();
        join(&mut s, 2, "AAAA");
        // The owner's only device leaves its account, which ends the account.
        s.handle([1; 32], AccountOp::Leave).unwrap();
        let t = team(&mut s, 2);
        assert_eq!(t.members.iter().map(|m| (m.id, m.role)).collect::<Vec<_>>(), vec![(admin, TeamRole::Owner)]);
        assert!(t.members.iter().all(|m| m.id != owner));
        // Saved and loaded again, nothing is lost.
        let again = Accounts::open(s.path.clone(), [3; 32]).unwrap();
        assert_eq!(again.stored.teams.len(), 1);
    }
}
