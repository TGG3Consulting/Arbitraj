//! Файл: core/manual.rs
//! Что делает: ручное управление (решение 37): алерт «убыток выше порога» с кнопками «Продать сейчас»
//! и «Беру на себя»; «Беру на себя» — монеты списываются из учёта бота, бот работает дальше; «Вернуть
//! боту» — монеты возвращаются в учёт по реальному балансу площадки и продаются по общим правилам
//! (выгодно — сразу, иначе режим застрявшей монеты). Алерты и монеты человека хранятся в учёте ядра.
//! Решение 177: продажа кнопкой — ручная команда (risk_count.rs, не «ошибка площадки подряд»); тот же отказ по той же
//! записи — обновление алерта, а не новый (manual_repeat.rs); итог продажи — отдельным событием (manual_outcome.rs).
//! Решение 235: «Продать сейчас» — по рынку без порогов убытка и «выгодно» до конца (sell::BUTTON, manual_sell.rs);
//! «Продать и перегнать» — проход оценки всех площадок, перегон на лучшую и продажа там (manual_relay.rs).
//! Кто вызывает: stuck.rs (raise_alert), commands.rs (команды), engine.rs (load при запуске).
//! От чего зависит: deal.rs, ledger.rs, sell.rs, persist.rs, view.rs, deps.rs (шлюз — балансы), manual_repeat.rs,
//! risk_count.rs.
//! Настройки: нет.

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{ManualHolding, StuckAlert};
use crate::notify::human::{self as hu, Msg};
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::view::CoreEvent;

/// Публикует алерты и монеты человека в интерфейс.
pub fn publish(core: &Core) {
    let alerts = lk(&core.inner.alerts)
        .values()
        .map(|(a, _)| a.clone())
        .collect();
    let holdings = lk(&core.inner.holdings)
        .values()
        .map(|(h, _)| h.clone())
        .collect();
    core.inner.live.send(CoreEvent::Manual { alerts, holdings });
}

/// Сохраняет алерты и монеты человека (вместе со сделкой) в учёт ядра.
pub async fn persist(core: &Core) {
    let db = &core.inner.deps.db;
    let alerts: Vec<(StuckAlert, Deal)> = lk(&core.inner.alerts).values().cloned().collect();
    let holdings: Vec<(ManualHolding, Deal)> = lk(&core.inner.holdings).values().cloned().collect();
    let r1 = super::persist::save(db, "manual_alerts", "all", &alerts).await;
    let r2 = super::persist::save(db, "manual_holdings", "all", &holdings).await;
    if let Err(e) = r1.and(r2) {
        core.inner
            .deps
            .log(
                Level::Error,
                "core.manual",
                None,
                format!("Ручное управление не сохранено: {e}"),
                None,
            )
            .await;
    }
    publish(core);
}

/// Загружает алерты и монеты человека при запуске. Ошибка базы — в журнал.
pub async fn load(core: &Core) {
    let db = &core.inner.deps.db;
    let alerts = super::persist::load::<Vec<(StuckAlert, Deal)>>(db, "manual_alerts").await;
    let holdings = super::persist::load::<Vec<(ManualHolding, Deal)>>(db, "manual_holdings").await;
    match (alerts, holdings) {
        (Ok(a), Ok(h)) => {
            for (x, d) in a.into_iter().flatten() {
                lk(&core.inner.alerts).insert(x.id.clone(), (x, d));
            }
            for (x, d) in h.into_iter().flatten() {
                lk(&core.inner.holdings).insert(x.id.clone(), (x, d));
            }
        }
        (Err(e), _) | (_, Err(e)) => {
            core.inner
                .deps
                .log(
                    Level::Error,
                    "core.manual",
                    None,
                    format!("Ручное управление не загружено: {e}"),
                    None,
                )
                .await
        }
    }
    publish(core);
}

