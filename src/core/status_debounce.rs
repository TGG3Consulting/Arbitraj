//! Файл: core/status_debounce.rs
//! Что делает: сглаживание мигания статусов ввода и вывода биржи (решение 112). Живой случай 02.11: BingX у одних и
//! тех же 64 монет за час 5 раз отдал «ввод закрыт», а через опрос (2 с) снова «открыт». Каждая такая смена
//! пересобирала справочник монет — монеты пропадали с экрана и появлялись снова, приходили лишние уведомления.
//! Теперь новый статус монеты (ввод/вывод по сети) принимается, только если биржа отдаёт его
//! venue.status_confirm_polls опросов подряд; до этого в справочнике остаётся прежний. Не задано, 0 или 1 — новый
//! статус принимается сразу, как раньше. Первая загрузка справочника принимается целиком.
//! Кто вызывает: core/registry_sync.rs (run — перед сравнением и записью справочника).
//! От чего зависит: core/types, engine.rs (справочник сетей).
//! Настройки: venue.status_confirm_polls (по площадке).

use std::collections::HashMap;

use crate::core::types::{CoinNetworkInfo, VenueId};
use crate::settings::read::At;

use super::engine::{Core, rd};

/// Ключ: тикер и сеть.
type Key = (String, String);

/// Неподтверждённые смены статуса одной биржи: ключ → (новый статус ввода, вывода; опросов подряд).
#[derive(Default)]
pub struct Debounce {
    pending: HashMap<Key, ((bool, bool), u64)>,
}

impl Debounce {
    /// Справочник с подтверждёнными статусами: смена, которая держится меньше venue.status_confirm_polls опросов
    /// подряд, заменяется прежним статусом. Принимает: ядро, биржу, свежий справочник. Возвращает: справочник для
    /// сравнения и записи. Ошибок нет.
    pub fn apply(
        &mut self,
        core: &Core,
        venue: &VenueId,
        mut list: Vec<CoinNetworkInfo>,
    ) -> Vec<CoinNetworkInfo> {
        let at = At {
            venue: Some(&venue.0),
            coin: None,
        };
        let need = core
            .inner
            .deps
            .settings
            .integer("venue.status_confirm_polls", at)
            .unwrap_or(0);
        let old: HashMap<Key, (bool, bool)> = {
            let nets = rd(&core.inner.networks);
            let Some(prev) = nets.get(venue) else {
                self.pending.clear();
                return list;
            };
            prev.iter()
                .map(|i| {
                    (
                        (i.asset.ticker.clone(), i.network.clone()),
                        (i.deposit_enabled, i.withdraw_enabled),
                    )
                })
                .collect()
        };
        let mut seen: HashMap<Key, ((bool, bool), u64)> = HashMap::new();
        for i in list.iter_mut() {
            let key = (i.asset.ticker.clone(), i.network.clone());
            let now = (i.deposit_enabled, i.withdraw_enabled);
            let Some(&was) = old.get(&key) else { continue };
            if now == was {
                continue;
            }
            // Сколько опросов подряд биржа отдаёт именно этот новый статус.
            let n = match self.pending.get(&key) {
                Some((s, n)) if *s == now => n + 1,
                _ => 1,
            };
            if n < need {
                (i.deposit_enabled, i.withdraw_enabled) = was;
                seen.insert(key, (now, n));
            }
        }
        // Остаются только смены, которые всё ещё ждут подтверждения; принятые и вернувшиеся — забываются.
        self.pending = seen;
        list
    }
}
