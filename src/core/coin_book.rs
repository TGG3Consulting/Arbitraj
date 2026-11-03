//! Файл: core/coin_book.rs
//! Что делает: справочник монет ядра снимком (решение 131). Справочник делится на две части:
//! - «рабочие» монеты — у монеты есть адрес контракта в BSC хотя бы у одной биржи: только они могут дать направление,
//!   подписку, пул; их и читают горячие пути (тик, расчёт, исполнитель) — разыменование CoinBook даёт именно их;
//! - «неподвижные» монеты — ни у одной биржи нет адреса контракта (нет сети BSC или сеть без адреса). Направлений,
//!   пулов и подписки у них нет; на экране «Монеты» они видны с причиной, как раньше. Их запись зависит только от
//!   справочника сетей бирж, названий сети BSC и котируемой монеты, состава площадок и ручного чёрного списка
//!   (coin.blocked*): поэтому они собираются заново только при смене этих входов, а не на каждый толчок пересборки.
//!   У MEXC это ~9300 из ~9600 записей (config/getall отдаёт все монеты биржи).
//!
//! Обе части — за `Arc`: снимок справочника (Core::coins, экран) — копия указателей, без копии записей.
//! Кто вызывает: core/rebuild.rs (сборка), core/inner.rs (хранит), читатели справочника ядра.
//! От чего зависит: registry.rs (CoinEntry), registry_build.rs (Networks), core/types.
//! Настройки: нет своих (поколение ручного чёрного списка — по изменению coin.blocked* в rebuild.rs watch_settings).

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;

use super::registry::CoinEntry;
use super::registry_build::Networks;

/// Справочник монет: рабочие и неподвижные (см. шапку).
#[derive(Default, Clone)]
pub struct CoinBook {
    pub live: Arc<Vec<CoinEntry>>,
    pub inert: Arc<Vec<CoinEntry>>,
    /// На чём собраны неподвижные монеты (None — ещё не собирались).
    pub inert_key: Option<InertKey>,
}

impl Deref for CoinBook {
    type Target = Vec<CoinEntry>;

    /// Рабочие монеты (горячие пути). Ошибок нет.
    fn deref(&self) -> &Vec<CoinEntry> {
        &self.live
    }
}

impl CoinBook {
    /// Все монеты для экрана: рабочие, затем неподвижные. Ошибок нет.
    pub fn all(&self) -> impl Iterator<Item = &CoinEntry> {
        self.live.iter().chain(self.inert.iter())
    }

    /// Сколько всего монет. Ошибок нет.
    pub fn total(&self) -> usize {
        self.live.len() + self.inert.len()
    }
}

/// Поколение ручного чёрного списка: растёт при изменении coin.blocked / coin.blocked_note (rebuild.rs).
static MANUAL_GEN: AtomicU64 = AtomicU64::new(0);

/// Изменилась настройка `name` — если это ручной чёрный список, неподвижные монеты собрать заново. Ошибок нет.
pub fn on_setting(name: &str) {
    if name.starts_with("coin.blocked") {
        MANUAL_GEN.fetch_add(1, Ordering::AcqRel);
    }
}

/// На чём собраны неподвижные монеты.
#[derive(Clone)]
pub struct InertKey {
    venues: Vec<(VenueId, VenueKind)>,
    nets: Vec<(VenueId, Arc<Vec<crate::core::types::CoinNetworkInfo>>)>,
    quote: Vec<(VenueId, String)>,
    bsc: Vec<(VenueId, String)>,
    manual: u64,
}

impl InertKey {
    /// Ключ по входам сборки. Ошибок нет.
    pub fn new(
        venues: &[(VenueId, VenueKind)],
        nets: &Networks,
        quote: &HashMap<VenueId, String>,
        bsc: &HashMap<VenueId, String>,
    ) -> InertKey {
        let sorted = |m: &HashMap<VenueId, String>| {
            let mut v: Vec<(VenueId, String)> =
                m.iter().map(|(k, x)| (k.clone(), x.clone())).collect();
            v.sort_by(|a, b| a.0.0.cmp(&b.0.0).then_with(|| a.1.cmp(&b.1)));
            v
        };
        let mut n: Vec<_> = nets.iter().map(|(k, l)| (k.clone(), l.clone())).collect();
        n.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        InertKey {
            venues: venues.to_vec(),
            nets: n,
            quote: sorted(quote),
            bsc: sorted(bsc),
            manual: MANUAL_GEN.load(Ordering::Acquire),
        }
    }

    /// Те же ли входы: списки сетей — тот же объект (Arc), остальное — по значению. Ошибок нет.
    pub fn same(&self, o: &InertKey) -> bool {
        self.venues == o.venues
            && self.quote == o.quote
            && self.bsc == o.bsc
            && self.manual == o.manual
            && self.nets.len() == o.nets.len()
            && self
                .nets
                .iter()
                .zip(&o.nets)
                .all(|(a, b)| a.0 == b.0 && Arc::ptr_eq(&a.1, &b.1))
    }
}
