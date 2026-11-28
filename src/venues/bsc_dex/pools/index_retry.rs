//! Повтор проверки монет справочника после сбоя узла. Вызывают index::begin и index_watch.
//! Хранит только монеты с неоконченными проверками и ближайший срок dex.verify_retry_s.

use std::collections::HashSet;
use std::time::Duration;

use alloy_primitives::Address;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;

use super::super::ctx::Ctx;
use crate::settings::live::Watch;

#[derive(Default)]
pub(super) struct Retry {
    coins: HashSet<Address>,
    at: Option<Instant>,
    /// С2: сколько сообщений канала забрано с начала подписки (directory/boot.rs handled).
    received: u64,
}

/// Настройки требуют проверить весь справочник, событие монеты или повтор — только указанное множество.
pub(super) enum Work {
    All,
    Coins(HashSet<Address>),
}

impl Retry {
    /// Итог штатной проверки. Успех очищает только проверенные монеты. Срок прежнего сбоя
    /// сохраняется при постороннем обновлении; полный проход перечитывает интервал из настроек.
    pub(super) fn checked(
        &mut self,
        ctx: &Ctx,
        only: Option<&HashSet<Address>>,
        failed: HashSet<Address>,
    ) {
        self.record(only, failed);
        self.arm(ctx);
    }

    fn record(&mut self, only: Option<&HashSet<Address>>, failed: HashSet<Address>) {
        match only {
            Some(checked) => self.coins.retain(|c| !checked.contains(c)),
            None => {
                self.coins.clear();
                self.at = None;
            }
        }
        self.coins.extend(failed);
        if self.coins.is_empty() {
            self.at = None;
        }
    }

    /// С2: сколько сообщений канала «монета обновлена» забрано с начала подписки. Ошибок нет.
    pub(super) fn received(&self) -> u64 {
        self.received
    }

    /// Если интервал не задан, повтор разбудит изменение настроек. Уже назначенный срок сохраняется.
    pub(super) fn arm(&mut self, ctx: &Ctx) {
        self.arm_with(|| {
            ctx.int("dex.verify_retry_s")
                .ok()
                .and_then(|s| Instant::now().checked_add(Duration::from_secs(s)))
        });
    }

    fn arm_with(&mut self, deadline: impl FnOnce() -> Option<Instant>) {
        if !self.coins.is_empty() && self.at.is_none() {
            self.at = deadline();
        }
    }

    /// Таймер с абсолютным сроком имеет приоритет над готовым адресом. Адрес, пришедший
    /// одновременно со сроком, остаётся в канале. Закрытие канала останавливает подписку.
    pub(super) async fn next(
        &mut self,
        rx: &mut UnboundedReceiver<Address>,
        changes: &mut Watch,
    ) -> Option<Work> {
        let deadline = self.at;
        let wait = async {
            match deadline {
                Some(t) if t <= Instant::now() => {}
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            _ = wait => {
                self.at = None;
                Some(Work::Coins(std::mem::take(&mut self.coins)))
            },
            change = changes.next_batch() => change.map(|_| Work::All),
            first = rx.recv() => {
                let mut coins = HashSet::from([first?]);
                self.received += 1;
                // Новые поступления не продлевают сбор пачки бесконечно.
                for _ in 0..rx.len() {
                    let Ok(c) = rx.try_recv() else { break };
                    self.received += 1;
                    coins.insert(c);
                }
                Some(Work::Coins(coins))
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;
    use tokio::sync::{broadcast, mpsc};

    // CAKE и WBNB из записи BSC tests/data/indexers_2026-10-24.
    const A: Address = address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");
    const B: Address = address!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");

    #[tokio::test]
    async fn unrelated_updates_preserve_deadline_and_timer_does_not_consume_ready_update() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (_changes, watch) = broadcast::channel(1);
        let mut watch = Watch::new(watch, vec!["dex.".into()]);
        let a = HashSet::from([A]);
        let b = HashSet::from([B]);
        let deadline = Instant::now() + Duration::from_secs(3600);
        let mut retry = Retry::default();
        retry.record(None, a.clone());
        retry.arm_with(|| Some(deadline));
        for _ in 0..32 {
            tx.send(B).unwrap();
            let Some(Work::Coins(checked)) = retry.next(&mut rx, &mut watch).await else {
                panic!("ожидалась монета")
            };
            assert_eq!(checked, b);
            retry.record(Some(&checked), HashSet::new());
            retry.arm_with(|| panic!("обновление B перенесло срок A"));
            assert_eq!((retry.at, &retry.coins), (Some(deadline), &a));
        }
        // К моменту следующего select готовы и таймер A, и монета B.
        retry.at = Some(Instant::now() - Duration::from_secs(1));
        tx.send(B).unwrap();
        let Some(Work::Coins(due)) = retry.next(&mut rx, &mut watch).await else {
            panic!("ожидался повтор")
        };
        assert_eq!(due, a);
        assert!(retry.at.is_none());
        // A снова не проверен. До нового срока B остался в канале. Второй проход A будет успешным.
        retry.record(Some(&due), a.clone());
        retry.arm_with(|| Some(deadline));
        let Some(Work::Coins(queued)) = retry.next(&mut rx, &mut watch).await else {
            panic!("ожидалось B")
        };
        assert_eq!(queued, b);
        retry.record(Some(&queued), HashSet::new());
        retry.record(Some(&due), HashSet::new());
        assert!(retry.at.is_none() && retry.coins.is_empty());
    }

    #[test]
    fn failures_in_expanded_pools_outside_the_input_survive() {
        let mut retry = Retry::default();
        let a = HashSet::from([A]);
        let b = HashSet::from([B]);
        retry.record(Some(&b), a.clone());
        retry.record(Some(&b), HashSet::new());
        assert_eq!(retry.coins, a);
        retry.record(None, HashSet::new());
        assert!(retry.coins.is_empty());
    }
}
