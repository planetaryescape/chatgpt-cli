//! Which chats a classification is judging right now, so two runs never
//! judge the same chat at once (and pay twice, or one's late first pass
//! lands on the other's follow-up). `classify` and the Jev guard wait for
//! their chats to be free, then hold them for the whole run; the
//! background Jev takes only chats nobody holds.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;

#[derive(Default)]
pub struct InFlight {
    held: Mutex<HashSet<String>>,
    freed: Notify,
}

/// Chats held until this drops.
pub struct Claim<'a> {
    flight: &'a InFlight,
    ids: Vec<String>,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        let mut held = self.flight.held();
        for id in &self.ids {
            held.remove(id);
        }
        drop(held);
        self.flight.freed.notify_waiters();
    }
}

impl InFlight {
    fn held(&self) -> MutexGuard<'_, HashSet<String>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait until none of `ids` is held, then hold them all. All at once,
    /// so two runs over overlapping chats can't each hold half and wait
    /// for the other.
    pub async fn claim(&self, ids: &[String]) -> Claim<'_> {
        let ids: Vec<String> = ids
            .iter()
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        loop {
            let freed = self.freed.notified();
            tokio::pin!(freed);
            // Registered before the check, so a release in between wakes it.
            freed.as_mut().enable();
            {
                let mut held = self.held();
                if ids.iter().all(|id| !held.contains(id)) {
                    held.extend(ids.iter().cloned());
                    return Claim { flight: self, ids };
                }
            }
            freed.await;
        }
    }

    /// Hold those of `ids` nobody holds, in order; the rest are left out.
    pub fn claim_free(&self, ids: Vec<String>) -> (Claim<'_>, Vec<String>) {
        let mut held = self.held();
        let free: Vec<String> = ids
            .into_iter()
            .filter(|id| held.insert(id.clone()))
            .collect();
        drop(held);
        (
            Claim {
                flight: self,
                ids: free.clone(),
            },
            free,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| (*id).to_owned()).collect()
    }

    #[tokio::test]
    async fn the_background_skips_held_chats_and_a_command_waits_for_them() {
        let flight = InFlight::default();
        let (background, taken) = flight.claim_free(ids(&["a", "b"]));
        assert_eq!(taken, ids(&["a", "b"]));
        let (_other, taken) = flight.claim_free(ids(&["b", "c"]));
        assert_eq!(taken, ids(&["c"]), "b is in flight");
        let wanted = ids(&["a"]);
        let waiting = flight.claim(&wanted);
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), waiting.as_mut())
                .await
                .is_err(),
            "a command waits for the background's chat"
        );
        drop(background);
        let held = tokio::time::timeout(Duration::from_secs(1), waiting).await;
        assert!(held.is_ok(), "and takes it once it's free");
        assert!(flight.claim_free(ids(&["a"])).1.is_empty());
    }
}
