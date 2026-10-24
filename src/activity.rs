use atri_qq::messages::PushEvent;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ActivityEvent {
    Notification(PushEvent),
    ReactionConfirmed {
        group: u64,
        sequence: u64,
        code: String,
        added: bool,
        operator_uid: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivityRecord {
    pub id: String,
    pub time: i64,
    pub event: ActivityEvent,
}
impl ActivityRecord {
    pub fn new(event: ActivityEvent) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            id: format!("{nanos:020}-{:016x}", rand::random::<u64>()),
            time: (nanos / 1_000_000).min(i64::MAX as u128) as i64,
            event,
        }
    }
    pub fn notification(event: PushEvent) -> Self {
        Self::new(ActivityEvent::Notification(event))
    }
}
#[derive(Clone, Debug, Default)]
pub struct ReactionState {
    pub count: u32,
    pub actors: HashSet<String>,
}
pub type ReactionKey = (u64, u64, String);
pub fn reduce_reaction(states: &mut HashMap<ReactionKey, ReactionState>, event: &ActivityEvent) {
    let (group, sequence, code, added, operator_uid, count) = match event {
        ActivityEvent::Notification(PushEvent::Reaction {
            group_uin,
            sequence,
            code,
            added,
            operator_uid,
            count,
        }) => (
            *group_uin,
            *sequence,
            code,
            *added,
            operator_uid,
            Some(*count),
        ),
        ActivityEvent::ReactionConfirmed {
            group,
            sequence,
            code,
            added,
            operator_uid,
        } => (*group, *sequence, code, *added, operator_uid, None),
        _ => return,
    };
    let state = states.entry((group, sequence, code.clone())).or_default();
    let changed = if operator_uid.is_empty() {
        false
    } else if added {
        state.actors.insert(operator_uid.clone())
    } else {
        state.actors.remove(operator_uid)
    };
    if let Some(count) = count {
        state.count = count;
    } else if changed {
        state.count = if added {
            state.count.saturating_add(1)
        } else {
            state.count.saturating_sub(1)
        };
    }
    if state.count == 0 {
        state.actors.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{ActivityEvent, ActivityRecord, ReactionState, reduce_reaction};
    use atri_qq::messages::PushEvent;
    use std::collections::HashMap;
    fn push(added: bool, count: u32) -> ActivityEvent {
        ActivityEvent::Notification(PushEvent::Reaction {
            group_uin: 42,
            sequence: 7,
            code: "76".into(),
            added,
            count,
            operator_uid: "self".into(),
        })
    }
    fn ack(added: bool) -> ActivityEvent {
        ActivityEvent::ReactionConfirmed {
            group: 42,
            sequence: 7,
            code: "76".into(),
            added,
            operator_uid: "self".into(),
        }
    }
    #[test]
    fn reaction_receipts_and_pushes_are_idempotent_in_both_orders() {
        for push_first in [false, true] {
            let key = (42, 7, "76".into());
            let mut states = HashMap::from([(
                key.clone(),
                ReactionState {
                    count: 3,
                    ..Default::default()
                },
            )]);
            for event in if push_first {
                [push(true, 4), ack(true)]
            } else {
                [ack(true), push(true, 4)]
            } {
                reduce_reaction(&mut states, &event);
            }
            assert_eq!(states[&key].count, 4);
            assert!(states[&key].actors.contains("self"));
            for event in if push_first {
                [push(false, 3), ack(false)]
            } else {
                [ack(false), push(false, 3)]
            } {
                reduce_reaction(&mut states, &event);
            }
            assert_eq!(states[&key].count, 3);
            assert!(!states[&key].actors.contains("self"));
            reduce_reaction(&mut states, &push(false, 0));
            assert_eq!(states[&key].count, 0);
        }
    }
    #[tokio::test]
    async fn notifications_and_confirmed_reactions_restore_from_database_and_keep_account_boundaries()
     {
        let root = tempfile::tempdir().unwrap();
        let db = crate::storage::Database::open(root.path()).unwrap();
        let poke = PushEvent::Poke {
            group_uin: None,
            actor_uin: Some(2),
            target_uin: Some(1),
            parameters: Default::default(),
            raw: vec![1, 2, 3],
        };
        db.put("notification/1/00000001700000000000", &poke)
            .await
            .unwrap();
        let a = db.save_notification(1, &poke).await.unwrap();
        let b = db.save_notification(1, &poke).await.unwrap();
        assert_ne!(a.id, b.id);
        let ActivityEvent::Notification(reaction) = push(true, 4) else {
            unreachable!()
        };
        db.save_notification(1, &reaction).await.unwrap();
        db.save_activity(1, &ActivityRecord::new(ack(true)))
            .await
            .unwrap();
        db.save_notification(2, &poke).await.unwrap();
        db.close().await.unwrap();
        drop(db);
        let db = crate::storage::Database::open(root.path()).unwrap();
        let model = db.restore(1).unwrap();
        assert_eq!(model.activity.len(), 5);
        assert_eq!(model.activity[0].time, 1_700_000_000_000);
        assert_eq!(model.activity.iter().filter(|record| matches!(&record.event, ActivityEvent::Notification(PushEvent::Poke { raw, .. }) if raw == &[1,2,3])).count(), 3);
        let state = &model.reactions[&(42, 7, "76".into())];
        assert_eq!(state.count, 4);
        assert!(state.actors.contains("self"));
        assert_eq!(db.restore(2).unwrap().activity.len(), 1);
        assert!(db.restore(3).unwrap().activity.is_empty());
        db.close().await.unwrap();
    }
}