/// Алерт: сделка ждёт решения человека — «убыток выше порога» (stuck.rs) или «продажа не прошла» (sell.rs,
/// решение 100). Уведомление stuck.loss_alert — текст; кнопки — запись manual.record (решение 156).
/// Решение 177: тот же отказ (error) по записи, с которой человек запустил продажу, — та же запись обновляется
/// (причина, попытка), уведомления и события алерта нет (manual_repeat.rs).
/// Принимает: сделку, убыток % (None — котировки нет), причину, текст отказа продажи (None — алерт по убытку).
/// Ошибок нет.
pub async fn raise_alert(
    core: &Core,
    mut deal: Deal,
    loss_pct: Option<Decimal>,
    reason: String,
    error: Option<String>,
) {
    let prev = super::manual_repeat::prior(&mut deal, error.as_deref());
    let id = match (&prev, super::ids::uuid_v4()) {
        (Some(p), _) => p.id.clone(),
        (None, Ok(id)) => id,
        (None, Err(e)) => {
            let why = format!("алерт не создан ({e}) — монеты на месте, нужна ручная проверка");
            return super::ledger::finish(core, &mut deal, Step::Halted, &why).await;
        }
    };
    let alert = StuckAlert {
        id,
        deal_id: deal.id.clone(),
        venue: deal.at.clone(),
        asset: deal.direction.asset.clone(),
        qty: deal.qty,
        loss_pct,
        reason: Some(reason.clone()),
        at: prev.as_ref().map_or_else(Utc::now, |p| p.at),
        error,
        attempts: prev.as_ref().map_or(1, super::manual_repeat::next_attempt),
    };
    // Решение 247, п. Е: подпись по знаку (прибыль / убыток), пометка «по модели», если продажа по ней откатилась.
    let (loss_k, loss) = hu::sale_now_alert(loss_pct, alert.error.as_deref());
    let head = hu::alert_head(loss_pct, alert.error.is_some());
    let hint = hu::min_out_hint(alert.error.as_deref());
    deal.step = Step::WaitingHuman;
    if prev.is_some() {
        deal.note(super::manual_repeat::note(
            &reason,
            alert.attempts,
            &alert.id,
        ));
    } else {
        deal.note(format!(
            "{reason} — ждём решения человека (алерт {})",
            alert.id
        ));
    }
    super::ledger::save(core, &deal).await;
    if prev.is_some() {
        // Решение 177: запись та же — экран и сообщение Telegram обновятся по ней (notify/mirror*.rs), без дубля.
        lk(&core.inner.alerts).insert(alert.id.clone(), (alert, deal));
        return persist(core).await;
    }
    // Решение 164: коротко и по-человечески; кнопки — в сообщении manual.record.
    let mut text = Msg::new(hu::STOP, head)
        .card(super::deal_card::card(&deal))
        .kv(loss_k, &loss)
        .reason(&reason);
    if let Some(h) = hint {
        text = text.kv("Что делает бот", h);
    }
    let text = text
        .todo("«Продать сейчас», «Продать и перегнать» или «Беру на себя» — в сообщении ниже или на экране «Ручное управление»")
        .id(&deal.id)
        .render();
    lk(&core.inner.alerts).insert(alert.id.clone(), (alert.clone(), deal));
    persist(core).await;
    core.inner.live.send(CoreEvent::StuckAlert(alert.clone()));
    // Решение 156: кнопки записи уходят событием manual.record (notify/mirror.rs) — те же, что на экране;
    // здесь — только извещение.
    core.inner
        .deps
        .notify("stuck.loss_alert", Some(&alert.venue.0), &text)
        .await;
}

