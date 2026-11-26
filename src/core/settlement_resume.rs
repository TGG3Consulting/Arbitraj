//! Файл: core/settlement_resume.rs.
//! Что делает: после холодного старта запускает один экземпляр ранее принятого денежного маршрута.
//! Кто вызывает: запуск ядра после чтения сделок из ledger.sqlite.
//! От чего зависит: опубликованный справочник и записи мандата с исходным кошельком.
//! Настройки: реквизиты и условия повтора читаются исполнителем.
use super::{
    coin_key::CoinKey,
    engine::{Core, lk},
    settlement_cycle::{FIELD, Mandate},
};
use crate::storage::journal::Level;
pub(super) fn resume(core: &Core) {
    let list: Vec<_> = lk(&core.inner.deals)
        .values()
        .filter(|d| d.is_open() && d.plan.get(FIELD).is_some())
        .cloned()
        .collect();
    for d in list {
        let c = core.clone();
        tokio::spawn(async move {
            let key = CoinKey::of(&d.direction.asset);
            if !super::coin_ride::take_settlement(&c, &key) {
                return;
            }
            let g = super::ride_guard::RideGuard {
                core: c,
                key,
                slot: super::ride_guard::Slot {
                    seat: super::coin_ride::Seat::Alone,
                    boarded: None,
                },
            };
            let res = match serde_json::from_value::<Mandate>(d.plan[FIELD].clone()) {
                Ok(p) => {
                    // Фоновые синхронизации каталогов начинают работу позже восстановления ledger. Настроенные
                    // попытки не расходуются на заведомо пустое состояние нового процесса.
                    g.core
                        .inner
                        .deps
                        .log(
                            Level::Info,
                            "core.settlement_restore",
                            Some(&p.market.cex.0),
                            format!(
                                "{}: ожидается первичный справочник исходной сети и целевой монеты",
                                d.id
                            ),
                            None,
                        )
                        .await;
                    let mut events = g.core.subscribe();
                    loop {
                        if super::engine::rd(&g.core.inner.networks).contains_key(&p.market.cex)
                            && super::engine::rd(&g.core.inner.tickers)
                                .pos
                                .contains_key(&p.direction.asset)
                        {
                            break;
                        }
                        if matches!(
                            events.recv().await,
                            Err(tokio::sync::broadcast::error::RecvError::Closed)
                        ) {
                            return;
                        }
                    }
                    Box::pin(crate::gateway::quote_stage::settlement_if(
                        true,
                        super::settlement_run::follow(&g.core, d, &p),
                    ))
                    .await
                }
                Err(e) => Err(format!(
                    "Не удалось прочитать сохранённое расчётное поручение: {e}"
                )),
            };
            if let Err(e) = res {
                g.core
                    .inner
                    .deps
                    .log(Level::Error, "core.settlement_pending", None, e, None)
                    .await;
            }
        });
    }
}