/// «Продать сейчас» по алерту: количество — по реальному балансу площадки (решение 100: после неудачной продажи
/// монет может быть меньше или уже нет), продажа по рынку без порогов убытка (решение 235). ret — «Продать и
/// перегнать» (решения 101, 235: лучшая площадка после затрат перевода — manual_relay.rs). Возвращает: текст для ответа. Ошибки: алерта нет, монет нет (алерт остаётся).
pub async fn sell_now(core: &Core, alert_id: &str, ret: bool) -> Result<String, String> {
    let (mut alert, mut deal) = lk(&core.inner.alerts)
        .remove(alert_id)
        .ok_or(format!("алерта {alert_id} нет"))?;
    match super::manual_halted::on_hand_sell(core, &deal, &deal.at, &deal.direction.asset, deal.qty)
        .await
    {
        Ok((qty, note)) => super::manual_halted::set_qty(&mut deal, qty, note),
        Err(e) => {
            // Алерт остаётся — человек решит иначе («Продано вручную», «Беру на себя»).
            alert.reason = Some(e.clone());
            lk(&core.inner.alerts).insert(alert_id.to_string(), (alert, deal));
            persist(core).await;
            return Err(e);
        }
    }
    // Решение 247, п. А (Тигран 10.11: согласовано): нажатие — новая цепочка повторов кнопки: счётчик автоматики
    // (Deal.unsent_retries) не наследуется, иначе после исчерпания автоматики кнопка давала одну попытку и Exhausted.
    deal.unsent_retries = 0;
    // Решение 268: и отметки «рыночный ордер отправлен» / «по коридору уходили в режим» — нажатие даёт кнопке свой
    // один рыночный повтор.
    deal.range_market.clear();
    deal.range_stuck.clear();
    deal.step = Step::Selling;
    // Решение 177: тот же отказ — эта запись обновится, а не появится новая (raise_alert, manual_repeat.rs).
    deal.alert_was = Some(alert);
    // Решения 101 и 235: «Продать и перегнать» — лучшая площадка после всех затрат (или здесь), продажа там, затем
    // выручка на площадку покупки.
    if ret {
        deal.return_usdt = true;
        deal.note("«Продать и перегнать»: оценка всех площадок, продажа там, где больше после затрат, затем выручка USDT — на площадку покупки");
    }
    super::ledger::save(core, &deal).await;
    persist(core).await;
    let id = deal.id.clone();
    let c = core.clone();
    // Решение 177: ручная команда — её сбой не «ошибка площадки подряд» (risk_count.rs); итог продажи — отдельным
    // событием (manual_outcome.rs: правка сообщения записи в Telegram).
    // Решение 235: кнопка — без порогов убытка на всём пути (manual_sell.rs); «Продать и перегнать» — manual_relay.rs.
    tokio::spawn(super::risk_count::manual(async move {
        if ret {
            super::manual_relay::run(&c, deal).await
        } else {
            super::sell::market(&c, deal, super::sell::BUTTON).await
        }
    }));
    Ok(if ret {
        format!("Сделка {id}: оценка площадок и продажа по рынку запущены — итог придёт отдельно")
    } else {
        format!("Сделка {id}: продажа по рынку запущена — итог придёт отдельно")
    })
}

/// «Беру на себя»: монеты списываются из учёта бота. Возвращает: текст. Ошибка: алерта нет.
pub async fn take_over(core: &Core, alert_id: &str, who: &str) -> Result<String, String> {
    let (alert, mut deal) = lk(&core.inner.alerts)
        .remove(alert_id)
        .ok_or(format!("алерта {alert_id} нет"))?;
    let h = ManualHolding {
        id: super::ids::uuid_v4().unwrap_or_else(|_| alert.id.clone()),
        deal_id: deal.id.clone(),
        venue: alert.venue.clone(),
        asset: alert.asset.clone(),
        qty: deal.qty,
        reason: format!(
            "{} (взял: {who})",
            alert.reason.clone().unwrap_or_else(|| alert
                .loss_pct
                .map(|l| format!("убыток {}", hu::pct(l)))
                .unwrap_or_default())
        ),
        taken_at: Utc::now(),
    };
    super::ledger::finish(
        core,
        &mut deal,
        Step::TakenOver,
        "«Беру на себя» — монеты у человека",
    )
    .await;
    lk(&core.inner.holdings).insert(h.id.clone(), (h.clone(), deal));
    persist(core).await;
    let text = format!(
        "{} {} на {} — у вас ({})",
        hu::qty(h.qty),
        h.asset.ticker,
        hu::venue(&h.venue.0),
        h.reason
    );
    let tg = Msg::new(hu::INFO, "Монеты взяты на себя — бот их больше не ведёт")
        .kv(
            "Монеты",
            format!(
                "{} {} на {}",
                hu::qty(h.qty),
                h.asset.ticker,
                hu::venue(&h.venue.0)
            ),
        )
        .reason(&h.reason)
        .todo("продать самому и нажать «Продано вручную» или «Вернуть боту»")
        .id(&h.deal_id)
        .render();
    core.inner
        .deps
        .notify("manual.taken", Some(&h.venue.0), &tg)
        .await;
    Ok(text)
}
